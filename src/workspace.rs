// SPDX-License-Identifier: MIT

//! "Open on a new workspace": shows a FreeRDP window on an empty COSMIC
//! workspace (ADR 0011), by switching there before it opens, or moving it
//! there when it opened elsewhere. Also brings a session's window to the
//! front ([`focus`], `activate` in `cosmic-toplevel-management`).
//!
//! FreeRDP is started with a known app ID (`/wm-class:`) and window title
//! (`/title:`); this module waits for that window with COSMIC's window
//! information protocols (`ext-foreign-toplevel-list` with
//! `cosmic-toplevel-info`). An empty workspace (or a new one from COSMIC)
//! is activated with `ext-workspace` first; a window that still opens
//! elsewhere is moved with `cosmic-toplevel-management`
//! (`move_to_ext_workspace`, version 4). cosmic-comp offers these protocols only to unsandboxed
//! clients (`src/state.rs`), which is one reason the app is native only.
//!
//! Blocking, with its own short-lived Wayland connection; call it from
//! `spawn_blocking`. Uses cosmic-client-toolkit as re-exported by libcosmic.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cosmic::cctk::{
    self,
    cosmic_protocols::{
        toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
        toplevel_management::v1::client::zcosmic_toplevel_manager_v1,
    },
    sctk::{
        self,
        output::{OutputHandler, OutputState},
        registry::{ProvidesRegistryState, RegistryState},
        registry_handlers,
        seat::{Capability, SeatHandler, SeatState},
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    toplevel_management::{ToplevelManagerHandler, ToplevelManagerState},
    wayland_client::{
        Connection, Proxy, QueueHandle, WEnum,
        globals::registry_queue_init,
        protocol::{wl_output, wl_seat},
    },
    wayland_protocols::ext::{
        foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1,
        workspace::v1::client::{ext_workspace_group_handle_v1, ext_workspace_handle_v1},
    },
    workspace::{WorkspaceHandler, WorkspaceState},
};

const POLL: Duration = Duration::from_millis(200);
/// How often "keep the screen on" looks at the windows: a lock or screen
/// blank comes after minutes, so a second is quick enough.
const WATCH_POLL: Duration = Duration::from_secs(1);
/// How long to wait for COSMIC to create a requested workspace.
const CREATE_WAIT: Duration = Duration::from_secs(3);
/// How long [`settle`] waits for the window list after connecting. Measured
/// arrival: 65–130 ms; with no windows at all the whole time is used.
const SETTLE_WAIT: Duration = Duration::from_millis(500);
/// How often [`settle`] looks again.
const SETTLE_POLL: Duration = Duration::from_millis(20);

struct State {
    registry: RegistryState,
    outputs: OutputState,
    info: ToplevelInfoState,
    manager: ToplevelManagerState,
    workspaces: WorkspaceState,
    seats: SeatState,
    can_move: bool,
    can_activate: bool,
}

/// Why the window wasn't moved; for the log only.
#[derive(Debug)]
pub enum MoveError {
    NoWayland(String),
    /// COSMIC's window management isn't offered (not COSMIC, or too old).
    Unsupported(&'static str),
    WindowNotFound,
    NoWorkspace,
    Cancelled,
    /// COSMIC didn't activate the window.
    NotActivated,
    /// Moved to the new workspace, but COSMIC didn't switch to it.
    MovedNotShown,
    /// Asked to switch to the empty workspace before the window opened,
    /// but COSMIC didn't.
    NotSwitched,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWayland(e) => write!(f, "no Wayland connection: {e}"),
            Self::Unsupported(what) => write!(f, "compositor doesn't offer {what}"),
            Self::WindowNotFound => write!(f, "window not found in time"),
            Self::NoWorkspace => write!(f, "no empty workspace"),
            Self::Cancelled => write!(f, "session ended first"),
            Self::NotActivated => write!(f, "compositor didn't activate the window"),
            Self::MovedNotShown => write!(f, "moved, but the compositor didn't switch to it"),
            Self::NotSwitched => write!(f, "the compositor didn't switch to the empty workspace"),
        }
    }
}

