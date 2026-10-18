// SPDX-License-Identifier: MIT

//! Resource icons from the feed (spec §7: icons are untrusted, with bounded
//! size and dimensions).
//!
//! Only the feed's PNG (`Icon32`) is used; the `.ico` isn't. Before an icon
//! goes to the renderer, its header must be a PNG whose first chunk is a
//! well-formed `IHDR` (PNG specification §5.2–5.3, §11.2.2) with at most
//! [`MAX_SIDE`] pixels a side. That bounds what decoding can allocate; the
//! renderer's PNG decoder (`png`, through libcosmic's `image`) checks the
//! rest of the file.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// Feed icons are 32×32; larger ones are refused.
pub const MAX_SIDE: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconError {
    NotPng,
    BadHeader,
    TooLarge { width: u32, height: u32 },
}

/// The icon's width and height, if it may be shown.
pub fn check_png(bytes: &[u8]) -> Result<(u32, u32), IconError> {
    let rest = bytes.strip_prefix(&SIGNATURE).ok_or(IconError::NotPng)?;
    // Length (4), type (4), then IHDR's 13 bytes.
    let header = rest.get(..21).ok_or(IconError::BadHeader)?;
    let be = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    if be(0) != 13 || &header[4..8] != b"IHDR" {
        return Err(IconError::BadHeader);
    }
    let (width, height) = (be(8), be(12));
    if width == 0 || height == 0 {
        return Err(IconError::BadHeader);
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(IconError::TooLarge { width, height });
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut v = SIGNATURE.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        v
    }

    #[test]
    fn accepts_small_png() {
        assert_eq!(check_png(&png_header(32, 32)), Ok((32, 32)));
        assert_eq!(check_png(&png_header(256, 1)), Ok((256, 1)));
    }

    #[test]
    fn refuses_large_or_odd_images() {
        assert_eq!(
            check_png(&png_header(257, 32)),
            Err(IconError::TooLarge {
                width: 257,
                height: 32
            })
        );
        // A decompression bomb announces itself in the header.
        assert!(matches!(
            check_png(&png_header(100_000, 100_000)),
            Err(IconError::TooLarge { .. })
        ));
        assert_eq!(check_png(&png_header(0, 32)), Err(IconError::BadHeader));
        assert_eq!(check_png(b"GIF89a......"), Err(IconError::NotPng));
        assert_eq!(check_png(&[]), Err(IconError::NotPng));
        assert_eq!(check_png(&SIGNATURE), Err(IconError::BadHeader));
        let mut wrong_chunk = png_header(32, 32);
        wrong_chunk[12..16].copy_from_slice(b"tEXt");
        assert_eq!(check_png(&wrong_chunk), Err(IconError::BadHeader));
    }

    /// A real PNG's header is read correctly.
    #[test]
    fn real_png_passes() {
        let bytes = include_bytes!("../../assets/cosmic-ext-deskfeed-logo.png");
        let (w, h) = (
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        );
        // The logo is larger than an icon may be: refused for its size,
        // not its format.
        let expected = if w <= MAX_SIDE && h <= MAX_SIDE {
            Ok((w, h))
        } else {
            Err(IconError::TooLarge {
                width: w,
                height: h,
            })
        };
        assert_eq!(check_png(bytes), expected);
    }
}
