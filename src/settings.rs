// SPDX-License-Identifier: MIT

//! Connection settings (spec §Settings): built-in defaults, overridden by
//! the app's settings, then per account, then per desktop. `None` means
//! "inherit".
//!
//! The app alone decides display: `sessions/rdpfile.rs` drops the feed's
//! `screen mode id` and `use multimon`. Every option here was checked
//! against `xfreerdp3` 3.32.2 `/help` and WinPR's parser: `f` is a flag
//! (only "on" exists), `multimon` and `clipboard` are booleans (`-` turns
//! them off).

use serde::{Deserialize, Serialize};

/// One level of settings; unset fields inherit from the level above.
/// The field names are the keys stored in the config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSettings {
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub fullscreen: Option<bool>,
    #[serde(default)]
    pub all_monitors: Option<bool>,
    #[serde(default)]
    pub new_workspace: Option<bool>,
    #[serde(default)]
    pub sound: Option<bool>,
    #[serde(default)]
    pub microphone: Option<bool>,
    #[serde(default)]
    pub camera: Option<bool>,
    /// Before clipboard directions: on means both directions. Read only
    /// (`Effective::with`); new values go to `clipboard_mode`.
    #[serde(default)]
    pub clipboard: Option<bool>,
    /// `ClipboardMode` as a number (`Choice::Clipboard`).
    #[serde(default)]
    pub clipboard_mode: Option<u32>,
    #[serde(default)]
    pub clipboard_files: Option<bool>,
    /// Super, Alt+Tab and the like go to the desktop (ADR 0018).
    #[serde(default)]
    pub system_shortcuts: Option<bool>,
    /// App and account levels only.
    #[serde(default)]
    pub refresh_mins: Option<u32>,
    /// App and account levels only.
    #[serde(default)]
    pub sign_in_mins: Option<u32>,
    /// App and account levels only.
    #[serde(default)]
    pub offline_days: Option<u32>,
    #[serde(default)]
    pub reconnect: Option<bool>,
    #[serde(default)]
    pub reconnect_attempts: Option<u32>,
    /// Windows' display scaling in percent; `Some(0)` follows the monitor.
    #[serde(default)]
    pub scale: Option<u32>,
}

/// Settings chosen from a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Windows' display scaling (ADR 0015).
    Scale,
    /// Which way copy and paste works (`clipboard_mode` values).
    Clipboard,
}

/// `Choice::Clipboard` values.
pub mod clipboard {
    pub const OFF: u32 = 0;
    pub const BOTH: u32 = 1;
    /// From this computer to the desktop only.
    pub const TO_DESKTOP: u32 = 2;
    /// From the desktop to this computer only.
    pub const TO_COMPUTER: u32 = 3;
}

impl Choice {
    /// The values offered. For `Scale`, 0 is "follow the monitor", the
    /// rest are Windows' own steps.
    pub fn options(self) -> &'static [u32] {
        match self {
            Self::Scale => &[0, 100, 125, 150, 175, 200, 250, 300],
            Self::Clipboard => &[
                clipboard::OFF,
                clipboard::BOTH,
                clipboard::TO_DESKTOP,
                clipboard::TO_COMPUTER,
            ],
        }
    }

    /// A value from the config, if it is one of the options.
    pub fn valid(self, value: u32) -> Option<u32> {
        self.options().contains(&value).then_some(value)
    }
}

/// The on/off settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Fullscreen,
    AllMonitors,
    NewWorkspace,
    Sound,
    Microphone,
    Camera,
    /// Files through the clipboard, in the clipboard's direction.
    ClipboardFiles,
    /// Super, Alt+Tab and the like go to the desktop (ADR 0018).
    SystemShortcuts,
    Reconnect,
}

impl Toggle {
    #[cfg(test)]
    pub const ALL: [Self; 9] = [
        Self::Fullscreen,
        Self::AllMonitors,
        Self::NewWorkspace,
        Self::Sound,
        Self::Microphone,
        Self::Camera,
        Self::ClipboardFiles,
        Self::SystemShortcuts,
        Self::Reconnect,
    ];
}

/// The number settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Number {
    /// Time to wait for the desktop, in seconds.
    Timeout,
    /// How often the desktop lists refresh, in minutes.
    Refresh,
    /// Time allowed for signing in, in minutes.
    SignIn,
    /// How long the offline desktop list is kept, in days.
    OfflineDays,
    /// How many times a lost connection is retried.
    ReconnectAttempts,
}

