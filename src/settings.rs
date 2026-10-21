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

/// Declares every setting once: its doc, its name in the saved settings,
/// its built-in default, and its range (numbers) or options (choices).
/// From that come the saved struct ([`ConnectionSettings`]), the resolved
/// one ([`Effective`]) with [`DEFAULTS`], the [`Toggle`], [`Number`] and
/// [`Choice`] enums the settings pages are built from, and the code
/// between them.
macro_rules! settings {
    (
        toggles { $( $(#[$td:meta])* $tv:ident $tf:ident = $tdef:expr, )* }
        numbers { $( $(#[$nd:meta])* $nv:ident $nf:ident = $ndef:expr, range $nrange:expr, )* }
        choices {
            $( $(#[$cd:meta])* $cv:ident $cf:ident $(as $ckey:literal)? = $cdef:expr,
               options $copts:expr, )*
        }
    ) => {
        /// One level of settings; unset fields inherit from the level
        /// above. The field names are the keys stored in the config.
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct ConnectionSettings {
            $( $(#[$td])* pub $tf: Option<bool>, )*
            $( $(#[$nd])* pub $nf: Option<u32>, )*
            $( $(#[$cd])* $(#[serde(rename = $ckey)])? pub $cf: Option<u32>, )*
        }

        /// The settings a connection actually uses.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        // Each setting really is on or off; a bitflag type would only hide
        // that.
        #[allow(clippy::struct_excessive_bools)]
        pub struct Effective {
            $( $(#[$td])* pub $tf: bool, )*
            $( $(#[$nd])* pub $nf: u32, )*
            $( $(#[$cd])* pub $cf: u32, )*
        }

        /// Built-in defaults: each setting's value in `settings!` below.
        pub const DEFAULTS: Effective = Effective {
            $( $tf: $tdef, )*
            $( $nf: $ndef, )*
            $( $cf: $cdef, )*
        };

        /// The on/off settings.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Toggle { $( $(#[$td])* $tv, )* }

        /// The number settings.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Number { $( $(#[$nd])* $nv, )* }

        /// Settings chosen from a list.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Choice { $( $(#[$cd])* $cv, )* }

        impl Toggle {
            #[cfg(test)]
            pub const ALL: &[Self] = &[ $( Self::$tv, )* ];
        }

        impl Number {
            #[cfg(test)]
            pub const ALL: &[Self] = &[ $( Self::$nv, )* ];

            /// Smallest value, largest value, step.
            pub fn range(self) -> (u32, u32, u32) {
                match self { $( Self::$nv => $nrange, )* }
            }
        }

        impl Choice {
            #[cfg(test)]
            pub const ALL: &[Self] = &[ $( Self::$cv, )* ];

            /// The values offered.
            pub fn options(self) -> &'static [u32] {
                match self { $( Self::$cv => &$copts, )* }
            }
        }

        impl ConnectionSettings {
            pub fn toggle(&self, t: Toggle) -> Option<bool> {
                match t { $( Toggle::$tv => self.$tf, )* }
            }

            pub fn set_toggle(&mut self, t: Toggle, value: Option<bool>) {
                match t { $( Toggle::$tv => self.$tf = value, )* }
            }

            pub fn number(&self, n: Number) -> Option<u32> {
                match n { $( Number::$nv => self.$nf, )* }
            }

            /// Sets a number, clamped to its range; `None` inherits.
            pub fn set_number(&mut self, n: Number, value: Option<u32>) {
                let value = value.map(|v| n.clamp(v));
                match n { $( Number::$nv => self.$nf = value, )* }
            }

            pub fn choice(&self, c: Choice) -> Option<u32> {
                match c { $( Choice::$cv => self.$cf, )* }
            }

            /// Sets a choice; `None` inherits, a value that isn't offered
            /// too.
            pub fn set_choice(&mut self, c: Choice, value: Option<u32>) {
                let value = value.and_then(|v| c.valid(v));
                match c { $( Choice::$cv => self.$cf = value, )* }
            }
        }

        impl Effective {
            /// This level with `s` on top. Hand-edited values are clamped,
            /// and ones that aren't offered ignored.
            pub fn with(self, s: Option<&ConnectionSettings>) -> Self {
                let Some(s) = s else { return self };
                Self {
                    $( $tf: s.$tf.unwrap_or(self.$tf), )*
                    $( $nf: s.$nf.map_or(self.$nf, |v| Number::$nv.clamp(v)), )*
                    $( $cf: s.$cf.and_then(|v| Choice::$cv.valid(v)).unwrap_or(self.$cf), )*
                }
            }

            pub fn toggle(&self, t: Toggle) -> bool {
                match t { $( Toggle::$tv => self.$tf, )* }
            }

            pub fn number(&self, n: Number) -> u32 {
                match n { $( Number::$nv => self.$nf, )* }
            }

            pub fn choice(&self, c: Choice) -> u32 {
                match c { $( Choice::$cv => self.$cf, )* }
            }
        }
    };
}

// Built-in defaults: microphone, camera and clipboard off (spec §9).
settings! {
    toggles {
        Fullscreen fullscreen = false,
        AllMonitors all_monitors = false,
        /// With all monitors in full screen: one wide screen (`+span`)
        /// instead of one per monitor (ADR 0029, compared with Remmina).
        SpanMonitors span_monitors = false,
        NewWorkspace new_workspace = false,
        Sound sound = true,
        Microphone microphone = false,
        Camera camera = false,
        /// Files through the clipboard, in the clipboard's direction.
        ClipboardFiles clipboard_files = false,
        /// Super, Alt+Tab and the like go to the desktop (ADR 0018).
        /// FreeRDP's own default: the desktop gets the keyboard while the
        /// pointer is over its focused window.
        SystemShortcuts system_shortcuts = true,
        /// No screen blanking, lock or suspend while a full-screen session
        /// has focus (ADR 0019); spec §9: only when the user turns it on.
        KeepAwake keep_awake = false,
        /// FreeRDP's Ctrl+Alt+Enter (full screen on and off); on by
        /// default, as in FreeRDP.
        FullscreenKey fullscreen_key = true,
        /// A lost connection is retried.
        Reconnect reconnect = true,
    }
    numbers {
        /// Time to wait for the desktop, in seconds. 120 s: measured Entra
        /// exchanges took up to 54.9 s (docs/compatibility.md).
        Timeout timeout_secs = 120, range (10, 600, 10),
        /// How often the desktop lists refresh, in minutes (app and account
        /// levels only). An hour is enough for a list that rarely changes;
        /// a day at most, so new assignments still show up.
        Refresh refresh_mins = 60, range (15, 24 * 60, 15),
        /// Time allowed for signing in, MFA included, in minutes (app and
        /// account levels only). The sign-in window takes at most 15 min
        /// (`signin/src/protocol.rs` `MAX_TIMEOUT_MS`).
        SignIn sign_in_mins = 5, range (2, 15, 1),
        /// How long the offline desktop list is kept, in days (app and
        /// account levels only; spec §10: cached metadata has an age
        /// limit).
        OfflineDays offline_days = 30, range (1, 90, 1),
        /// How many times a lost connection is retried.
        ReconnectAttempts reconnect_attempts = 5, range (1, 10, 1),
    }
    choices {
        /// Windows' display scaling in percent (ADR 0015); 0 follows the
        /// monitor (`display::x11_scale`; spec §3: derived, not fixed).
        /// The options after 0 are Windows' own steps.
        Scale scale = 0, options [0, 100, 125, 150, 175, 200, 250, 300],
        /// Which way copy and paste works: one of `clipboard::*`.
        Clipboard clipboard as "clipboard_mode" = clipboard::OFF,
            options [clipboard::OFF, clipboard::BOTH, clipboard::TO_DESKTOP, clipboard::TO_COMPUTER],
        /// A desktop's window size when it opens, in percent of the monitor
        /// (`/size:N%`, FreeRDP measures the monitor it opens on,
        /// `client/X11/xf_monitor.c`); 0 keeps FreeRDP's 1024 × 768. The
        /// desktop follows the window afterwards (`/dynamic-resolution`).
        WindowSize window_size = 0, options [0, 50, 60, 70, 80, 90, 100],
        /// Bits per pixel (`/bpp:`, the values FreeRDP 3.32.2 accepts);
        /// 32 is FreeRDP's own default, and only 32 asks Windows for a
        /// 32-bit session (`libfreerdp/core/gcc.c`).
        ColorDepth color_depth = 32, options [32, 24, 16, 15, 8],
        /// The connection type Windows adjusts to: one of `network::*`.
        Network network = network::AUTO,
            options [network::AUTO, network::LAN, network::WAN, network::BROADBAND_HIGH,
                     network::BROADBAND_LOW, network::MODEM],
    }
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

/// `Choice::Network` values. Their names are FreeRDP's (`/network:`,
/// `client/common/cmdline.c` `parse_network_options`); the bandwidths in
/// the labels are MS-RDPBCGR's (`TS_UD_CS_CORE` `connectionType`).
pub mod network {
    /// FreeRDP measures it (its default: `CONNECTION_TYPE_AUTODETECT`).
    pub const AUTO: u32 = 0;
    pub const LAN: u32 = 1;
    pub const WAN: u32 = 2;
    pub const BROADBAND_HIGH: u32 = 3;
    pub const BROADBAND_LOW: u32 = 4;
    pub const MODEM: u32 = 5;

    /// FreeRDP's name for a type other than `AUTO`.
    pub fn freerdp_name(value: u32) -> Option<&'static str> {
        Some(match value {
            LAN => "lan",
            WAN => "wan",
            BROADBAND_HIGH => "broadband-high",
            BROADBAND_LOW => "broadband-low",
            MODEM => "modem",
            _ => return None,
        })
    }
}

impl Choice {
    /// A value from the config, if it is one of the options.
    pub fn valid(self, value: u32) -> Option<u32> {
        self.options().contains(&value).then_some(value)
    }
}

impl Number {
    pub fn clamp(self, value: u32) -> u32 {
        let (min, max, _) = self.range();
        value.clamp(min, max)
    }
}

/// One change to a setting. `None` follows the level above (account and
/// desktop levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Toggle(Toggle, Option<bool>),
    /// Clamped to the number's range.
    Number(Number, Option<u32>),
    /// A value that isn't offered follows the level above.
    Choice(Choice, Option<u32>),
}

impl ConnectionSettings {
    /// Applies one change from a settings page.
    pub fn edit(&mut self, edit: Edit) {
        match edit {
            Edit::Toggle(t, value) => self.set_toggle(t, value),
            Edit::Number(n, value) => self.set_number(n, value),
            Edit::Choice(c, value) => self.set_choice(c, value),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A device setting the administrator's policy turned off for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limited {
    Clipboard,
    Microphone,
    Sound,
}

impl Effective {
    /// Turns off what the connection file's device policy doesn't allow
    /// (spec §5: server policy wins over a preference) and says what it
    /// turned off, so the session can show it.
    pub fn limit_to(&mut self, policy: crate::sessions::rdpfile::DevicePolicy) -> Vec<Limited> {
        let mut limited = Vec::new();
        if self.clipboard != clipboard::OFF && !policy.clipboard {
            self.clipboard = clipboard::OFF;
            self.clipboard_files = false;
            limited.push(Limited::Clipboard);
        }
        if self.microphone && !policy.microphone {
            self.microphone = false;
            limited.push(Limited::Microphone);
        }
        if self.sound && !policy.sound {
            self.sound = false;
            limited.push(Limited::Sound);
        }
        limited
    }

    /// FreeRDP arguments for these settings, for a desktop. "Open on a new
    /// workspace" is done by the app, not FreeRDP.
    pub fn freerdp_args(&self) -> Vec<String> {
        self.args(false)
    }

    /// For a RemoteApp (ADR 0020): its windows are ordinary windows, so
    /// the full-screen, keyboard-grab and full-screen-key settings don't
    /// apply, and the remote screen covers every monitor so its windows can
    /// go anywhere (FreeRDP sizes it to one monitor otherwise,
    /// `client/X11/xf_monitor.c`).
    pub fn freerdp_app_args(&self) -> Vec<String> {
        self.args(true)
    }

    fn args(&self, remote_app: bool) -> Vec<String> {
        let fullscreen = self.fullscreen && !remote_app;
        let mut args = vec![format!("/timeout:{}", self.timeout_secs * 1000)];
        // The desktop scale Windows uses (MS-RDPBCGR desktopScaleFactor,
        // 100–500 %). `scale` is resolved before launch; 0 left over means
        // the monitor's scale wasn't known.
        args.push(format!("/scale-desktop:{}", self.scale.clamp(100, 500)));
        // `f` is a FreeRDP *flag*: any prefix, `-f` included, turns it on
        // (WinPR `cmdline.c`, COMMAND_LINE_VALUE_FLAG). So windowed means
        // leaving it out; `rdpfile.rs` drops `screen mode id` so the feed
        // can't switch it on either.
        if fullscreen {
            args.push("+f".to_owned());
        }
        // A window's starting size; full screen and an app's windows take
        // the monitors' size instead.
        if !fullscreen && !remote_app && self.window_size > 0 {
            args.push(format!("/size:{}%", self.window_size));
        }
        // `/multimon` only takes effect in full screen, or for a RemoteApp.
        // `+span` covers every monitor on its own, as one screen
        // (`xf_monitor.c` checks `UseMultimon || SpanMonitors`), and then
        // no per-monitor layout is sent (`gcc.c`).
        let span = fullscreen && self.all_monitors && self.span_monitors && !remote_app;
        args.push(
            if remote_app || (fullscreen && self.all_monitors && !span) {
                "/multimon"
            } else {
                "-multimon"
            }
            .to_owned(),
        );
        if span {
            args.push("+span".to_owned());
        }
        // FreeRDP's defaults (32 bits, network detected) need no argument,
        // so they stay exactly as without these settings.
        if self.color_depth != 32 {
            args.push(format!("/bpp:{}", self.color_depth));
        }
        if let Some(name) = network::freerdp_name(self.network) {
            args.push(format!("/network:{name}"));
        }
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
            if self.system_shortcuts && !remote_app {
                "+grab-keyboard"
            } else {
                "-grab-keyboard"
            }
            .to_owned(),
        );
        // A bool option, on by default (`client/X11/xf_keyboard.c`,
        // `fullscreen_toggle`). Ctrl+Alt+M (minimize) and Ctrl+Alt+D
        // (disconnect) have no switch in 3.32.2.
        args.push(
            if self.fullscreen_key && !remote_app {
                "+toggle-fullscreen"
            } else {
                "-toggle-fullscreen"
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

    /// A desktop's settings as an earlier version saved them (with the
    /// old on/off `clipboard`), read the way cosmic-config reads them:
    /// they still load, with every other value kept.
    #[test]
    fn settings_saved_by_earlier_versions_still_load() {
        let saved = r#"{
    "z06as8n1B0WLUh_L01y7Ix3x": (
        timeout_secs: None,
        fullscreen: Some(true),
        new_workspace: Some(true),
        sound: Some(true),
        clipboard: None,
        clipboard_mode: Some(1),
        clipboard_files: Some(true),
    ),
    "older": (
        clipboard: Some(true),
        fullscreen: Some(false),
    ),
}"#;
        let map: std::collections::BTreeMap<String, ConnectionSettings> =
            ron::from_str(saved).unwrap();
        let first = &map["z06as8n1B0WLUh_L01y7Ix3x"];
        assert_eq!(first.fullscreen, Some(true));
        assert_eq!(first.new_workspace, Some(true));
        assert_eq!(first.clipboard, Some(1));
        assert_eq!(first.clipboard_files, Some(true));
        assert_eq!(map["older"].fullscreen, Some(false));
    }

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
            clipboard: Some(clipboard::BOTH),
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
        for &n in Number::ALL {
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
    fn fullscreen_key_follows_the_setting() {
        assert!(
            DEFAULTS
                .freerdp_args()
                .contains(&"+toggle-fullscreen".into())
        );
        let mut e = DEFAULTS;
        e.fullscreen_key = false;
        assert!(e.freerdp_args().contains(&"-toggle-fullscreen".into()));
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

    /// Expectations from FreeRDP 3.32.2's sources (`client/common/cmdline.c`,
    /// `client/X11/xf_monitor.c`, `libfreerdp/core/gcc.c`), not the code.
    #[test]
    fn window_size_color_depth_network_and_span() {
        let has = |a: &[String], x: &str| a.iter().any(|y| y == x);
        let starts = |a: &[String], p: &str| a.iter().any(|y| y.starts_with(p));
        // The defaults add nothing: FreeRDP's own 1024 × 768, 32 bits and
        // detected network, as before these settings.
        let d = DEFAULTS.freerdp_args();
        for p in ["/size:", "/bpp:", "/network:", "+span"] {
            assert!(!starts(&d, p), "{p} in {d:?}");
        }
        let mut e = DEFAULTS;
        e.window_size = 80;
        e.color_depth = 16;
        e.network = network::WAN;
        let a = e.freerdp_args();
        assert!(has(&a, "/size:80%") && has(&a, "/bpp:16") && has(&a, "/network:wan"));
        // Full screen and an app's windows have no window size; colour
        // and network still apply to an app.
        e.fullscreen = true;
        assert!(!starts(&e.freerdp_args(), "/size:"));
        e.fullscreen = false;
        let app = e.freerdp_app_args();
        assert!(!starts(&app, "/size:") && has(&app, "/bpp:16") && has(&app, "/network:wan"));
        // Span: only with all monitors in full screen, instead of the
        // per-monitor layout.
        let mut e = DEFAULTS;
        e.span_monitors = true;
        e.all_monitors = true;
        assert!(!has(&e.freerdp_args(), "+span"), "windowed");
        e.fullscreen = true;
        let a = e.freerdp_args();
        assert!(has(&a, "+span") && has(&a, "-multimon") && !has(&a, "/multimon"));
        e.all_monitors = false;
        assert!(!has(&e.freerdp_args(), "+span"), "one monitor");
        // An app's windows keep every monitor, never spanned.
        e.all_monitors = true;
        let app = e.freerdp_app_args();
        assert!(has(&app, "/multimon") && !has(&app, "+span"));
        // Every network type has FreeRDP's name; Automatic passes none.
        for &v in Choice::Network.options() {
            assert_eq!(network::freerdp_name(v).is_none(), v == network::AUTO);
        }
    }

    #[test]
    fn a_remote_app_gets_ordinary_windows_on_every_monitor() {
        let mut e = DEFAULTS;
        e.fullscreen = true;
        e.all_monitors = false;
        e.system_shortcuts = true;
        e.fullscreen_key = true;
        e.sound = true;
        let a = e.freerdp_app_args();
        assert!(!a.contains(&"+f".into()));
        assert!(a.contains(&"/multimon".into()) && !a.contains(&"-multimon".into()));
        assert!(a.contains(&"-grab-keyboard".into()));
        assert!(a.contains(&"-toggle-fullscreen".into()));
        // Devices and the rest still follow the settings.
        assert!(a.contains(&"/sound".into()));
        assert!(a.iter().any(|x| x.starts_with("/timeout:")));
        // A desktop with the same settings is unchanged.
        let d = e.freerdp_args();
        assert!(d.contains(&"+f".into()) && d.contains(&"+grab-keyboard".into()));
    }

    #[test]
    fn the_administrators_policy_wins() {
        use crate::sessions::rdpfile::DevicePolicy;
        let mut e = DEFAULTS;
        e.clipboard = clipboard::BOTH;
        e.clipboard_files = true;
        e.microphone = true;
        e.sound = true;
        let all = DevicePolicy {
            clipboard: true,
            microphone: true,
            sound: true,
            drives: true,
        };
        let mut allowed = e;
        assert_eq!(allowed.limit_to(all), []);
        assert_eq!(allowed, e);
        let none = DevicePolicy {
            clipboard: false,
            microphone: false,
            sound: false,
            drives: false,
        };
        let limited = e.limit_to(none);
        assert_eq!(
            limited,
            [Limited::Clipboard, Limited::Microphone, Limited::Sound]
        );
        let a = e.freerdp_args();
        assert!(a.contains(&"-clipboard".into()));
        assert!(a.contains(&"/audio-mode:none".into()));
        assert!(!a.contains(&"/microphone".into()));
        // Nothing to say when the user had it off anyway.
        let mut off = DEFAULTS;
        off.clipboard = clipboard::OFF;
        off.microphone = false;
        off.sound = false;
        assert_eq!(off.limit_to(none), []);
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

    /// The keys a settings file holds, by setting. Changing one loses
    /// what users saved under the old key.
    // Extended 2026-10-20 with span_monitors, window_size, color_depth and
    // network (ADR 0029, compared with Remmina); existing keys unchanged.
    const SAVED_KEYS: [&str; 22] = [
        "timeout_secs",
        "fullscreen",
        "all_monitors",
        "new_workspace",
        "sound",
        "microphone",
        "camera",
        "clipboard_mode",
        "clipboard_files",
        "system_shortcuts",
        "keep_awake",
        "fullscreen_key",
        "refresh_mins",
        "sign_in_mins",
        "offline_days",
        "reconnect",
        "reconnect_attempts",
        "scale",
        "span_monitors",
        "window_size",
        "color_depth",
        "network",
    ];

    /// Every key, read the way cosmic-config reads it, lands in its own
    /// setting; written, a settings value has exactly these keys.
    #[test]
    fn the_saved_format_is_fixed() {
        let saved = r"(
            timeout_secs: Some(130),
            fullscreen: Some(true),
            all_monitors: Some(false),
            new_workspace: Some(true),
            sound: Some(false),
            microphone: Some(true),
            camera: Some(false),
            clipboard_mode: Some(2),
            clipboard_files: Some(true),
            system_shortcuts: Some(false),
            keep_awake: Some(true),
            fullscreen_key: Some(false),
            refresh_mins: Some(45),
            sign_in_mins: Some(7),
            offline_days: Some(12),
            reconnect: Some(true),
            reconnect_attempts: Some(3),
            scale: Some(175),
            span_monitors: Some(true),
            window_size: Some(80),
            color_depth: Some(16),
            network: Some(4),
        )";
        let s: ConnectionSettings = ron::from_str(saved).unwrap();
        let toggles = [
            (Toggle::Fullscreen, true),
            (Toggle::AllMonitors, false),
            (Toggle::NewWorkspace, true),
            (Toggle::Sound, false),
            (Toggle::Microphone, true),
            (Toggle::Camera, false),
            (Toggle::ClipboardFiles, true),
            (Toggle::SystemShortcuts, false),
            (Toggle::KeepAwake, true),
            (Toggle::FullscreenKey, false),
            (Toggle::Reconnect, true),
            (Toggle::SpanMonitors, true),
        ];
        for (t, on) in toggles {
            assert_eq!(s.toggle(t), Some(on), "{t:?}");
        }
        let numbers = [
            (Number::Timeout, 130),
            (Number::Refresh, 45),
            (Number::SignIn, 7),
            (Number::OfflineDays, 12),
            (Number::ReconnectAttempts, 3),
        ];
        for (n, v) in numbers {
            assert_eq!(s.number(n), Some(v), "{n:?}");
        }
        assert_eq!(s.choice(Choice::Clipboard), Some(clipboard::TO_DESKTOP));
        assert_eq!(s.choice(Choice::Scale), Some(175));
        assert_eq!(s.choice(Choice::WindowSize), Some(80));
        assert_eq!(s.choice(Choice::ColorDepth), Some(16));
        assert_eq!(s.choice(Choice::Network), Some(network::BROADBAND_LOW));
        // Written: exactly the saved keys.
        let written = ron::to_string(&s).unwrap();
        let mut keys: Vec<&str> = written
            .trim_matches(|c| c == '(' || c == ')')
            .split(',')
            .filter_map(|kv| kv.split(':').next())
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .collect();
        keys.sort_unstable();
        let mut expected = SAVED_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{written}");
    }

    /// Each setting, set on a level, reaches the settings a connection
    /// uses; and each level's getter returns what its setter set.
    #[test]
    fn every_setting_flows_through_the_levels() {
        for &t in Toggle::ALL {
            let mut s = ConnectionSettings::default();
            let flipped = !DEFAULTS.toggle(t);
            s.set_toggle(t, Some(flipped));
            assert_eq!(s.toggle(t), Some(flipped), "{t:?}");
            let e = DEFAULTS.with(Some(&s));
            assert_eq!(e.toggle(t), flipped, "{t:?}");
            for other in Toggle::ALL.iter().copied().filter(|&o| o != t) {
                assert_eq!(
                    e.toggle(other),
                    DEFAULTS.toggle(other),
                    "{t:?} moved {other:?}"
                );
            }
        }
        for &n in Number::ALL {
            let mut s = ConnectionSettings::default();
            let (min, _, _) = n.range();
            let value = if DEFAULTS.number(n) == min {
                min + 1
            } else {
                min
            };
            s.set_number(n, Some(value));
            assert_eq!(s.number(n), Some(value), "{n:?}");
            let e = DEFAULTS.with(Some(&s));
            assert_eq!(e.number(n), value, "{n:?}");
            for other in Number::ALL.iter().copied().filter(|&o| o != n) {
                assert_eq!(
                    e.number(other),
                    DEFAULTS.number(other),
                    "{n:?} moved {other:?}"
                );
            }
        }
        for &c in Choice::ALL {
            let mut s = ConnectionSettings::default();
            let value = *c
                .options()
                .iter()
                .find(|&&v| v != DEFAULTS.choice(c))
                .unwrap();
            s.set_choice(c, Some(value));
            assert_eq!(s.choice(c), Some(value), "{c:?}");
            assert_eq!(DEFAULTS.with(Some(&s)).choice(c), value, "{c:?}");
        }
    }

    #[test]
    fn toggles_round_trip() {
        let mut s = ConnectionSettings::default();
        for &t in Toggle::ALL {
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
        let mut s = ConnectionSettings::default();
        s.set_choice(Choice::Clipboard, Some(clipboard::TO_DESKTOP));
        assert_eq!(
            effective(Some(&s), None, None).clipboard,
            clipboard::TO_DESKTOP
        );
        // Following the level above clears it.
        s.set_choice(Choice::Clipboard, None);
        assert!(s.is_empty());
    }
}
