// SPDX-License-Identifier: MIT

//! "Open on a new workspace": moves a FreeRDP window to an empty COSMIC
//! workspace (ADR 0011). Also brings a session's window to the front
//! ([`focus`], `activate` in `cosmic-toplevel-management`).
//!
//! FreeRDP is started with a known app ID (`/wm-class:`) and window title
//! (`/title:`); this module waits for that window with COSMIC's window
//! information protocols (`ext-foreign-toplevel-list` with
//! `cosmic-toplevel-info`), picks an empty workspace in the window's
//! workspace group (or asks COSMIC for a new one), and moves the window
//! there with `cosmic-toplevel-management` (`move_to_ext_workspace`,
//! version 4). cosmic-comp offers these protocols only to unsandboxed
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
        }
    }
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

/// Waits up to `wait` for the window with `app_id` and `title`, then moves
/// it to an empty workspace. `cancel` stops the wait (the session ended).
pub fn move_to_new_workspace(
    app_id: &str,
    title: &str,
    wait: Duration,
    cancel: &Arc<AtomicBool>,
) -> Result<(), MoveError> {
    let conn = Connection::connect_to_env().map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let registry = RegistryState::new(&globals);
    let info = ToplevelInfoState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel info"))?;
    let manager = ToplevelManagerState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel management"))?;
    if manager.manager.version() < 4 {
        return Err(MoveError::Unsupported("move_to_ext_workspace (v4)"));
    }
    let workspaces = WorkspaceState::new(&registry, &qh);
    let outputs = OutputState::new(&globals, &qh);
    let seats = SeatState::new(&globals, &qh);
    let mut state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        seats,
        can_move: false,
        can_activate: false,
    };
    let roundtrip = |queue: &mut cctk::wayland_client::EventQueue<State>, state: &mut State| {
        queue
            .roundtrip(state)
            .map(|_| ())
            .map_err(|e| MoveError::NoWayland(e.to_string()))
    };
    roundtrip(&mut queue, &mut state)?;
    roundtrip(&mut queue, &mut state)?;
    if !state.can_move {
        return Err(MoveError::Unsupported("move capability"));
    }

    // Wait for the window, with its workspace known.
    let deadline = Instant::now() + wait;
    let window = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(MoveError::Cancelled);
        }
        if let Some(w) = state.info.toplevels().find(|t| {
            t.app_id == app_id
                && t.title == title
                && t.cosmic_toplevel.is_some()
                && !t.workspace.is_empty()
        }) {
            break w.clone();
        }
        if Instant::now() > deadline {
            return Err(MoveError::WindowNotFound);
        }
        std::thread::sleep(POLL);
        roundtrip(&mut queue, &mut state)?;
    };

    let (group, target) = if let Some(found) = empty_workspace(&state, &window) {
        found
    } else {
        {
            // None free: ask COSMIC for one, if the group allows it.
            let group = group_of(&state, &window).ok_or(MoveError::NoWorkspace)?;
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
                roundtrip(&mut queue, &mut state)?;
                if let Some(found) = empty_workspace(&state, &window) {
                    break found;
                }
                if Instant::now() > until {
                    return Err(MoveError::NoWorkspace);
                }
                std::thread::sleep(POLL);
            }
        }
    };
    let output = window
        .output
        .iter()
        .next()
        .cloned()
        .or(group.outputs.first().cloned())
        .ok_or(MoveError::NoWorkspace)?;
    let toplevel = window
        .cosmic_toplevel
        .as_ref()
        .ok_or(MoveError::WindowNotFound)?;
    state
        .manager
        .manager
        .move_to_ext_workspace(toplevel, &target, &output);
    roundtrip(&mut queue, &mut state)?;
    Ok(())
}

/// Brings the window with `app_id` and `title` to the front (COSMIC
/// switches to its workspace). Blocking; call from `spawn_blocking`.
pub fn focus(app_id: &str, title: &str) -> Result<(), MoveError> {
    with_window(app_id, title, |state, toplevel, seat| {
        state.manager.manager.activate(toplevel, seat);
    })?;
    Ok(())
}