impl Number {
    #[cfg(test)]
    pub const ALL: [Self; 5] = [
        Self::Timeout,
        Self::ReconnectAttempts,
        Self::Refresh,
        Self::SignIn,
        Self::OfflineDays,
    ];

    /// Smallest value, largest value, step.
    pub fn range(self) -> (u32, u32, u32) {
        match self {
            Self::Timeout => (10, 600, 10),
            // An hour is enough for a list that rarely changes; a day at
            // most, so new assignments still show up.
            Self::Refresh => (15, 24 * 60, 15),
            // The sign-in window takes at most 15 min (`signin/src/protocol.rs`
            // `MAX_TIMEOUT_MS`).
            Self::SignIn => (2, 15, 1),
            // Spec §10: cached metadata has an age limit.
            Self::OfflineDays => (1, 90, 1),
            Self::ReconnectAttempts => (1, 10, 1),
        }
    }

    pub fn clamp(self, value: u32) -> u32 {
        let (min, max, _) = self.range();
        value.clamp(min, max)
    }
}

impl ConnectionSettings {
    pub fn toggle(&self, t: Toggle) -> Option<bool> {
        match t {
            Toggle::Fullscreen => self.fullscreen,
            Toggle::AllMonitors => self.all_monitors,
            Toggle::NewWorkspace => self.new_workspace,
            Toggle::Sound => self.sound,
            Toggle::Microphone => self.microphone,
            Toggle::Camera => self.camera,
            Toggle::ClipboardFiles => self.clipboard_files,
            Toggle::SystemShortcuts => self.system_shortcuts,
            Toggle::Reconnect => self.reconnect,
        }
    }

    pub fn set_toggle(&mut self, t: Toggle, value: Option<bool>) {
        let slot = match t {
            Toggle::Fullscreen => &mut self.fullscreen,
            Toggle::AllMonitors => &mut self.all_monitors,
            Toggle::NewWorkspace => &mut self.new_workspace,
            Toggle::Sound => &mut self.sound,
            Toggle::Microphone => &mut self.microphone,
            Toggle::Camera => &mut self.camera,
            Toggle::ClipboardFiles => &mut self.clipboard_files,
            Toggle::SystemShortcuts => &mut self.system_shortcuts,
            Toggle::Reconnect => &mut self.reconnect,
        };
        *slot = value;
    }

    pub fn number(&self, n: Number) -> Option<u32> {
        match n {
            Number::Timeout => self.timeout_secs,
            Number::Refresh => self.refresh_mins,
            Number::SignIn => self.sign_in_mins,
            Number::OfflineDays => self.offline_days,
            Number::ReconnectAttempts => self.reconnect_attempts,
        }
    }

    /// Sets a number, clamped to its range; `None` inherits.
    pub fn set_number(&mut self, n: Number, value: Option<u32>) {
        let slot = match n {
            Number::Timeout => &mut self.timeout_secs,
            Number::Refresh => &mut self.refresh_mins,
            Number::SignIn => &mut self.sign_in_mins,
            Number::OfflineDays => &mut self.offline_days,
            Number::ReconnectAttempts => &mut self.reconnect_attempts,
        };
        *slot = value.map(|v| n.clamp(v));
    }

    pub fn choice(&self, c: Choice) -> Option<u32> {
        match c {
            Choice::Scale => self.scale,
            // An older on/off value counts until a direction is chosen.
            Choice::Clipboard => self.clipboard_mode.or(self
                .clipboard
                .map(|on| if on { clipboard::BOTH } else { clipboard::OFF })),
        }
    }