/// How the window got onto its new workspace; for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// It opened on the workspace switched to beforehand: no move.
    OpenedThere,
    /// It opened elsewhere and was moved.
    Moved,
}

impl std::fmt::Display for Placed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenedThere => write!(f, "the window opened there"),
            Self::Moved => write!(f, "the window was moved there"),
        }
    }
}

fn no_wayland(e: impl std::fmt::Display) -> MoveError {
    MoveError::NoWayland(e.to_string())
}

/// Connects to the compositor and binds the window list, window
/// management, workspaces, outputs and seats.
fn connect() -> Result<(cctk::wayland_client::EventQueue<State>, State), MoveError> {
    let conn = Connection::connect_to_env().map_err(no_wayland)?;
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(no_wayland)?;
    let qh = queue.handle();
    let registry = RegistryState::new(&globals);
    let info = ToplevelInfoState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel info"))?;
    let manager = ToplevelManagerState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel management"))?;
    let workspaces = WorkspaceState::new(&registry, &qh);
    let outputs = OutputState::new(&globals, &qh);
    let seats = SeatState::new(&globals, &qh);
    let state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        seats,
        can_move: false,
        can_activate: false,
    };
    Ok((queue, state))
}

/// Whether the compositor offers what [`move_to_new_workspace`] needs:
/// the window list, window management with `move_to_ext_workspace`
/// (version 4), and workspaces. Only looks at the advertised globals.
pub fn check() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, _queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let list = globals.contents().clone_list();
    let has = |name: &str, version: u32| {
        list.iter()
            .any(|g| g.interface == name && g.version >= version)
    };
    for (name, version) in [
        ("ext_foreign_toplevel_list_v1", 1),
        ("zcosmic_toplevel_info_v1", 2),
        ("zcosmic_toplevel_manager_v1", 4),
        ("ext_workspace_manager_v1", 1),
    ] {
        if !has(name, version) {
            return Err(format!("{name} v{version} not offered"));
        }
    }
    Ok(())
}

