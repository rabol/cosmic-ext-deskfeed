// SPDX-License-Identifier: MIT

//! Connection settings (spec §Settings): built-in defaults, overridden per
//! account, overridden per desktop. `None` means "inherit".
//!
//! The app alone decides display: `sessions/rdpfile.rs` drops the feed's
//! `screen mode id` and `use multimon`. Every option here was checked
//! against `sdl-freerdp3` 3.32.2 `/help` and WinPR's parser: `f` is a flag
//! (only "on" exists), `multimon` and `clipboard` are booleans (`-` turns
//! them off).

use serde::{Deserialize, Serialize};

pub const TIMEOUT_MIN_SECS: u32 = 10;
pub const TIMEOUT_MAX_SECS: u32 = 600;

/// One level of settings; unset fields inherit from the level above.
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
    pub clipboard: Option<bool>,
}

/// The on/off settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Fullscreen,
    AllMonitors,
    NewWorkspace,
    Sound,
    Microphone,
    Clipboard,
}

impl Toggle {
    #[cfg(test)]
    pub const ALL: [Self; 6] = [
        Self::Fullscreen,
        Self::AllMonitors,
        Self::NewWorkspace,
        Self::Sound,
        Self::Microphone,
        Self::Clipboard,
    ];
}

impl ConnectionSettings {
    pub fn toggle(&self, t: Toggle) -> Option<bool> {
        match t {
            Toggle::Fullscreen => self.fullscreen,
            Toggle::AllMonitors => self.all_monitors,
            Toggle::NewWorkspace => self.new_workspace,
            Toggle::Sound => self.sound,
            Toggle::Microphone => self.microphone,
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
            Toggle::Clipboard => &mut self.clipboard,
        };
        *slot = value;
    }

    pub fn set_timeout(&mut self, secs: Option<u32>) {
        self.timeout_secs = secs.map(|s| s.clamp(TIMEOUT_MIN_SECS, TIMEOUT_MAX_SECS));
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The settings a connection actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub timeout_secs: u32,
    pub fullscreen: bool,
    pub all_monitors: bool,
    pub new_workspace: bool,
    pub sound: bool,
    pub microphone: bool,
    pub clipboard: bool,
}

/// Built-in defaults. 120 s: measured Entra exchanges took up to 49.8 s
/// (docs/milestone-1.md). Microphone and clipboard off (spec §11).
pub const DEFAULTS: Effective = Effective {
    timeout_secs: 120,
    fullscreen: false,
    all_monitors: false,
    new_workspace: false,
    sound: true,
    microphone: false,
    clipboard: false,
};

impl Effective {
    /// This level with `s` on top.
    pub fn with(self, s: Option<&ConnectionSettings>) -> Self {
        let Some(s) = s else { return self };
        Self {
            timeout_secs: s.timeout_secs.map_or(self.timeout_secs, |t| {
                t.clamp(TIMEOUT_MIN_SECS, TIMEOUT_MAX_SECS)
            }),
            fullscreen: s.fullscreen.unwrap_or(self.fullscreen),
            all_monitors: s.all_monitors.unwrap_or(self.all_monitors),
            new_workspace: s.new_workspace.unwrap_or(self.new_workspace),
            sound: s.sound.unwrap_or(self.sound),
            microphone: s.microphone.unwrap_or(self.microphone),
            clipboard: s.clipboard.unwrap_or(self.clipboard),
        }
    }

    pub fn toggle(&self, t: Toggle) -> bool {
        match t {
            Toggle::Fullscreen => self.fullscreen,
            Toggle::AllMonitors => self.all_monitors,
            Toggle::NewWorkspace => self.new_workspace,
            Toggle::Sound => self.sound,
            Toggle::Microphone => self.microphone,
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

/// Defaults, then the account's settings, then the desktop's.
pub fn effective(
    account: Option<&ConnectionSettings>,
    desktop: Option<&ConnectionSettings>,
) -> Effective {
    DEFAULTS.with(account).with(desktop)
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
        let e = effective(Some(&account), Some(&desktop));
        assert!(e.fullscreen); // from the account
        assert_eq!(e.timeout_secs, 300); // desktop wins
        assert!(!e.sound); // desktop
        assert!(!e.clipboard); // default
        assert_eq!(effective(None, None), DEFAULTS);
    }

    #[test]
    fn timeouts_are_clamped() {
        let mut s = ConnectionSettings::default();
        s.set_timeout(Some(1));
        assert_eq!(s.timeout_secs, Some(TIMEOUT_MIN_SECS));
        s.set_timeout(Some(100_000));
        assert_eq!(s.timeout_secs, Some(TIMEOUT_MAX_SECS));
        // Hand-edited config is clamped too.
        let raw = ConnectionSettings {
            timeout_secs: Some(0),
            ..Default::default()
        };
        assert_eq!(effective(Some(&raw), None).timeout_secs, TIMEOUT_MIN_SECS);
    }

    #[test]
    fn defaults_keep_devices_private() {
        let a = DEFAULTS.freerdp_args();
        assert!(a.contains(&"/timeout:120000".into()));
        assert!(!a.iter().any(|x| x.ends_with('f') && x.len() == 2));
        assert!(a.contains(&"-multimon".into()));
        assert!(a.contains(&"-clipboard".into()));
        assert!(!a.iter().any(|x| x.starts_with("/microphone")));
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
        e.clipboard = true;
        let a = e.freerdp_args();
        assert!(a.contains(&"/audio-mode:none".into()));
        assert!(!a.contains(&"/sound".into()));
        assert!(a.contains(&"/microphone".into()));
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
