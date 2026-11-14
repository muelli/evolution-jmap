// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The last loaded scripts of one account, kept on disk (item 180 step
//! (c)'s cold-start half), so a page opened while already offline can show
//! them read-only instead of an empty page.
//!
//! The read and write of the cache file, and [`cache_dir`], the real
//! directory `page.rs` passes to both: no GTK, no `ESource` here. `page.rs`
//! wires [`save`] to a successful load and [`load`] to a cold start that
//! finds the shell offline.

use std::ffi::CStr;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One script, with just what a read-only cold start needs to show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedScript {
    pub name: String,
    pub text: String,
    pub active: bool,
}

/// The path of one account's cache file inside `cache_dir`, named by the
/// account's `ESource` UID so two accounts never collide.
fn cache_path(cache_dir: &Path, source_uid: &str) -> PathBuf {
    cache_dir.join(format!("filters-{source_uid}.json"))
}

/// This plugin's own cache directory, in the shape `jmap-mail`'s folder-tree
/// cache already uses for its module name: `g_get_user_cache_dir()`, then
/// `evolution`, then `evolution-jmap`, then `filters`.
pub fn cache_dir() -> PathBuf {
    // SAFETY: `g_get_user_cache_dir` takes no arguments and always returns a
    // process-owned, NUL-terminated string GLib keeps for the process's
    // life; `CStr::from_ptr` only borrows it, nothing here frees it.
    let base = unsafe { CStr::from_ptr(glib_sys::g_get_user_cache_dir()) };
    PathBuf::from(base.to_string_lossy().into_owned())
        .join("evolution")
        .join("evolution-jmap")
        .join("filters")
}

/// Writes `scripts` as this account's cache, replacing what was there.
///
/// Mirrors `jmap-mail`'s folder-tree cache: a sibling temp file is written
/// and permission-set first, then renamed over the real path, so a process
/// killed mid-write leaves the previous, still-valid cache in place rather
/// than a truncated one. The directory is mode 700 and the file mode 600:
/// a script's text is private, another local account must not read it.
pub fn save(cache_dir: &Path, source_uid: &str, scripts: &[CachedScript]) -> io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    std::fs::set_permissions(cache_dir, std::fs::Permissions::from_mode(0o700))?;
    let path = cache_path(cache_dir, source_uid);
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let json = serde_json::to_vec(scripts)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    std::fs::write(&tmp, json)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, &path)
}

/// Reads back what [`save`] last wrote for this account.
///
/// `None` when there is no cache yet, or when the file does not parse: a
/// cold-start offline page then falls back to its own offline message,
/// the same as having no cache at all.
pub fn load(cache_dir: &Path, source_uid: &str) -> Option<Vec<CachedScript>> {
    let path = cache_path(cache_dir, source_uid);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory for one test, removed again when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "evolution-jmap-filters-cache-test-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fresh_directory_has_no_cache() {
        let dir = TempDir::new("fresh");
        assert_eq!(load(&dir.0, "account-1"), None);
    }

    #[test]
    fn what_was_saved_is_what_loads_back() {
        let dir = TempDir::new("round-trip");
        let scripts = vec![
            CachedScript {
                name: "evolution-jmap".to_owned(),
                text: "require [\"fileinto\"];\n".to_owned(),
                active: true,
            },
            CachedScript {
                name: "vacation".to_owned(),
                text: "keep;\n".to_owned(),
                active: false,
            },
        ];
        save(&dir.0, "account-1", &scripts).unwrap();
        assert_eq!(load(&dir.0, "account-1"), Some(scripts));
    }

    #[test]
    fn two_accounts_do_not_collide() {
        let dir = TempDir::new("two-accounts");
        let a = vec![CachedScript {
            name: "a".to_owned(),
            text: "keep;\n".to_owned(),
            active: true,
        }];
        let b = vec![CachedScript {
            name: "b".to_owned(),
            text: "discard;\n".to_owned(),
            active: false,
        }];
        save(&dir.0, "account-a", &a).unwrap();
        save(&dir.0, "account-b", &b).unwrap();
        assert_eq!(load(&dir.0, "account-a"), Some(a));
        assert_eq!(load(&dir.0, "account-b"), Some(b));
    }

    #[test]
    fn a_second_save_replaces_the_first() {
        let dir = TempDir::new("replace");
        let first = vec![CachedScript {
            name: "a".to_owned(),
            text: "keep;\n".to_owned(),
            active: true,
        }];
        let second = vec![CachedScript {
            name: "b".to_owned(),
            text: "discard;\n".to_owned(),
            active: false,
        }];
        save(&dir.0, "account-1", &first).unwrap();
        save(&dir.0, "account-1", &second).unwrap();
        assert_eq!(load(&dir.0, "account-1"), Some(second));
    }

    #[test]
    fn the_cache_directory_is_private() {
        let dir = TempDir::new("dir-perms");
        save(&dir.0, "account-1", &[]).unwrap();
        let mode = std::fs::metadata(&dir.0).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn the_cache_file_is_private() {
        let dir = TempDir::new("file-perms");
        save(&dir.0, "account-1", &[]).unwrap();
        let path = cache_path(&dir.0, "account-1");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_corrupt_cache_file_loads_as_none() {
        let dir = TempDir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(cache_path(&dir.0, "account-1"), b"not json").unwrap();
        assert_eq!(load(&dir.0, "account-1"), None);
    }
}