/// Waits up to `wait` for the window with `app_id` and `title` and shows it
/// on an empty workspace. `cancel` stops the wait (the session ended).
///
/// Switches to the empty workspace first, so the window opens there:
/// FreeRDP's X11 client maps its window on the current workspace and only
/// then makes it full screen (FreeRDP 3.32.2 `client/X11/xf_client.c`,
/// `xf_create_window`), so moving it afterwards showed it there for a
/// moment (owner, 2026-10-21, most visible with an Azure desktop). The
/// sign-in page opens there too. If the window opens elsewhere anyway (the
/// user switched meanwhile), it is moved as before.
pub fn move_to_new_workspace(
    app_id: &str,
    title: &str,
    wait: Duration,
    cancel: &Arc<AtomicBool>,
) -> Result<Placed, MoveError> {
    let (mut queue, mut state) = connect()?;
    if state.manager.manager.version() < 4 {
        return Err(MoveError::Unsupported("move_to_ext_workspace (v4)"));
    }
    let roundtrip = |queue: &mut cctk::wayland_client::EventQueue<State>, state: &mut State| {
        queue.roundtrip(state).map(|_| ()).map_err(no_wayland)
    };
    roundtrip(&mut queue, &mut state)?;
    roundtrip(&mut queue, &mut state)?;
    if !state.can_move {
        return Err(MoveError::Unsupported("move capability"));
    }
    // Which workspaces are empty, and which screen is in use, come from
    // the window list.
    let started = Instant::now();
    settle(&mut queue, &mut state)?;
    crate::app_log!(
        "open on a new workspace: {} windows known after {} ms",
        state.info.toplevels().count(),
        started.elapsed().as_millis()
    );

    // Not fatal: without it the window is moved once it is there.
    let switched = match switch_first(&mut queue, &mut state) {
        Ok(s) => Some(s),
        Err(e) => {
            crate::app_log!("open on a new workspace: no switch first: {e}");
            None
        }
    };

    // Wait for the window, with its workspace known.
    let deadline = Instant::now() + wait;
    let found = loop {
        if cancel.load(Ordering::SeqCst) {
            break Err(MoveError::Cancelled);
        }
        if let Some(w) = state.info.toplevels().find(|t| {
            t.app_id == app_id
                && t.title == title
                && t.cosmic_toplevel.is_some()
                && !t.workspace.is_empty()
        }) {
            break Ok(w.clone());
        }
        if Instant::now() > deadline {
            break Err(MoveError::WindowNotFound);
        }
        std::thread::sleep(POLL);
        roundtrip(&mut queue, &mut state)?;
    };
    let window = match found {
        Ok(w) => w,
        Err(e) => {
            if let Some(s) = &switched {
                switch_back(&mut queue, &mut state, s)?;
            }
            return Err(e);
        }
    };
    let toplevel = window
        .cosmic_toplevel
        .clone()
        .ok_or(MoveError::WindowNotFound)?;
    if let Some(s) = switched
        && window.workspace.contains(&s.target)
    {
        return show_moved(&mut queue, &mut state, &toplevel, &s.target, app_id, title)
            .map(|()| Placed::OpenedThere);
    }

    let group = group_of(&state, &window).ok_or(MoveError::NoWorkspace)?;
    let target = empty_or_new(&mut queue, &mut state, &group)?;
    let output = window
        .output
        .iter()
        .next()
        .cloned()
        .or(group.outputs.first().cloned())
        .ok_or(MoveError::NoWorkspace)?;
    state
        .manager
        .manager
        .move_to_ext_workspace(&toplevel, &target, &output);
    show_moved(&mut queue, &mut state, &toplevel, &target, app_id, title).map(|()| Placed::Moved)
}

/// Reads from COSMIC until its window list has arrived.
///
/// cosmic-comp 1.11 (8cb1802, installed 2026-10-27) sends the window list
/// 65–130 ms after connecting, not in reply to the first round trips;
/// 1.10 (41497b4) sent it straight away. A look before it arrives sees no
/// windows, so every workspace seemed empty and the one in use was chosen:
/// no switch, and the desktop opened where the user was (owner,
/// 2026-10-27; measured on the live session with a read-only probe).
///
/// No event says the list is complete, so it is taken as complete when
/// [`settled`] says so. With no windows at all (started from the tray with
/// nothing open) that never happens, and the empty list is used after
/// [`SETTLE_WAIT`].
fn settle(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
) -> Result<(), MoveError> {
    let until = Instant::now() + SETTLE_WAIT;
    let mut last = None;
    loop {
        queue.roundtrip(state).map_err(no_wayland)?;
        let now: Vec<Seen> = state
            .info
            .toplevels()
            .map(|t| Seen {
                app_id: t.app_id.clone(),
                title: t.title.clone(),
                workspaces: t.workspace.len(),
                managed: t.cosmic_toplevel.is_some(),
            })
            .collect();
        if settled(last.as_deref(), &now) || Instant::now() > until {
            return Ok(());
        }
        last = Some(now);
        std::thread::sleep(SETTLE_POLL);
    }
}

/// A window as [`settled`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    app_id: String,
    title: String,
    /// How many workspaces it is on; 0 until COSMIC has said.
    workspaces: usize,
    /// COSMIC's handle for it has arrived.
    managed: bool,
}

/// The window list is complete: it has windows, each with its workspace and
/// COSMIC's handle, and it didn't change since the previous look (the
/// windows arrive together, but a late one would change it).
fn settled(previous: Option<&[Seen]>, now: &[Seen]) -> bool {
    !now.is_empty() && now.iter().all(|w| w.workspaces > 0 && w.managed) && previous == Some(now)
}

