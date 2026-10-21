// SPDX-License-Identifier: MIT

//! A local folder shared with one desktop or saved connection (FreeRDP's
//! `/drive:<name>,<path>`; ADR 0029, compared with Remmina). Chosen per
//! desktop, off by default. FreeRDP 3.32.2 has no read-only share: the
//! remote side can change and delete what is in it.

use std::path::Path;

/// FreeRDP reads a drive's path up to `MAX_PATH` bytes
/// (`channels/drive/client/drive_main.c`, `strnlen(path, MAX_PATH)`): a
/// longer one would be cut short.
const MAX_PATH: usize = 259;

/// The longest share name; it shows in Windows as "<name> on <computer>".
const MAX_NAME: usize = 32;

/// The share as FreeRDP gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareError {
    /// Not an absolute path.
    NotAbsolute,
    /// A comma (FreeRDP splits `/drive:` at commas, `client/common/
    /// cmdline.c`), a control character, or longer than FreeRDP reads.
    Unusable,
    /// Not there, or not a folder (deleted, or a disk not plugged in).
    Missing,
}

impl Share {
    /// FreeRDP's argument.
    pub fn freerdp_arg(&self) -> String {
        format!("/drive:{},{}", self.name, self.path)
    }
}

/// Whether `path` can be passed to FreeRDP at all, without looking at the
/// disk (for the settings, before the folder is there).
pub fn check_form(path: &str) -> Result<(), ShareError> {
    if !path.starts_with('/') {
        return Err(ShareError::NotAbsolute);
    }
    if path.len() > MAX_PATH || path.contains(',') || path.chars().any(char::is_control) {
        return Err(ShareError::Unusable);
    }
    Ok(())
}

/// The share for `path`, checked again before each connection.
pub fn share(path: &str) -> Result<Share, ShareError> {
    check_form(path)?;
    if !Path::new(path).is_dir() {
        return Err(ShareError::Missing);
    }
    Ok(Share {
        name: share_name(path),
        path: path.to_owned(),
    })
}

/// The folder's own name, with only letters, digits, `-` and `_` (FreeRDP
/// would turn `:<>"/\|` and spaces into `_` anyway, and a comma would
/// split the argument); "Deskfeed" when nothing is left, as for `/`.
pub fn share_name(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME)
        .collect();
    if name.trim_matches('_').is_empty() {
        "Deskfeed".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_paths_freerdp_can_take() {
        assert_eq!(check_form("/home/anna/Shared"), Ok(()));
        assert_eq!(check_form("/home/anna/With space"), Ok(()));
        assert_eq!(check_form("Shared"), Err(ShareError::NotAbsolute));
        assert_eq!(check_form(""), Err(ShareError::NotAbsolute));
        assert_eq!(check_form("~/Shared"), Err(ShareError::NotAbsolute));
        // A comma would end the path early; a line break would end the
        // argument (they go to FreeRDP one per line).
        assert_eq!(check_form("/home/anna/a,b"), Err(ShareError::Unusable));
        assert_eq!(check_form("/home/anna/a\nb"), Err(ShareError::Unusable));
        let long = format!("/{}", "a".repeat(MAX_PATH));
        assert_eq!(check_form(&long), Err(ShareError::Unusable));
        assert_eq!(check_form(&long[..MAX_PATH]), Ok(()));
    }

    #[test]
    fn a_missing_folder_is_told_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Shared stuff");
        let path = dir.to_str().unwrap().to_owned();
        assert_eq!(share(&path), Err(ShareError::Missing));
        std::fs::write(&dir, "a file").unwrap();
        assert_eq!(
            share(&path),
            Err(ShareError::Missing),
            "a file, not a folder"
        );
        std::fs::remove_file(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let s = share(&path).unwrap();
        assert_eq!(s.name, "Shared_stuff");
        assert_eq!(s.freerdp_arg(), format!("/drive:Shared_stuff,{path}"));
    }

    #[test]
    fn the_name_is_the_folders_in_safe_characters() {
        assert_eq!(share_name("/home/anna/Projects"), "Projects");
        assert_eq!(share_name("/home/anna/my-files_2"), "my-files_2");
        assert_eq!(share_name("/media/anna/USB: \"Disk\"|x"), "USB___Disk__x");
        assert_eq!(share_name("/home/anna/Bilder æøå"), "Bilder____");
        assert_eq!(share_name("/"), "Deskfeed");
        assert_eq!(share_name("/home/anna/..."), "Deskfeed");
        assert_eq!(share_name(&format!("/{}", "x".repeat(50))).len(), MAX_NAME);
    }
}
