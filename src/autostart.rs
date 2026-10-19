// SPDX-License-Identifier: MIT

//! "Start when you log in" (App settings → Background, ADR 0017): an XDG
//! autostart entry, `~/.config/autostart/<app ID>.desktop`, that starts
//! Deskfeed with `--background`. With background operation on and a tray,
//! that start shows only the tray icon; otherwise the window opens.
//!
//! The file itself is the setting, so removing it by hand turns it off.

use std::io::Write;
use std::path::PathBuf;

/// The argument the entry passes.
pub const BACKGROUND_ARG: &str = "--background";

fn path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(
        config
            .join("autostart")
            .join(format!("{}.desktop", crate::app::APP_ID)),
    )
}

fn entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Deskfeed\n\
         Comment=Start Deskfeed in the background\n\
         Exec=cosmic-ext-deskfeed {BACKGROUND_ARG}\n\
         Icon={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        crate::app::APP_ID
    )
}

/// Whether Deskfeed starts at login (its entry is there).
pub fn enabled() -> bool {
    path().is_some_and(|p| p.is_file())
}

/// Adds or removes the entry.
pub fn set(on: bool) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no home folder"))?;
    if !on {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir)?;
    // Written whole, then renamed into place.
    let tmp = dir.join(format!(".{}.desktop.tmp", crate::app::APP_ID));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(entry().as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_starts_in_the_background() {
        let e = entry();
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=cosmic-ext-deskfeed --background\n"));
        assert!(e.contains("\nType=Application\n"));
    }
}