/// The empty workspace the window should open on, shown before it opens.
struct Switched {
    target: ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    /// The workspace it replaced; `None` when the one in use was empty
    /// already and is the target itself.
    previous: Option<ext_workspace_handle_v1::ExtWorkspaceHandleV1>,
}

/// Switches the screen in use to an empty workspace (none needed when the
/// one in use is empty).
fn switch_first(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
) -> Result<Switched, MoveError> {
    let group = current_group(state).ok_or(MoveError::NoWorkspace)?;
    let previous = active_in(state, &group).ok_or(MoveError::NoWorkspace)?;
    let target = empty_or_new(queue, state, &group)?;
    if target == previous {
        return Ok(Switched {
            target,
            previous: None,
        });
    }
    target.activate();
    if let Ok(manager) = state.workspaces.workspace_manager().get() {
        manager.commit();
    }
    if !wait_active(queue, state, &target)? {
        return Err(MoveError::NotSwitched);
    }
    Ok(Switched {
        target,
        previous: Some(previous),
    })
}

/// No window came: back to the workspace the user was on, unless they
/// have moved on or put something on the new one meanwhile.
fn switch_back(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
    switched: &Switched,
) -> Result<(), MoveError> {
    let Some(previous) = &switched.previous else {
        return Ok(());
    };
    queue.roundtrip(state).map_err(no_wayland)?;
    let still_active = state.workspaces.workspaces().any(|w| {
        w.handle == switched.target && w.state.contains(ext_workspace_handle_v1::State::Active)
    });
    let in_use = state
        .info
        .toplevels()
        .any(|t| t.workspace.contains(&switched.target));
    let previous_there = state.workspaces.workspaces().any(|w| w.handle == *previous);
    if still_active && !in_use && previous_there {
        previous.activate();
        if let Ok(manager) = state.workspaces.workspace_manager().get() {
            manager.commit();
        }
        queue.roundtrip(state).map_err(no_wayland)?;
    }
    Ok(())
}

/// An empty workspace in `group`, or a new one if the group allows it.
fn empty_or_new(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
    group: &cctk::workspace::WorkspaceGroup,
) -> Result<ext_workspace_handle_v1::ExtWorkspaceHandleV1, MoveError> {
    if let Some(found) = empty_workspace(state, group) {
        return Ok(found);
    }
    // None free: ask COSMIC for one, if the group allows it.
    if !group
        .capabilities
        .contains(ext_workspace_group_handle_v1::GroupCapabilities::CreateWorkspace)
    {
        return Err(MoveError::NoWorkspace);
    }
    group.handle.create_workspace(String::new());
    if let Ok(manager) = state.workspaces.workspace_manager().get() {
        manager.commit();
    }
    let until = Instant::now() + CREATE_WAIT;
    loop {
        queue.roundtrip(state).map_err(no_wayland)?;
        // The group's list now includes the new workspace.
        let group = state
            .workspaces
            .workspace_groups()
            .find(|g| g.handle == group.handle)
            .cloned()
            .ok_or(MoveError::NoWorkspace)?;
        if let Some(found) = empty_workspace(state, &group) {
            return Ok(found);
        }
        if Instant::now() > until {
            return Err(MoveError::NoWorkspace);
        }
        std::thread::sleep(POLL);
    }
}

/// Shows the window just moved to `target`: see the comment inside.
fn show_moved(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
    toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    target: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
    app_id: &str,
    title: &str,
) -> Result<(), MoveError> {
    // COSMIC moves the window but doesn't follow it (owner, 2026-10-21,
    // with a saved connection's window: it was moved, the view stayed).
    // So once COSMIC reports it there: activate the window (it also gets
    // the keyboard, as with Show), and if that doesn't switch, the
    // workspace itself. Done when the new workspace is the active one.
    let until = Instant::now() + CREATE_WAIT;
    loop {
        queue.roundtrip(state).map_err(no_wayland)?;
        let there = state
            .info
            .toplevels()
            .any(|t| t.app_id == app_id && t.title == title && t.workspace.contains(target));
        if there {
            break;
        }
        if Instant::now() > until {
            return Err(MoveError::MovedNotShown);
        }
        std::thread::sleep(POLL);
    }
    // Its own confirmation only says the window is focused, which it may
    // already have been; the workspace's state below is the check.
    let _ = activate(queue, state, toplevel, app_id, title);
    if wait_active(queue, state, target)? {
        return Ok(());
    }
    target.activate();
    if let Ok(manager) = state.workspaces.workspace_manager().get() {
        manager.commit();
    }
    if wait_active(queue, state, target)? {
        return Ok(());
    }
    Err(MoveError::MovedNotShown)
}

