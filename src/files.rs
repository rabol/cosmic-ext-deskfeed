// SPDX-License-Identifier: MIT

//! Folders only the user can use (spec §6, §12): the log, the offline
//! lists and the sign-in profiles.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

/// Creates `dir` (and parents) as 0700 and makes an existing one 0700 too.
/// An error means it may not be private, and nothing should go in it.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_existing_folders_end_up_private() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let dir = base.join("a/b");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        // An existing, too open folder is tightened.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        // A file in the way is an error, not "private".
        let file = base.join("file");
        fs::write(&file, "x").unwrap();
        assert!(private_dir(&file).is_err());
    }
}