/// Connects, finds the window, runs `act` on it, then waits up to a
/// second for it to be activated.
fn with_window(
    app_id: &str,
    title: &str,
    act: impl FnOnce(
        &mut State,
        &cctk::cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        &wl_seat::WlSeat,
    ),
) -> Result<(), MoveError> {
    let conn = Connection::connect_to_env().map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let registry = RegistryState::new(&globals);
    let info = ToplevelInfoState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel info"))?;
    let manager = ToplevelManagerState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel management"))?;
    let workspaces = WorkspaceState::new(&registry, &qh);
    let outputs = OutputState::new(&globals, &qh);
    let seats = SeatState::new(&globals, &qh);
    let mut state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        seats,
        can_move: false,
        can_activate: false,
    };
    for _ in 0..2 {
        queue
            .roundtrip(&mut state)
            .map_err(|e| MoveError::NoWayland(e.to_string()))?;
    }
    if !state.can_activate {
        return Err(MoveError::Unsupported("activate capability"));
    }
    let seat = state
        .seats
        .seats()
        .next()
        .ok_or(MoveError::Unsupported("seat"))?;
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
        queue
            .roundtrip(&mut state)
            .map_err(|e| MoveError::NoWayland(e.to_string()))?;
    };
    act(&mut state, &toplevel, &seat);
    // Confirm it: COSMIC reports the window as activated.
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        queue
            .roundtrip(&mut state)
            .map_err(|e| MoveError::NoWayland(e.to_string()))?;
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
    let conn = Connection::connect_to_env().map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| MoveError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let registry = RegistryState::new(&globals);
    let info = ToplevelInfoState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel info"))?;
    let manager = ToplevelManagerState::try_new(&registry, &qh)
        .ok_or(MoveError::Unsupported("toplevel management"))?;
    let workspaces = WorkspaceState::new(&registry, &qh);
    let outputs = OutputState::new(&globals, &qh);
    let seats = SeatState::new(&globals, &qh);
    let mut state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        seats,
        can_move: false,
        can_activate: false,
    };
    let mut last = None;
    while wanted() {
        queue
            .roundtrip(&mut state)
            .map_err(|e| MoveError::NoWayland(e.to_string()))?;
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

/// An empty workspace in the window's group (the first by position), and
/// that group.
fn empty_workspace(
    state: &State,
    window: &ToplevelInfo,
) -> Option<(
    cctk::workspace::WorkspaceGroup,
    ext_workspace_handle_v1::ExtWorkspaceHandleV1,
)> {
    let group = group_of(state, window)?;
    let occupied: Vec<_> = state
        .info
        .toplevels()
        .flat_map(|t| t.workspace.iter().cloned())
        .collect();
    let target = state
        .workspaces
        .workspaces()
        .filter(|w| group.workspaces.contains(&w.handle) && !occupied.contains(&w.handle))
        .min_by(|a, b| a.coordinates.cmp(&b.coordinates))?
        .handle
        .clone();
    Some((group, target))
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
    /// Moves a real window: `MOVE_APP_ID=… MOVE_TITLE=… cargo test --release
    /// move_a_real_window -- --ignored` (for example a session from
    /// `local_test_server_session`).
    #[test]
    #[ignore = "moves a real window on the running COSMIC session"]
    fn move_a_real_window() {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        super::move_to_new_workspace(
            &std::env::var("MOVE_APP_ID").unwrap(),
            &std::env::var("MOVE_TITLE").unwrap(),
            std::time::Duration::from_secs(10),
            &cancel,
        )
        .unwrap();
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
        let conn = Connection::connect_to_env().unwrap();
        let (globals, mut queue) = registry_queue_init::<State>(&conn).unwrap();
        let qh = queue.handle();
        let registry = RegistryState::new(&globals);
        let info = ToplevelInfoState::try_new(&registry, &qh).unwrap();
        let manager = ToplevelManagerState::try_new(&registry, &qh).unwrap();
        let workspaces = WorkspaceState::new(&registry, &qh);
        let outputs = OutputState::new(&globals, &qh);
        let seats = SeatState::new(&globals, &qh);
        let mut state = State {
            registry,
            outputs,
            info,
            manager,
            workspaces,
            seats,
            can_move: false,
            can_activate: false,
        };
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

    /// Prints how many windows `WATCH_APP_ID` has, every half second for
    /// `WATCH_SECS` seconds.
    #[test]
    #[ignore = "watches the running COSMIC session's windows"]
    fn watch_windows() {
        use super::*;
        let app_id = std::env::var("WATCH_APP_ID").unwrap();
        let secs: u64 = std::env::var("WATCH_SECS").map_or(12, |s| s.parse().unwrap());
        let conn = Connection::connect_to_env().unwrap();
        let (globals, mut queue) = registry_queue_init::<State>(&conn).unwrap();
        let qh = queue.handle();
        let registry = RegistryState::new(&globals);
        let info = ToplevelInfoState::try_new(&registry, &qh).unwrap();
        let manager = ToplevelManagerState::try_new(&registry, &qh).unwrap();
        let workspaces = WorkspaceState::new(&registry, &qh);
        let outputs = OutputState::new(&globals, &qh);
        let seats = SeatState::new(&globals, &qh);
        let mut state = State {
            registry,
            outputs,
            info,
            manager,
            workspaces,
            seats,
            can_move: false,
            can_activate: false,
        };
        let start = Instant::now();
        let mut last = usize::MAX;
        while start.elapsed() < Duration::from_secs(secs) {
            queue.roundtrip(&mut state).unwrap();
            let n = state
                .info
                .toplevels()
                .filter(|t| t.app_id == app_id)
                .count();
            if n != last {
                println!("{:>5.1}s: {n} window(s)", start.elapsed().as_secs_f32());
                last = n;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