/// Waits up to a second for `workspace` to be the active one.
fn wait_active(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
    workspace: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
) -> Result<bool, MoveError> {
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        queue.roundtrip(state).map_err(no_wayland)?;
        let active = state.workspaces.workspaces().any(|w| {
            w.handle == *workspace && w.state.contains(ext_workspace_handle_v1::State::Active)
        });
        if active {
            return Ok(true);
        }
        if Instant::now() > until {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
}

/// Brings the window with `app_id` and `title` to the front (COSMIC
/// switches to its workspace). Blocking; call from `spawn_blocking`.
///
/// Connects, finds the window, asks COSMIC to activate it, then waits up
/// to a second for COSMIC to report it activated.
pub fn focus(app_id: &str, title: &str) -> Result<(), MoveError> {
    let (mut queue, mut state) = connect()?;
    for _ in 0..2 {
        queue.roundtrip(&mut state).map_err(no_wayland)?;
    }
    // The window list arrives over several round trips.
    let until = Instant::now() + Duration::from_secs(2);
    let toplevel = loop {
        if let Some(t) = state
            .info
            .toplevels()
            .find(|t| t.app_id == app_id && t.title == title)
            .and_then(|t| t.cosmic_toplevel.clone())
        {
            break t;
        }
        if Instant::now() > until {
            #[cfg(test)]
            eprintln!(
                "windows: {:?}",
                state
                    .info
                    .toplevels()
                    .map(|t| t.app_id.clone())
                    .collect::<Vec<_>>()
            );
            return Err(MoveError::WindowNotFound);
        }
        std::thread::sleep(POLL);
        queue.roundtrip(&mut state).map_err(no_wayland)?;
    };
    activate(&mut queue, &mut state, &toplevel, app_id, title)
}

/// Asks COSMIC to activate `toplevel` (it switches to the window's
/// workspace) and waits up to a second for it to report the window
/// activated.
fn activate(
    queue: &mut cctk::wayland_client::EventQueue<State>,
    state: &mut State,
    toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    app_id: &str,
    title: &str,
) -> Result<(), MoveError> {
    if !state.can_activate {
        return Err(MoveError::Unsupported("activate capability"));
    }
    let seat = state
        .seats
        .seats()
        .next()
        .ok_or(MoveError::Unsupported("seat"))?;
    state.manager.manager.activate(toplevel, &seat);
    // Confirm it: COSMIC reports the window as activated.
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        queue.roundtrip(state).map_err(no_wayland)?;
        let active = state.info.toplevels().any(|t| {
            t.app_id == app_id
                && t.title == title
                && t.state
                    .contains(&zcosmic_toplevel_handle_v1::State::Activated)
        });
        if active {
            return Ok(());
        }
        if Instant::now() > until {
            return Err(MoveError::NotActivated);
        }
        std::thread::sleep(POLL);
    }
}

