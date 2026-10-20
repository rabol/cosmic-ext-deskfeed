// SPDX-License-Identifier: MIT

//! The offline desktop list (spec §7, §10): per account, only what the list
//! shows. Never connection files, URLs or tokens, so a cached entry can't
//! be used to connect (spec §7: the live flow is always required).
//!
//! - `$XDG_CACHE_HOME/cosmic-ext-deskfeed/accounts/<account id>/desktops.json`
//!   (folder 0700, file 0600). Account IDs are opaque hex
//!   (`accounts::account_id`); anything else is refused, so a name can't
//!   leave the folder.
//! - Versioned, written atomically (temporary file, then rename), at most
//!   [`MAX_FILE`] bytes and [`MAX_RESOURCES`] desktops; older than the
//!   account's "keep" setting is ignored and deleted.
//! - Deleted on sign-out and for accounts no longer saved.
//!
//! Blocking; call from `spawn_blocking`.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const FILE: &str = "desktops.json";
pub const MAX_FILE: u64 = 1024 * 1024;
pub const MAX_RESOURCES: usize = 2_000;
const MAX_WORKSPACES: usize = 64;
const MAX_TEXT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedResource {
    pub key: String,
    pub title: String,
    pub kind: crate::service::feed::ResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedWorkspace {
    pub name: String,
    pub resources: Vec<CachedResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub saved: SystemTime,
    pub workspaces: Vec<CachedWorkspace>,
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    /// Seconds since the Unix epoch.
    saved: u64,
    workspaces: Vec<CachedWorkspace>,
}

/// Where the lists live.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The user's cache folder.
    pub fn user() -> Option<Self> {
        Some(Self {
            root: dirs::cache_dir()?
                .join("cosmic-ext-deskfeed")
                .join("accounts"),
        })
    }

    /// Tests: a store in `root`.
    #[cfg(test)]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The account's folder; `None` for anything but an opaque hex ID.
    fn dir(&self, account: &str) -> Option<PathBuf> {
        let valid = !account.is_empty()
            && account.len() <= 64
            && account.bytes().all(|b| b.is_ascii_hexdigit());
        valid.then(|| self.root.join(account))
    }

    /// Saves the list, replacing the previous one. Too large a list isn't
    /// saved (and the old one is removed, so it can't be shown as current).
    pub fn save(&self, account: &str, workspaces: &[CachedWorkspace]) -> Result<(), String> {
        let dir = self.dir(account).ok_or("bad account id")?;
        let count: usize = workspaces.iter().map(|w| w.resources.len()).sum();
        let bounded = workspaces.len() <= MAX_WORKSPACES && count <= MAX_RESOURCES;
        let text = serde_json::to_vec(&OnDisk {
            version: VERSION,
            saved: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            workspaces: workspaces.to_vec(),
        })
        .map_err(|e| e.to_string())?;
        if !bounded || text.len() as u64 > MAX_FILE {
            let _ = self.remove(account);
            return Err("list too large to keep offline".into());
        }
        crate::files::private_dir(&dir).map_err(|e| e.kind().to_string())?;
        // A uniquely named temporary file (0600), renamed over the list; on
        // any error it is removed when dropped.
        let result = (|| {
            let mut tmp = tempfile::Builder::new()
                .prefix(&format!(".{FILE}."))
                .tempfile_in(&dir)?;
            tmp.write_all(&text)?;
            tmp.as_file().sync_all()?;
            tmp.persist(dir.join(FILE)).map_err(|e| e.error)?;
            Ok::<_, std::io::Error>(())
        })();
        result.map_err(|e| e.kind().to_string())
    }

    /// The saved list, if there is one younger than `keep`. A damaged, too
    /// large, too old or other-version file is deleted.
    pub fn load(&self, account: &str, keep: Duration) -> Option<Cached> {
        let path = self.dir(account)?.join(FILE);
        let file = fs::File::open(&path).ok()?;
        let mut bytes = Vec::new();
        let read = file.take(MAX_FILE + 1).read_to_end(&mut bytes);
        let parsed = read
            .ok()
            .filter(|_| bytes.len() as u64 <= MAX_FILE)
            .and_then(|_| serde_json::from_slice::<OnDisk>(&bytes).ok())
            .filter(|d| d.version == VERSION)
            .and_then(|d| {
                let saved = UNIX_EPOCH + Duration::from_secs(d.saved);
                let age = SystemTime::now().duration_since(saved).ok()?;
                (age <= keep).then_some(Cached {
                    saved,
                    workspaces: d.workspaces,
                })
            })
            .filter(valid);
        if parsed.is_none() {
            let _ = fs::remove_file(&path);
        }
        parsed
    }

    /// Deletes the account's offline list (sign-out).
    pub fn remove(&self, account: &str) -> Result<(), ()> {
        match self.dir(account) {
            Some(dir) if dir.exists() => fs::remove_dir_all(dir).map_err(|_| ()),
            _ => Ok(()),
        }
    }

    /// Deletes the lists of accounts that are no longer saved.
    pub fn remove_others(&self, keep: &[String]) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !keep.contains(&name) && self.dir(&name).is_some() {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// The same limits a live feed has (`service/feed.rs`).
fn valid(c: &Cached) -> bool {
    let count: usize = c.workspaces.iter().map(|w| w.resources.len()).sum();
    c.workspaces.len() <= MAX_WORKSPACES
        && count <= MAX_RESOURCES
        && c.workspaces.iter().all(|w| {
            w.name.chars().count() <= MAX_TEXT
                && w.resources.iter().all(|r| {
                    r.title.chars().count() <= MAX_TEXT
                        && r.key.len() <= 128
                        // URL-safe base64 (`app/model.rs` `hash_key`).
                        && r
                            .key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A store in a fresh temporary folder.
    fn with_store(f: impl FnOnce(&Store)) {
        let tmp = tempfile::tempdir().unwrap();
        f(&Store {
            root: tmp.path().to_path_buf(),
        });
    }

    fn sample() -> Vec<CachedWorkspace> {
        vec![CachedWorkspace {
            name: "Work".into(),
            resources: vec![CachedResource {
                key: "aB3-_x".into(),
                title: "Windows 11".into(),
                kind: crate::service::feed::ResourceKind::Desktop,
            }],
        }]
    }

    const DAY: Duration = Duration::from_hours(24);

    #[test]
    fn saves_and_loads_privately() {
        with_store(|st| {
            st.save("ab12", &sample()).unwrap();
            let c = st.load("ab12", DAY).unwrap();
            assert_eq!(c.workspaces, sample());
            let d = st.dir("ab12").unwrap();
            let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&d), 0o700);
            assert_eq!(mode(&d.join(FILE)), 0o600);
            // Accounts don't see each other's lists.
            assert!(st.load("cd34", DAY).is_none());
            st.remove("ab12").unwrap();
            assert!(st.load("ab12", DAY).is_none());
        });
    }

    #[test]
    fn the_kind_is_stored_as_a_word_and_unknown_ones_read_as_other() {
        use crate::service::feed::ResourceKind;
        with_store(|st| {
            st.save("ab12", &sample()).unwrap();
            let file = st.dir("ab12").unwrap().join(FILE);
            let text = fs::read_to_string(&file).unwrap();
            // The format lists written by earlier versions use.
            assert!(text.contains(r#""kind":"desktop""#), "{text}");
            // A kind from a newer version: the list still loads.
            fs::write(
                &file,
                text.replace(r#""kind":"desktop""#, r#""kind":"cloudpc""#),
            )
            .unwrap();
            let c = st.load("ab12", DAY).unwrap();
            assert_eq!(c.workspaces[0].resources[0].kind, ResourceKind::Other);
        });
    }

    #[test]
    fn refuses_odd_ids_old_and_damaged_files() {
        with_store(|st| {
            assert!(st.save("../x", &sample()).is_err());
            assert!(st.save("", &sample()).is_err());
            assert!(st.dir("ab/../cd").is_none());
            st.save("ab12", &sample()).unwrap();
            // Older than the keep setting: ignored and deleted.
            assert!(st.load("ab12", Duration::ZERO).is_none());
            assert!(!st.dir("ab12").unwrap().join(FILE).exists());
            st.save("ab12", &sample()).unwrap();
            fs::write(st.dir("ab12").unwrap().join(FILE), b"{not json").unwrap();
            assert!(st.load("ab12", DAY).is_none());
            assert!(!st.dir("ab12").unwrap().join(FILE).exists());
        });
    }

    #[test]
    fn a_leftover_temporary_file_or_a_second_save_does_not_block_saving() {
        with_store(|st| {
            // A temporary file a crashed run left behind, under any name
            // this process might pick.
            let d = st.dir("ab12").unwrap();
            crate::files::private_dir(&d).unwrap();
            fs::write(d.join(format!(".{FILE}.{}", std::process::id())), "x").unwrap();
            st.save("ab12", &sample()).unwrap();
            assert_eq!(st.load("ab12", DAY).unwrap().workspaces, sample());
            // Saves at the same time all succeed, and no temporary file
            // of theirs is left.
            std::thread::scope(|scope| {
                let saves: Vec<_> = (0..8)
                    .map(|_| scope.spawn(|| st.save("ab12", &sample())))
                    .collect();
                for save in saves {
                    save.join().unwrap().unwrap();
                }
            });
            let left = fs::read_dir(&d).unwrap().count();
            assert_eq!(left, 2, "the list and the crashed run's file");
        });
    }

    #[test]
    fn too_large_lists_are_not_kept() {
        with_store(|st| {
            st.save("ab12", &sample()).unwrap();
            let big = vec![CachedWorkspace {
                name: "W".into(),
                resources: vec![sample()[0].resources[0].clone(); MAX_RESOURCES + 1],
            }];
            assert!(st.save("ab12", &big).is_err());
            // The old list is gone too, so it can't pass for the current one.
            assert!(st.load("ab12", DAY).is_none());
        });
    }

    #[test]
    fn removes_lists_of_unsaved_accounts() {
        with_store(|st| {
            st.save("aa", &sample()).unwrap();
            st.save("bb", &sample()).unwrap();
            st.remove_others(&["aa".to_owned()]);
            assert!(st.load("aa", DAY).is_some());
            assert!(st.load("bb", DAY).is_none());
        });
    }
}
