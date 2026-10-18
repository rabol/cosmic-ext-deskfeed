// SPDX-License-Identifier: MIT

//! "Open on a new workspace": moves a FreeRDP window to an empty COSMIC
//! workspace (ADR 0011).
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
//! [`refit_window`] works around a FreeRDP 3.32.2 behaviour: in a window,
//! the SDL3 client sends its size to Windows only on a window event after
//! the display channel is ready (`client/SDL/SDL3/sdl_disp.cpp`:
//! `OnActivated` returns early on the first activation, and
//! `DisplayControlCaps` only makes the window resizable). Under COSMIC
//! auto-tiling the window gets its tile size before that, so the remote
//! desktop kept its first size. Maximizing and restoring the window once
//! the channel is ready gives FreeRDP those events; the app repeats it if
//! the window didn't take the new size (see [`refit_window`]).
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
    },
    toplevel_info::{ToplevelInfo, ToplevelInfoHandler, ToplevelInfoState},
    toplevel_management::{ToplevelManagerHandler, ToplevelManagerState},
    wayland_client::{
        Connection, Proxy, QueueHandle, WEnum, globals::registry_queue_init, protocol::wl_output,
    },
    wayland_protocols::ext::{
        foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1,
        workspace::v1::client::{ext_workspace_group_handle_v1, ext_workspace_handle_v1},
    },
    workspace::{WorkspaceHandler, WorkspaceState},
};

const POLL: Duration = Duration::from_millis(200);
/// How long to wait for COSMIC to create a requested workspace.
const CREATE_WAIT: Duration = Duration::from_secs(3);
/// How long to wait for COSMIC to report the window maximized.
const REFIT_WAIT: Duration = Duration::from_secs(2);

struct State {
    registry: RegistryState,
    outputs: OutputState,
    info: ToplevelInfoState,
    manager: ToplevelManagerState,
    workspaces: WorkspaceState,
    capabilities: Vec<WEnum<Capability>>,
}

type Capability = zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1;

/// Why the window wasn't moved; for the log only.
#[derive(Debug)]
pub enum MoveError {
    NoWayland(String),
    /// COSMIC's window management isn't offered (not COSMIC, or too old).
    Unsupported(&'static str),
    WindowNotFound,
    NoWorkspace,
    Cancelled,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWayland(e) => write!(f, "no Wayland connection: {e}"),
            Self::Unsupported(what) => write!(f, "compositor doesn't offer {what}"),
            Self::WindowNotFound => write!(f, "window not found in time"),
            Self::NoWorkspace => write!(f, "no empty workspace"),
            Self::Cancelled => write!(f, "session ended first"),
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
    let mut state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        capabilities: Vec::new(),
    };
    let roundtrip = |queue: &mut cctk::wayland_client::EventQueue<State>, state: &mut State| {
        queue
            .roundtrip(state)
            .map(|_| ())
            .map_err(|e| MoveError::NoWayland(e.to_string()))
    };
    roundtrip(&mut queue, &mut state)?;
    roundtrip(&mut queue, &mut state)?;
    if !state
        .capabilities
        .contains(&WEnum::Value(Capability::MoveToExtWorkspace))
    {
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

/// What [`refit_window`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refit {
    /// Maximized and restored the window.
    Done,
    /// Left it alone: maximized or full screen already.
    Skipped,
}

/// Maximizes the window with `app_id` and `title`, waits until COSMIC
/// reports it maximized, then restores it, so FreeRDP sends the window's
/// real size (see the module notes). Leaves maximized or full-screen
/// windows alone.
///
/// The window may ignore both sizes: FreeRDP logs that its window is
/// resizable before SDL makes it so (`sdl_context.cpp` `setResizeable`
/// queues the change), and SDL ignores sizes for a fixed-size window
/// (SDL 3.4.18 `SDL_waylandwindow.c`, `handle_xdg_toplevel_configure`).
/// The caller checks that the window took a new size and calls again.
pub fn refit_window(app_id: &str, title: &str) -> Result<Refit, MoveError> {
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
    let mut state = State {
        registry,
        outputs,
        info,
        manager,
        workspaces,
        capabilities: Vec::new(),
    };
    let mut roundtrip = |state: &mut State| {
        queue
            .roundtrip(state)
            .map(|_| ())
            .map_err(|e| MoveError::NoWayland(e.to_string()))
    };
    roundtrip(&mut state)?;
    roundtrip(&mut state)?;
    if !state
        .capabilities
        .contains(&WEnum::Value(Capability::Maximize))
    {
        return Err(MoveError::Unsupported("maximize capability"));
    }
    let find = |state: &State| {
        state
            .info
            .toplevels()
            .find(|t| t.app_id == app_id && t.title == title && t.cosmic_toplevel.is_some())
            .cloned()
    };
    let maximized = |t: &ToplevelInfo| {
        t.state
            .contains(&zcosmic_toplevel_handle_v1::State::Maximized)
    };
    // The window's details arrive in steps; FreeRDP's window exists by
    // now, so this is short.
    let until = Instant::now() + REFIT_WAIT;
    let window = loop {
        if let Some(w) = find(&state) {
            break w;
        }
        if Instant::now() > until {
            return Err(MoveError::WindowNotFound);
        }
        std::thread::sleep(Duration::from_millis(50));
        roundtrip(&mut state)?;
    };
    if maximized(&window) || is_fullscreen(&window) {
        return Ok(Refit::Skipped);
    }
    let toplevel = window.cosmic_toplevel.ok_or(MoveError::WindowNotFound)?;
    state.manager.manager.set_maximized(&toplevel);
    let until = Instant::now() + REFIT_WAIT;
    loop {
        roundtrip(&mut state)?;
        if find(&state).is_some_and(|t| maximized(&t)) {
            break;
        }
        if Instant::now() > until {
            break; // restore anyway, so the window isn't left maximized
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    state.manager.manager.unset_maximized(&toplevel);
    roundtrip(&mut state)?;
    Ok(Refit::Done)
}

fn is_fullscreen(t: &ToplevelInfo) -> bool {
    t.state
        .contains(&zcosmic_toplevel_handle_v1::State::Fullscreen)
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
        capabilities: Vec<WEnum<Capability>>,
    ) {
        self.capabilities = capabilities;
    }
}

impl WorkspaceHandler for State {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspaces
    }
    fn done(&mut self) {}
}

sctk::delegate_output!(State);
sctk::delegate_registry!(State);
cctk::delegate_toplevel_info!(State);
cctk::delegate_toplevel_manager!(State);
cctk::delegate_workspace!(State);

#[cfg(test)]
mod tests {
    /// Against the running COSMIC session: a window with this app ID and
    /// title must be open (for example a small GTK test window).
    #[test]
    #[ignore = "needs a COSMIC session and a test window"]
    fn refit_a_real_window() {
        super::refit_window(
            &std::env::var("REFIT_APP_ID").unwrap(),
            &std::env::var("REFIT_TITLE").unwrap(),
        )
        .unwrap();
    }
}