/// Watches whether one of the windows with `app_id` and a title in
/// `titles` is both focused and full screen. Calls `changed` with each new
/// answer (first with the current one) and stops once `wanted` says no.
/// Blocking, with its own Wayland connection; run it on its own thread.
pub fn watch_fullscreen_focus(
    app_id: &str,
    titles: &[String],
    wanted: impl Fn() -> bool,
    mut changed: impl FnMut(bool),
) -> Result<(), MoveError> {
    use zcosmic_toplevel_handle_v1::State as S;
    let (mut queue, mut state) = connect()?;
    let mut last = None;
    while wanted() {
        queue.roundtrip(&mut state).map_err(no_wayland)?;
        let now = state.info.toplevels().any(|t| {
            t.app_id == app_id
                && titles.contains(&t.title)
                && t.state.contains(&S::Activated)
                && t.state.contains(&S::Fullscreen)
        });
        if last != Some(now) {
            last = Some(now);
            changed(now);
        }
        std::thread::sleep(WATCH_POLL);
    }
    Ok(())
}

/// The workspace group holding the window's workspace.
fn group_of(state: &State, window: &ToplevelInfo) -> Option<cctk::workspace::WorkspaceGroup> {
    state
        .workspaces
        .workspace_groups()
        .find(|g| g.workspaces.iter().any(|w| window.workspace.contains(w)))
        .cloned()
}

/// The group of the screen in use: the focused window's (usually
/// Deskfeed's, just clicked), else the first with an active workspace.
fn current_group(state: &State) -> Option<cctk::workspace::WorkspaceGroup> {
    state
        .info
        .toplevels()
        .find(|t| {
            t.state
                .contains(&zcosmic_toplevel_handle_v1::State::Activated)
        })
        .and_then(|t| group_of(state, t))
        .or_else(|| {
            state
                .workspaces
                .workspace_groups()
                .find(|g| active_in(state, g).is_some())
                .cloned()
        })
}

/// The group's active workspace.
fn active_in(
    state: &State,
    group: &cctk::workspace::WorkspaceGroup,
) -> Option<ext_workspace_handle_v1::ExtWorkspaceHandleV1> {
    state
        .workspaces
        .workspaces()
        .find(|w| {
            group.workspaces.contains(&w.handle)
                && w.state.contains(ext_workspace_handle_v1::State::Active)
        })
        .map(|w| w.handle.clone())
}

/// An empty workspace in `group`: see [`pick_empty`].
fn empty_workspace(
    state: &State,
    group: &cctk::workspace::WorkspaceGroup,
) -> Option<ext_workspace_handle_v1::ExtWorkspaceHandleV1> {
    let occupied: Vec<_> = state
        .info
        .toplevels()
        .flat_map(|t| t.workspace.iter().cloned())
        .collect();
    let candidates: Vec<_> = state
        .workspaces
        .workspaces()
        .filter(|w| group.workspaces.contains(&w.handle))
        .map(|w| Candidate {
            handle: w.handle.clone(),
            active: w.state.contains(ext_workspace_handle_v1::State::Active),
            empty: !occupied.contains(&w.handle),
            position: w.coordinates.clone(),
        })
        .collect();
    pick_empty(candidates)
}

/// A workspace as [`pick_empty`] sees it.
struct Candidate<H> {
    handle: H,
    active: bool,
    /// No window on it, minimized ones included.
    empty: bool,
    position: Vec<u32>,
}

/// The active workspace if it is empty (no need to switch), otherwise the
/// first empty one by position.
fn pick_empty<H>(workspaces: Vec<Candidate<H>>) -> Option<H> {
    workspaces
        .into_iter()
        .filter(|w| w.empty)
        .min_by(|a, b| b.active.cmp(&a.active).then(a.position.cmp(&b.position)))
        .map(|w| w.handle)
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers!(OutputState);
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ToplevelInfoHandler for State {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.info
    }
    fn new_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
    }
    fn update_toplevel(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
    }
    fn toplevel_closed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
    }
}