    /// Sets a choice; `None` inherits, a value that isn't offered too.
    pub fn set_choice(&mut self, c: Choice, value: Option<u32>) {
        let slot = match c {
            Choice::Scale => &mut self.scale,
            Choice::Clipboard => {
                // The older value is replaced, not left to come back.
                self.clipboard = None;
                &mut self.clipboard_mode
            }
        };
        *slot = value.and_then(|v| c.valid(v));
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The settings a connection actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Each setting really is on or off; a bitflag type would only hide that.
#[allow(clippy::struct_excessive_bools)]
pub struct Effective {
    pub timeout_secs: u32,
    pub fullscreen: bool,
    pub all_monitors: bool,
    pub new_workspace: bool,
    pub sound: bool,
    pub microphone: bool,
    pub camera: bool,
    /// `clipboard::*`.
    pub clipboard: u32,
    pub clipboard_files: bool,
    pub system_shortcuts: bool,
    pub refresh_mins: u32,
    pub sign_in_mins: u32,
    pub offline_days: u32,
    pub reconnect: bool,
    pub reconnect_attempts: u32,
    /// Percent, or 0 to follow the monitor (`display::x11_scale`).
    pub scale: u32,
}

/// Built-in defaults. 120 s: measured Entra exchanges took up to 49.8 s
/// (docs/milestone-1.md). Microphone, camera and clipboard off (spec §9).
/// Lists refresh hourly; signing in may take 5 min (MFA included); the
/// offline list is kept 30 days; a lost connection is retried 5 times;
/// Windows' scaling follows the monitor (spec §3: derived, not fixed).
pub const DEFAULTS: Effective = Effective {
    timeout_secs: 120,
    fullscreen: false,
    all_monitors: false,
    new_workspace: false,
    sound: true,
    microphone: false,
    camera: false,
    clipboard: clipboard::OFF,
    clipboard_files: false,
    // FreeRDP's own default: the desktop gets the keyboard while the
    // pointer is over its focused window.
    system_shortcuts: true,
    refresh_mins: 60,
    sign_in_mins: 5,
    offline_days: 30,
    reconnect: true,
    reconnect_attempts: 5,
    scale: 0,
};

impl Effective {
    /// This level with `s` on top. Hand-edited values are clamped too.
    pub fn with(self, s: Option<&ConnectionSettings>) -> Self {
        let Some(s) = s else { return self };
        let number = |n: Number, own: u32| s.number(n).map_or(own, |v| n.clamp(v));
        Self {
            timeout_secs: number(Number::Timeout, self.timeout_secs),
            fullscreen: s.fullscreen.unwrap_or(self.fullscreen),
            all_monitors: s.all_monitors.unwrap_or(self.all_monitors),
            new_workspace: s.new_workspace.unwrap_or(self.new_workspace),
            sound: s.sound.unwrap_or(self.sound),
            microphone: s.microphone.unwrap_or(self.microphone),
            camera: s.camera.unwrap_or(self.camera),
            clipboard: s
                .choice(Choice::Clipboard)
                .and_then(|v| Choice::Clipboard.valid(v))
                .unwrap_or(self.clipboard),
            clipboard_files: s.clipboard_files.unwrap_or(self.clipboard_files),
            system_shortcuts: s.system_shortcuts.unwrap_or(self.system_shortcuts),
            refresh_mins: number(Number::Refresh, self.refresh_mins),
            sign_in_mins: number(Number::SignIn, self.sign_in_mins),
            offline_days: number(Number::OfflineDays, self.offline_days),
            reconnect: s.reconnect.unwrap_or(self.reconnect),
            reconnect_attempts: number(Number::ReconnectAttempts, self.reconnect_attempts),
            scale: s
                .scale
                .and_then(|v| Choice::Scale.valid(v))
                .unwrap_or(self.scale),
        }
    }

    pub fn choice(&self, c: Choice) -> u32 {
        match c {
            Choice::Scale => self.scale,
            Choice::Clipboard => self.clipboard,
        }
    }

    pub fn number(&self, n: Number) -> u32 {
        match n {
            Number::Timeout => self.timeout_secs,
            Number::Refresh => self.refresh_mins,
            Number::SignIn => self.sign_in_mins,
            Number::OfflineDays => self.offline_days,
            Number::ReconnectAttempts => self.reconnect_attempts,
        }
    }

    pub fn toggle(&self, t: Toggle) -> bool {
        match t {
            Toggle::Fullscreen => self.fullscreen,
            Toggle::AllMonitors => self.all_monitors,
            Toggle::NewWorkspace => self.new_workspace,
            Toggle::Sound => self.sound,
            Toggle::Microphone => self.microphone,
            Toggle::Camera => self.camera,
            Toggle::ClipboardFiles => self.clipboard_files,
            Toggle::SystemShortcuts => self.system_shortcuts,
            Toggle::Reconnect => self.reconnect,
        }
    }

    /// FreeRDP arguments for these settings. "Open on a new workspace" is
    /// done by the app, not FreeRDP.
    pub fn freerdp_args(&self) -> Vec<String> {
        let mut args = vec![format!("/timeout:{}", self.timeout_secs * 1000)];
        // The desktop scale Windows uses (MS-RDPBCGR desktopScaleFactor,
        // 100–500 %). `scale` is resolved before launch; 0 left over means
        // the monitor's scale wasn't known.
        args.push(format!("/scale-desktop:{}", self.scale.clamp(100, 500)));
        // `f` is a FreeRDP *flag*: any prefix, `-f` included, turns it on
        // (WinPR `cmdline.c`, COMMAND_LINE_VALUE_FLAG). So windowed means
        // leaving it out; `rdpfile.rs` drops `screen mode id` so the feed
        // can't switch it on either.
        if self.fullscreen {
            args.push("+f".to_owned());
        }
        // `/multimon` only takes effect in full screen.
        args.push(
            if self.fullscreen && self.all_monitors {
                "/multimon"
            } else {
                "-multimon"
            }
            .to_owned(),
        );
        if self.sound {
            args.extend(["/audio-mode:redirect".to_owned(), "/sound".to_owned()]);
        } else {
            args.push("/audio-mode:none".to_owned());
        }
        if self.microphone {
            args.push("/microphone".to_owned());
        }
        // 3.32.2 has no `/camera` option; this loads the camera channel
        // (MS-RDPECAM), which offers every V4L2 camera
        // (`channels/rdpecam/client`, ADR 0013).
        if self.camera {
            args.push("/dvc:rdpecam".to_owned());
        }
        args.push(self.clipboard_arg());
        // A bool option, on by default (`client/common/cmdline.h`): FreeRDP
        // grabs the keyboard while the pointer is over its focused window,
        // XWayland passes the grab to COSMIC, and COSMIC then sends its own
        // shortcuts to the window (ADR 0018). Right Ctrl releases it.
        args.push(
            if self.system_shortcuts {
                "+grab-keyboard"
            } else {
                "-grab-keyboard"
            }
            .to_owned(),
        );
        args
    }

    /// The clipboard in FreeRDP's terms (`client/common/cmdline.c`): text
    /// and pictures `direction-to`, files `files-to`, each `all`, `remote`
    /// (to the desktop), `local` (to this computer) or `off`. Always both,
    /// because FreeRDP's default lets files through both ways.
    fn clipboard_arg(&self) -> String {
        let direction = match self.clipboard {
            clipboard::BOTH => "all",
            clipboard::TO_DESKTOP => "remote",
            clipboard::TO_COMPUTER => "local",
            _ => return "-clipboard".to_owned(),
        };
        let files = if self.clipboard_files {
            direction
        } else {
            "off"
        };
        format!("/clipboard:direction-to:{direction},files-to:{files}")
    }
}

/// Defaults, then the app's settings, then the account's, then the
/// desktop's.
pub fn effective(
    app: Option<&ConnectionSettings>,
    account: Option<&ConnectionSettings>,
    desktop: Option<&ConnectionSettings>,
) -> Effective {
    DEFAULTS.with(app).with(account).with(desktop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_override_in_order() {
        let account = ConnectionSettings {
            fullscreen: Some(true),
            timeout_secs: Some(60),
            ..Default::default()
        };
        let desktop = ConnectionSettings {
            timeout_secs: Some(300),
            sound: Some(false),
            ..Default::default()
        };
        let app = ConnectionSettings {
            fullscreen: Some(false),
            clipboard_mode: Some(clipboard::BOTH),
            timeout_secs: Some(30),
            ..Default::default()
        };
        let e = effective(Some(&app), Some(&account), Some(&desktop));
        assert!(e.fullscreen); // the account overrides the app
        assert_eq!(e.timeout_secs, 300); // desktop wins
        assert!(!e.sound); // desktop
        assert_eq!(e.clipboard, clipboard::BOTH); // from the app
        assert!(!e.microphone); // default
        assert_eq!(effective(Some(&app), None, None).timeout_secs, 30);
        assert_eq!(effective(None, None, None), DEFAULTS);
        // Account-level numbers follow the same order.
        let app = ConnectionSettings {
            refresh_mins: Some(30),
            sign_in_mins: Some(10),
            ..Default::default()
        };
        let account = ConnectionSettings {
            sign_in_mins: Some(3),
            ..Default::default()
        };
        let e = effective(Some(&app), Some(&account), None);
        assert_eq!((e.refresh_mins, e.sign_in_mins), (30, 3));
    }

    #[test]
    fn numbers_are_clamped() {
        let mut s = ConnectionSettings::default();
        for n in Number::ALL {
            let (min, max, _) = n.range();
            s.set_number(n, Some(0));
            assert_eq!(s.number(n), Some(min));
            s.set_number(n, Some(100_000));
            assert_eq!(s.number(n), Some(max));
            s.set_number(n, None);
        }
        assert!(s.is_empty());
        // Hand-edited config is clamped too.
        let raw = ConnectionSettings {
            timeout_secs: Some(0),
            ..Default::default()
        };
        assert_eq!(
            effective(None, Some(&raw), None).timeout_secs,
            Number::Timeout.range().0
        );
    }

    #[test]
    fn defaults_keep_devices_private() {
        let a = DEFAULTS.freerdp_args();
        assert!(a.contains(&"/timeout:120000".into()));
        assert!(a.contains(&"/scale-desktop:100".into()));
        assert!(!a.iter().any(|x| x.ends_with('f') && x.len() == 2));
        assert!(a.contains(&"-multimon".into()));
        assert!(a.contains(&"-clipboard".into()));
        assert!(!a.iter().any(|x| x.starts_with("/microphone")));
        assert!(!a.iter().any(|x| x.contains("rdpecam")));
        assert!(a.contains(&"/sound".into()));
    }

    #[test]
    fn system_shortcuts_follow_the_setting() {
        assert!(DEFAULTS.freerdp_args().contains(&"+grab-keyboard".into()));
        let desktop = ConnectionSettings {
            system_shortcuts: Some(false),
            ..Default::default()
        };
        let a = effective(None, None, Some(&desktop)).freerdp_args();
        assert!(a.contains(&"-grab-keyboard".into()));
        assert!(!a.contains(&"+grab-keyboard".into()));
    }

    #[test]
    fn all_monitors_only_in_fullscreen() {
        let mut e = DEFAULTS;
        e.all_monitors = true;
        assert!(e.freerdp_args().contains(&"-multimon".into()));
        e.fullscreen = true;
        let a = e.freerdp_args();
        assert!(a.contains(&"+f".into()) && a.contains(&"/multimon".into()));
    }

    #[test]
    fn devices_on_and_off() {
        let mut e = DEFAULTS;
        e.sound = false;
        e.microphone = true;
        e.camera = true;
        e.clipboard = clipboard::BOTH;
        let a = e.freerdp_args();
        assert!(a.contains(&"/audio-mode:none".into()));
        assert!(!a.contains(&"/sound".into()));
        assert!(a.contains(&"/microphone".into()));
        assert!(a.contains(&"/dvc:rdpecam".into()));
        assert!(a.contains(&"/clipboard:direction-to:all,files-to:off".into()));
    }

    #[test]
    fn toggles_round_trip() {
        let mut s = ConnectionSettings::default();
        for t in Toggle::ALL {
            s.set_toggle(t, Some(true));
            assert_eq!(s.toggle(t), Some(true));
            s.set_toggle(t, None);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn scale_levels_and_odd_values() {
        let app = ConnectionSettings {
            scale: Some(150),
            ..Default::default()
        };
        let desktop = ConnectionSettings {
            scale: Some(0),
            ..Default::default()
        };
        assert_eq!(effective(Some(&app), None, None).scale, 150);
        // The desktop goes back to following the monitor.
        assert_eq!(effective(Some(&app), None, Some(&desktop)).scale, 0);
        // A hand-edited value that isn't offered is ignored.
        let odd = ConnectionSettings {
            scale: Some(137),
            ..Default::default()
        };
        assert_eq!(effective(Some(&app), Some(&odd), None).scale, 150);
        let mut s = ConnectionSettings::default();
        s.set_choice(Choice::Scale, Some(999));
        assert!(s.is_empty());
        let mut e = DEFAULTS;
        e.scale = 125;
        assert!(e.freerdp_args().contains(&"/scale-desktop:125".into()));
    }

    #[test]
    fn clipboard_directions_and_files() {
        let arg = |mode, files| {
            let mut e = DEFAULTS;
            e.clipboard = mode;
            e.clipboard_files = files;
            e.clipboard_arg()
        };
        assert_eq!(arg(clipboard::OFF, true), "-clipboard");
        assert_eq!(
            arg(clipboard::TO_DESKTOP, false),
            "/clipboard:direction-to:remote,files-to:off"
        );
        assert_eq!(
            arg(clipboard::TO_COMPUTER, true),
            "/clipboard:direction-to:local,files-to:local"
        );
        // An older on/off value still counts, until a direction is chosen.
        let old = ConnectionSettings {
            clipboard: Some(true),
            ..Default::default()
        };
        assert_eq!(effective(Some(&old), None, None).clipboard, clipboard::BOTH);
        let mut s = old.clone();
        s.set_choice(Choice::Clipboard, Some(clipboard::TO_DESKTOP));
        assert_eq!(s.clipboard, None);
        assert_eq!(
            effective(Some(&s), None, None).clipboard,
            clipboard::TO_DESKTOP
        );
        // Following the level above clears both.
        s.set_choice(Choice::Clipboard, None);
        assert!(s.is_empty());
    }
}
