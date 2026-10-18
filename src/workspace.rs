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
//! Blocking, with its own short-lived Wayland connection; call it from
//! `spawn_blocking`. Uses cosmic-client-toolkit as re-exported by libcosmic.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cosmic::cctk::{
    self,
    cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1,
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

struct State {
    registry: RegistryState,
    outputs: OutputState,
    info: ToplevelInfoState,
    manager: ToplevelManagerState,
    workspaces: WorkspaceState,
    can_move: bool,
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
        can_move: false,
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
}