impl ToplevelManagerHandler for State {
    fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
        &mut self.manager
    }
    fn capabilities(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        capabilities: Vec<
            WEnum<zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>,
        >,
    ) {
        // cosmic-comp (at 41497b4) handles `move_to_ext_workspace` but
        // advertises only the older `move_to_workspace` capability, whose
        // request it ignores (`src/state.rs`,
        // `src/wayland/protocols/toplevel_management.rs`). The request
        // exists from version 4 on, which `move_to_new_workspace` checks.
        use zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1 as C;
        self.can_move = capabilities.contains(&WEnum::Value(C::MoveToExtWorkspace))
            || capabilities.contains(&WEnum::Value(C::MoveToWorkspace));
        self.can_activate = capabilities.contains(&WEnum::Value(C::Activate));
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl WorkspaceHandler for State {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspaces
    }
    fn done(&mut self) {}
}

sctk::delegate_output!(State);
sctk::delegate_registry!(State);
sctk::delegate_seat!(State);
cctk::delegate_toplevel_info!(State);
cctk::delegate_toplevel_manager!(State);
cctk::delegate_workspace!(State);

#[cfg(test)]
mod tests {
    use super::{Candidate, Seen, pick_empty, settled};

    fn seen(title: &str, workspaces: usize, managed: bool) -> Seen {
        Seen {
            app_id: "app".into(),
            title: title.into(),
            workspaces,
            managed,
        }
    }

    #[test]
    fn no_windows_is_never_settled() {
        // Either the list hasn't arrived or there are none: only the time
        // limit tells, so it is waited out.
        assert!(!settled(None, &[]));
        assert!(!settled(Some(&[]), &[]));
    }

    #[test]
    fn the_first_look_is_not_enough() {
        assert!(!settled(None, &[seen("a", 1, true)]));
    }

    #[test]
    fn the_same_complete_list_twice_is_settled() {
        let list = vec![seen("a", 1, true), seen("b", 2, true)];
        assert!(settled(Some(&list), &list));
    }

    #[test]
    fn a_window_still_without_workspace_or_handle_waits() {
        let missing_workspace = vec![seen("a", 1, true), seen("b", 0, true)];
        assert!(!settled(Some(&missing_workspace), &missing_workspace));
        let missing_handle = vec![seen("a", 1, false)];
        assert!(!settled(Some(&missing_handle), &missing_handle));
    }

    #[test]
    fn a_list_that_still_changes_waits() {
        let before = vec![seen("a", 1, true)];
        let after = vec![seen("a", 1, true), seen("b", 1, true)];
        assert!(!settled(Some(&before), &after));
        // So does a window's workspaces changing.
        assert!(!settled(Some(&[seen("a", 1, true)]), &[seen("a", 2, true)]));
    }

    fn ws(handle: u32, active: bool, empty: bool, position: u32) -> Candidate<u32> {
        Candidate {
            handle,
            active,
            empty,
            position: vec![position],
        }
    }

    #[test]
    fn an_empty_active_workspace_needs_no_switch() {
        let all = vec![
            ws(1, false, true, 0),
            ws(2, true, true, 1),
            ws(3, false, true, 2),
        ];
        assert_eq!(pick_empty(all), Some(2));
    }

    #[test]
    fn otherwise_the_first_empty_one_by_position() {
        // Listed out of order: position decides, not the order COSMIC sent.
        let all = vec![
            ws(3, false, true, 2),
            ws(1, true, false, 0),
            ws(2, false, true, 1),
        ];
        assert_eq!(pick_empty(all), Some(2));
    }

    #[test]
    fn no_empty_workspace_gives_none() {
        let all = vec![ws(1, true, false, 0), ws(2, false, false, 1)];
        assert_eq!(pick_empty(all), None);
        assert_eq!(pick_empty(Vec::<Candidate<u32>>::new()), None);
    }

