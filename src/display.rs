// SPDX-License-Identifier: MIT

//! The scale X11 windows get, for Windows' display scaling (ADR 0015).
//!
//! FreeRDP's window is an X11 window. COSMIC gives X11 windows the full
//! pixels of a scaled monitor (its default "fractional" `XWayland`
//! descaling) and tells X11 programs the matching DPI through XSETTINGS
//! `Xft/DPI` (96 × 1024 × scale; cosmic-comp `src/xwayland.rs`
//! `update_xwayland_settings`). Without it, Windows draws at 100 % and its
//! text is smaller than the local apps'. When COSMIC instead scales X11
//! windows up itself (descaling off), `Xft/DPI` is 96 and Windows must stay
//! at 100 %, so reading this value is right in both cases.
//!
//! Blocking (a few X11 round trips); call from `spawn_blocking`.

use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

/// Largest XSETTINGS block read.
const MAX_SETTINGS: u32 = 64 * 1024;

/// The X11 scale, for example 1.25; `None` without an X server or an
/// XSETTINGS manager that sets `Xft/DPI`.
pub fn x11_scale() -> Option<f64> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let selection = conn
        .intern_atom(false, format!("_XSETTINGS_S{screen}").as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    let settings = conn
        .intern_atom(false, b"_XSETTINGS_SETTINGS")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let owner = conn
        .get_selection_owner(selection)
        .ok()?
        .reply()
        .ok()?
        .owner;
    if owner == x11rb::NONE {
        return None;
    }
    let reply = conn
        .get_property(false, owner, settings, AtomEnum::ANY, 0, MAX_SETTINGS / 4)
        .ok()?
        .reply()
        .ok()?;
    let dpi = xft_dpi(&reply.value)?;
    Some(f64::from(dpi) / 1024.0 / 96.0)
}

/// Windows' desktop scale in percent for an X11 scale: rounded, within
/// what RDP allows (100–500 %, MS-RDPBCGR 2.2.1.3.2).
pub fn desktop_scale_percent(scale: f64) -> u32 {
    if !scale.is_finite() {
        return 100;
    }
    // Clamped first, so the cast can't overflow.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (scale * 100.0).round().clamp(100.0, 500.0) as u32;
    percent
}

/// The percent to send for a scale setting: a fixed value as it is, 0
/// (follow the monitor) from the X11 scale, 100 if that isn't known.
pub fn resolve_percent(setting: u32) -> u32 {
    if setting != 0 {
        return setting;
    }
    if let Some(scale) = x11_scale() {
        let percent = desktop_scale_percent(scale);
        crate::app_log!("display scale: {percent} % (follows the monitor)");
        percent
    } else {
        crate::app_log!("display scale: X11 scale unknown, using 100 %");
        100
    }
}

/// `Xft/DPI` from an XSETTINGS block (freedesktop XSETTINGS
/// specification 0.5): byte order, 3 unused bytes, serial, count, then
/// each setting: type (0 integer, 1 string, 2 colour), unused byte, name
/// length (u16), name padded to 4, last-change serial, value.
fn xft_dpi(data: &[u8]) -> Option<i32> {
    let big = match data.first()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let pad4 = |n: usize| n.div_ceil(4) * 4;
    let count = u32_at(8)?;
    let mut at = 12;
    for _ in 0..count.min(4096) {
        let kind = *data.get(at)?;
        let name_len = usize::from(u16_at(at + 2)?);
        let name = data.get(at + 4..at + 4 + name_len)?;
        at += 4 + pad4(name_len) + 4; // header, name, last-change serial
        match kind {
            0 => {
                let value = u32_at(at)?;
                if name == b"Xft/DPI" {
                    return i32::try_from(value).ok();
                }
                at += 4;
            }
            1 => at += 4 + pad4(usize::try_from(u32_at(at)?).ok()?),
            2 => at += 8,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An XSETTINGS block as cosmic-comp writes one (little-endian).
    fn block(entries: &[(&str, u8, Vec<u8>)]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0];
        v.extend_from_slice(&7u32.to_le_bytes()); // serial
        v.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        for (name, kind, value) in entries {
            v.push(*kind);
            v.push(0);
            v.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
            v.extend_from_slice(name.as_bytes());
            v.resize(v.len().div_ceil(4) * 4, 0);
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(value);
        }
        v
    }

    fn int(v: i32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    fn string(s: &str) -> Vec<u8> {
        let mut v = u32::try_from(s.len()).unwrap().to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v.resize(4 + s.len().div_ceil(4) * 4, 0);
        v
    }

    #[test]
    fn finds_xft_dpi_among_other_settings() {
        // cosmic-comp at 1.25: 96 × 1024 × 1.25.
        let data = block(&[
            ("Gtk/CursorThemeName", 1, string("Cosmic")),
            ("Net/ThemeColor", 2, vec![0; 8]),
            ("Xft/DPI", 0, int(122_880)),
            ("Gdk/WindowScalingFactor", 0, int(1)),
        ]);
        assert_eq!(xft_dpi(&data), Some(122_880));
        assert_eq!(xft_dpi(&block(&[("Gdk/UnscaledDPI", 0, int(1))])), None);
    }

    #[test]
    fn refuses_damaged_blocks() {
        assert_eq!(xft_dpi(&[]), None);
        assert_eq!(xft_dpi(&[9, 0, 0, 0]), None);
        let mut cut = block(&[("Xft/DPI", 0, int(98_304))]);
        cut.truncate(cut.len() - 2);
        assert_eq!(xft_dpi(&cut), None);
        // Claims many settings, has none.
        let mut many = block(&[]);
        many[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(xft_dpi(&many), None);
    }

    #[test]
    fn percent_is_rounded_and_bounded() {
        assert_eq!(desktop_scale_percent(1.25), 125);
        assert_eq!(desktop_scale_percent(1.0), 100);
        assert_eq!(desktop_scale_percent(0.5), 100);
        assert_eq!(desktop_scale_percent(1.333), 133);
        assert_eq!(desktop_scale_percent(9.0), 500);
        assert_eq!(desktop_scale_percent(f64::NAN), 100);
    }

    /// The running session's X11 scale: `cargo test --release
    /// real_x11_scale -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the running session's XSETTINGS"]
    fn real_x11_scale() {
        let scale = x11_scale().expect("XSETTINGS with Xft/DPI");
        println!("X11 scale {scale} -> {} %", desktop_scale_percent(scale));
    }
}
