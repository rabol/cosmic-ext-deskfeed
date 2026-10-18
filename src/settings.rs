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
    #[serde(default)]
    pub clipboard: Option<bool>,
    /// App and account levels only.
    #[serde(default)]
    pub refresh_mins: Option<u32>,
    /// App and account levels only.
    #[serde(default)]
    pub sign_in_mins: Option<u32>,
    /// App and account levels only.
    #[serde(default)]
    pub offline_days: Option<u32>,
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
    Clipboard,
}

impl Toggle {
    #[cfg(test)]
    pub const ALL: [Self; 7] = [
        Self::Fullscreen,
        Self::AllMonitors,
        Self::NewWorkspace,
        Self::Sound,
        Self::Microphone,
        Self::Camera,
        Self::Clipboard,
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
}

impl Number {
    pub const ALL: [Self; 4] = [
        Self::Timeout,
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
        }
    }

    pub fn clamp(self, value: u32) -> u32 {
        let (min, max, _) = self.range();
        value.clamp(min, max)
    }

    /// Whether a single desktop can override it.
    pub fn per_desktop(self) -> bool {
        matches!(self, Self::Timeout)
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
            Toggle::Clipboard => self.clipboard,
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
            Toggle::Clipboard => &mut self.clipboard,
        };
        *slot = value;
    }

    pub fn number(&self, n: Number) -> Option<u32> {
        match n {
            Number::Timeout => self.timeout_secs,
            Number::Refresh => self.refresh_mins,
            Number::SignIn => self.sign_in_mins,
            Number::OfflineDays => self.offline_days,
        }
    }

    /// Sets a number, clamped to its range; `None` inherits.
    pub fn set_number(&mut self, n: Number, value: Option<u32>) {
        let slot = match n {
            Number::Timeout => &mut self.timeout_secs,
            Number::Refresh => &mut self.refresh_mins,
            Number::SignIn => &mut self.sign_in_mins,
            Number::OfflineDays => &mut self.offline_days,
        };
        *slot = value.map(|v| n.clamp(v));
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
    pub clipboard: bool,
    pub refresh_mins: u32,
    pub sign_in_mins: u32,
    pub offline_days: u32,
}

/// Built-in defaults. 120 s: measured Entra exchanges took up to 49.8 s
/// (docs/milestone-1.md). Microphone, camera and clipboard off (spec §9).
/// Lists refresh hourly; signing in may take 5 min (MFA included); the
/// offline list is kept 30 days.
pub const DEFAULTS: Effective = Effective {
    timeout_secs: 120,
    fullscreen: false,
    all_monitors: false,
    new_workspace: false,
    sound: true,
    microphone: false,
    camera: false,
    clipboard: false,
    refresh_mins: 60,
    sign_in_mins: 5,
    offline_days: 30,
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
            clipboard: s.clipboard.unwrap_or(self.clipboard),
            refresh_mins: number(Number::Refresh, self.refresh_mins),
            sign_in_mins: number(Number::SignIn, self.sign_in_mins),
            offline_days: number(Number::OfflineDays, self.offline_days),
        }
    }

    pub fn number(&self, n: Number) -> u32 {
        match n {
            Number::Timeout => self.timeout_secs,
            Number::Refresh => self.refresh_mins,
            Number::SignIn => self.sign_in_mins,
            Number::OfflineDays => self.offline_days,
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
            Toggle::Clipboard => self.clipboard,
        }
    }

    /// FreeRDP arguments for these settings. "Open on a new workspace" is
    /// done by the app, not FreeRDP.
    pub fn freerdp_args(&self) -> Vec<String> {
        let mut args = vec![format!("/timeout:{}", self.timeout_secs * 1000)];
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
        args.push(
            if self.clipboard {
                "/clipboard"
            } else {
                "-clipboard"
            }
            .to_owned(),
        );
        args
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
            clipboard: Some(true),
            timeout_secs: Some(30),
            ..Default::default()
        };
        let e = effective(Some(&app), Some(&account), Some(&desktop));
        assert!(e.fullscreen); // the account overrides the app
        assert_eq!(e.timeout_secs, 300); // desktop wins
        assert!(!e.sound); // desktop
        assert!(e.clipboard); // from the app
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
        assert!(!a.iter().any(|x| x.ends_with('f') && x.len() == 2));
        assert!(a.contains(&"-multimon".into()));
        assert!(a.contains(&"-clipboard".into()));
        assert!(!a.iter().any(|x| x.starts_with("/microphone")));
        assert!(!a.iter().any(|x| x.contains("rdpecam")));
        assert!(a.contains(&"/sound".into()));
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
        e.clipboard = true;
        let a = e.freerdp_args();
        assert!(a.contains(&"/audio-mode:none".into()));
        assert!(!a.contains(&"/sound".into()));
        assert!(a.contains(&"/microphone".into()));
        assert!(a.contains(&"/dvc:rdpecam".into()));
        assert!(a.contains(&"/clipboard".into()));
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
}