    /// Prints which workspace "Open on a new workspace" would choose on the
    /// running session, without switching: `cargo test --release
    /// choose_on_the_real_session -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the running COSMIC session's windows and workspaces"]
    fn choose_on_the_real_session() {
        use super::*;
        let (mut queue, mut state) = connect().unwrap();
        for _ in 0..2 {
            queue.roundtrip(&mut state).unwrap();
        }
        let started = Instant::now();
        settle(&mut queue, &mut state).unwrap();
        println!(
            "{} windows after {} ms",
            state.info.toplevels().count(),
            started.elapsed().as_millis()
        );
        let group = current_group(&state).expect("a workspace group");
        let name = |h: &ext_workspace_handle_v1::ExtWorkspaceHandleV1| {
            state
                .workspaces
                .workspaces()
                .find(|w| w.handle == *h)
                .map(|w| w.name.clone())
        };
        let active = active_in(&state, &group);
        let chosen = empty_workspace(&state, &group);
        println!(
            "in use: {:?}, chosen: {:?}",
            active.as_ref().and_then(name),
            chosen.as_ref().and_then(name)
        );
    }

    /// Moves a real window: `MOVE_APP_ID=… MOVE_TITLE=… cargo test --release
    /// move_a_real_window -- --ignored` (for example a session from
    /// `local_test_server_session`). Passes only once COSMIC shows the new
    /// workspace, not only when the window has moved.
    #[test]
    #[ignore = "moves a real window on the running COSMIC session"]
    fn move_a_real_window() {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let placed = super::move_to_new_workspace(
            &std::env::var("MOVE_APP_ID").unwrap(),
            &std::env::var("MOVE_TITLE").unwrap(),
            std::time::Duration::from_secs(10),
            &cancel,
        );
        println!("{placed:?}");
        placed.unwrap();
    }

    /// Focuses a real window: `MOVE_APP_ID=… MOVE_TITLE=… cargo test
    /// --release focus_a_real_window -- --ignored`.
    /// Run it while another window has the focus (for example a second
    /// session opened later); `focus` only succeeds once COSMIC reports
    /// the window activated.
    #[test]
    #[ignore = "focuses a real window on the running COSMIC session"]
    fn focus_a_real_window() {
        super::focus(
            &std::env::var("MOVE_APP_ID").unwrap(),
            &std::env::var("MOVE_TITLE").unwrap(),
        )
        .unwrap();
    }

    /// Asks COSMIC to close a real window, as its close button does:
    /// `MOVE_APP_ID=… cargo test --release close_a_real_window -- --ignored`.
    #[test]
    #[ignore = "closes a real window on the running COSMIC session"]
    fn close_a_real_window() {
        use super::*;
        let app_id = std::env::var("MOVE_APP_ID").unwrap();
        let (mut queue, mut state) = connect().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let toplevel = loop {
            queue.roundtrip(&mut state).unwrap();
            if let Some(t) = state
                .info
                .toplevels()
                .find(|t| t.app_id == app_id)
                .and_then(|t| t.cosmic_toplevel.clone())
            {
                break t;
            }
            assert!(Instant::now() < until, "window not found");
            std::thread::sleep(POLL);
        };
        state.manager.manager.close(&toplevel);
        queue.roundtrip(&mut state).unwrap();
    }

    /// Prints `WATCH_APP_ID`'s windows (title and state) whenever they
    /// change, for `WATCH_SECS` seconds.
    #[test]
    #[ignore = "watches the running COSMIC session's windows"]
    fn watch_windows() {
        use super::*;
        let app_id = std::env::var("WATCH_APP_ID").unwrap();
        let secs: u64 = std::env::var("WATCH_SECS").map_or(12, |s| s.parse().unwrap());
        let (mut queue, mut state) = connect().unwrap();
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < Duration::from_secs(secs) {
            queue.roundtrip(&mut state).unwrap();
            let windows: Vec<String> = state
                .info
                .toplevels()
                .filter(|t| t.app_id == app_id)
                .map(|t| format!("{:?} {:?}", t.title, t.state))
                .collect();
            if last.as_ref() != Some(&windows) {
                println!(
                    "{:>5.1}s: {} window(s)",
                    start.elapsed().as_secs_f32(),
                    windows.len()
                );
                for w in &windows {
                    println!("        {w}");
                }
                last = Some(windows);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
