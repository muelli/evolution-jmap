// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which command opens the script text in an external editor (item 180
//! step (d)).
//!
//! Precedence, argv, temp-file and spawn logic, plus the GSettings read
//! [`gsettings_command`] feeds into [`choose_command`]. [`read_if_changed`]
//! tells a poll loop whether [`write_temp_file`]'s path changed since its
//! last check. Still open: the button, and wiring a poll loop and the
//! script buffer to this module.

use std::ffi::CStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::ptr;

use gio_sys::{
    g_settings_get_string, g_settings_new_full, g_settings_schema_has_key,
    g_settings_schema_source_get_default, g_settings_schema_source_lookup, g_settings_schema_unref,
};
use glib_sys::GTRUE;
use gobject_sys::g_object_unref;
use jmap_backend_core::marshal::take_string;

/// The external editor command to run, or [`EditorCommand::Default`] to
/// hand the file to the desktop's default handler for text instead of
/// spawning a configured command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorCommand {
    /// Run this command line.
    Command(String),
    /// No command configured anywhere: open the file with the desktop's
    /// default handler for text.
    Default,
}

/// Picks the editor command, in the External Editor plugin's own order:
/// its GSettings `command` key first, then `$VISUAL`, then `$EDITOR`,
/// else [`EditorCommand::Default`]. A present but blank or whitespace-only
/// value is treated as absent, so an empty GSettings key falls through to
/// the environment instead of running nothing.
pub fn choose_command(
    gsettings_command: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
) -> EditorCommand {
    [gsettings_command, visual, editor]
        .into_iter()
        .find_map(|candidate| {
            let trimmed = candidate?.trim();
            (!trimmed.is_empty()).then(|| EditorCommand::Command(trimmed.to_string()))
        })
        .unwrap_or(EditorCommand::Default)
}

/// Reads the External Editor plugin's own `command` GSettings key
/// (`org.gnome.evolution.plugin.external-editor`), for [`choose_command`]'s
/// first precedence source. `None` when the plugin's schema is not
/// installed, has no such key, or the key holds an empty string.
pub fn gsettings_command() -> Option<String> {
    gsettings_string(c"org.gnome.evolution.plugin.external-editor", c"command")
}

/// Reads a string key from an installed GSettings schema, or `None` when
/// the schema is not installed, has no such key, or the key holds an
/// empty string.
///
/// `g_settings_new` aborts the whole process when `schema_id` names a
/// schema GLib has not registered, so this looks `schema_id` up through
/// `GSettingsSchemaSource` first: an uninstalled plugin's schema then
/// reads as a plain `None`, never a crash.
fn gsettings_string(schema_id: &CStr, key: &CStr) -> Option<String> {
    // SAFETY: `g_settings_schema_source_get_default` returns GLib's own
    // singleton (transfer none); nothing here frees it.
    let source = unsafe { g_settings_schema_source_get_default() };
    if source.is_null() {
        return None;
    }
    // SAFETY: `source` is non-null; `schema_id` is a valid NUL-terminated
    // string for the call's duration. The returned schema, if any, is
    // transfer full and unreffed below on every path.
    let schema = unsafe { g_settings_schema_source_lookup(source, schema_id.as_ptr(), GTRUE) };
    if schema.is_null() {
        return None;
    }
    // SAFETY: `schema` is non-null, owned by this call until the unref
    // below; `key` is a valid NUL-terminated string for the call's
    // duration.
    let has_key = unsafe { g_settings_schema_has_key(schema, key.as_ptr()) } != 0;
    let value = has_key
        .then(|| {
            // SAFETY: `schema` is non-null; NULL backend and path select
            // GLib's default backend and the schema's own path.
            let settings = unsafe { g_settings_new_full(schema, ptr::null_mut(), ptr::null()) };
            if settings.is_null() {
                return None;
            }
            // SAFETY: `settings` is non-null; `key` was just confirmed
            // present on its schema.
            let raw = unsafe { g_settings_get_string(settings, key.as_ptr()) };
            // SAFETY: `settings` is a GObject this call owns.
            unsafe { g_object_unref(settings.cast()) };
            // SAFETY: `raw` is NULL or a GLib-owned string this call takes
            // ownership of.
            unsafe { take_string(raw) }
        })
        .flatten();
    // SAFETY: `schema` is owned by this call; every path above returns
    // before reaching here, so this runs exactly once.
    unsafe { g_settings_schema_unref(schema) };
    value
}

/// Splits an editor command line into an argv, with `file_path` appended
/// as its final argument. `command` is the string inside
/// [`EditorCommand::Command`]; the [`EditorCommand::Default`] case has no
/// command line to build an argv from.
///
/// Mirrors the GNOME External Editor plugin: the configured command plus
/// the file path, shell-word split so quoting in `command` still works.
/// The file path itself is appended whole, not shell-split, so a space in
/// the path stays one argument.
///
/// Returns `None` when `command` does not parse as a shell word sequence
/// (for example, an unmatched quote) or names no program at all.
pub fn build_argv(command: &str, file_path: &Path) -> Option<Vec<String>> {
    let mut argv = shlex::split(command)?;
    if argv.is_empty() {
        return None;
    }
    argv.push(file_path.to_string_lossy().into_owned());
    Some(argv)
}

/// Writes `script_text` to a new, private file inside `dir` and returns
/// its path, for a later step to spawn an external editor on.
///
/// The file is mode 600 from creation, with no window where it is
/// readable by another local account, and its name is unpredictable, so
/// another local process cannot pre-create or symlink the path before
/// this call opens it. Retries on a name collision; gives up after a
/// bounded number of tries.
pub fn write_temp_file(dir: &Path, script_text: &str) -> io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let pid = std::process::id();
    for attempt in 0..100u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!(
            "evolution-jmap-filter-{pid}-{nanos}-{attempt}.sieve"
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(script_text.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique external-editor temp file",
    ))
}

/// Starts `argv` as a child process: `argv[0]` is the program, the rest
/// are its arguments. Returns the running [`Child`] so the caller can
/// wait for it to finish editing before reloading the temp file.
///
/// Returns an error when `argv` is empty or the program cannot start
/// (not found, not executable, and so on).
pub fn spawn(argv: &[String]) -> io::Result<Child> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty external editor argv"))?;
    std::process::Command::new(program).args(args).spawn()
}

/// What [`write_temp_file`]'s path held last time a poll loop checked it,
/// for [`read_if_changed`] to compare against.
pub struct FileSnapshot(String);

/// Reads `path`'s current content, to compare against on a later poll.
pub fn snapshot_file(path: &Path) -> io::Result<FileSnapshot> {
    Ok(FileSnapshot(std::fs::read_to_string(path)?))
}

/// Re-reads `path` and returns its new content when it differs from
/// `since`, or `None` when the external editor has not written a
/// different script to it yet.
///
/// Compares content, not the file's modify time: a poll loop ties this
/// to a timer, not a filesystem event, so a save that lands within one
/// tick's resolution of the last poll is still caught, and a rewrite of
/// the same text (for example, the editor's own "save" on an unchanged
/// buffer) is correctly not reported as a change.
pub fn read_if_changed(path: &Path, since: &FileSnapshot) -> io::Result<Option<String>> {
    let contents = std::fs::read_to_string(path)?;
    Ok((contents != since.0).then_some(contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uninstalled_schema_reads_as_absent() {
        assert_eq!(
            gsettings_string(
                c"org.gnome.evolution-jmap.no-such-schema-at-all",
                c"command"
            ),
            None
        );
    }

    #[test]
    fn an_installed_schema_with_an_unknown_key_reads_as_absent() {
        assert_eq!(
            gsettings_string(
                c"org.gnome.evolution.plugin.external-editor",
                c"no-such-key-at-all"
            ),
            None
        );
    }

    #[test]
    fn the_real_schema_reads_as_absent_or_a_non_empty_string() {
        if let Some(value) = gsettings_command() {
            assert!(!value.is_empty());
        }
    }

    #[test]
    fn the_gsettings_key_wins_over_the_environment() {
        assert_eq!(
            choose_command(Some("gedit"), Some("vim"), Some("nano")),
            EditorCommand::Command("gedit".to_string())
        );
    }

    #[test]
    fn visual_wins_over_editor_when_gsettings_is_unset() {
        assert_eq!(
            choose_command(None, Some("vim"), Some("nano")),
            EditorCommand::Command("vim".to_string())
        );
    }

    #[test]
    fn editor_is_the_last_fallback() {
        assert_eq!(
            choose_command(None, None, Some("nano")),
            EditorCommand::Command("nano".to_string())
        );
    }

    #[test]
    fn no_source_set_means_the_default_handler() {
        assert_eq!(choose_command(None, None, None), EditorCommand::Default);
    }

    #[test]
    fn a_blank_gsettings_key_falls_through_to_the_environment() {
        assert_eq!(
            choose_command(Some("   "), Some("vim"), None),
            EditorCommand::Command("vim".to_string())
        );
    }

    #[test]
    fn a_blank_value_everywhere_means_the_default_handler() {
        assert_eq!(
            choose_command(Some(""), Some(""), Some("")),
            EditorCommand::Default
        );
    }

    #[test]
    fn a_command_with_surrounding_whitespace_is_trimmed() {
        assert_eq!(
            choose_command(Some("  gedit --wait  "), None, None),
            EditorCommand::Command("gedit --wait".to_string())
        );
    }

    #[test]
    fn a_bare_command_gets_the_file_path_appended() {
        assert_eq!(
            build_argv("gedit", Path::new("/tmp/x.sieve")),
            Some(vec!["gedit".to_string(), "/tmp/x.sieve".to_string()])
        );
    }

    #[test]
    fn command_flags_split_on_whitespace() {
        assert_eq!(
            build_argv("gedit --wait", Path::new("/tmp/x.sieve")),
            Some(vec![
                "gedit".to_string(),
                "--wait".to_string(),
                "/tmp/x.sieve".to_string(),
            ])
        );
    }

    #[test]
    fn quoting_in_the_command_line_still_works() {
        assert_eq!(
            build_argv(r#"emacsclient -c -a "" --wait"#, Path::new("/tmp/x.sieve")),
            Some(vec![
                "emacsclient".to_string(),
                "-c".to_string(),
                "-a".to_string(),
                String::new(),
                "--wait".to_string(),
                "/tmp/x.sieve".to_string(),
            ])
        );
    }

    #[test]
    fn a_space_in_the_file_path_stays_one_argument() {
        assert_eq!(
            build_argv("gedit", Path::new("/tmp/my file.sieve")),
            Some(vec!["gedit".to_string(), "/tmp/my file.sieve".to_string()])
        );
    }

    #[test]
    fn an_unmatched_quote_is_refused() {
        assert_eq!(build_argv(r#"gedit ""#, Path::new("/tmp/x.sieve")), None);
    }

    #[test]
    fn an_empty_command_names_no_program() {
        assert_eq!(build_argv("", Path::new("/tmp/x.sieve")), None);
    }

    #[test]
    fn a_whitespace_only_command_names_no_program() {
        assert_eq!(build_argv("   ", Path::new("/tmp/x.sieve")), None);
    }

    #[test]
    fn the_temp_file_holds_the_script_text() {
        let dir = std::env::temp_dir();
        let path = write_temp_file(&dir, "require [\"fileinto\"];").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "require [\"fileinto\"];"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_temp_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir();
        let path = write_temp_file(&dir, "").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn spawn_starts_the_program_with_its_arguments() {
        let argv = build_argv("true", Path::new("/dev/null")).unwrap();
        let mut child = spawn(&argv).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
    }

    #[test]
    fn spawn_reports_a_program_that_cannot_start() {
        let argv = vec!["evolution-jmap-no-such-editor-binary".to_string()];
        assert!(spawn(&argv).is_err());
    }

    #[test]
    fn spawn_refuses_an_empty_argv() {
        assert_eq!(spawn(&[]).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn an_unchanged_file_reads_as_no_change() {
        let dir = std::env::temp_dir();
        let path = write_temp_file(&dir, "require [\"fileinto\"];").unwrap();
        let since = snapshot_file(&path).unwrap();
        assert_eq!(read_if_changed(&path, &since).unwrap(), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_edited_file_reads_back_its_new_text() {
        let dir = std::env::temp_dir();
        let path = write_temp_file(&dir, "require [\"fileinto\"];").unwrap();
        let since = snapshot_file(&path).unwrap();
        std::fs::write(&path, "require [\"vacation\"];").unwrap();
        assert_eq!(
            read_if_changed(&path, &since).unwrap(),
            Some("require [\"vacation\"];".to_string())
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_rewrite_with_the_same_text_is_not_a_change() {
        let dir = std::env::temp_dir();
        let path = write_temp_file(&dir, "require [\"fileinto\"];").unwrap();
        let since = snapshot_file(&path).unwrap();
        std::fs::write(&path, "require [\"fileinto\"];").unwrap();
        assert_eq!(read_if_changed(&path, &since).unwrap(), None);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn two_calls_do_not_collide() {
        let dir = std::env::temp_dir();
        let a = write_temp_file(&dir, "a").unwrap();
        let b = write_temp_file(&dir, "b").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
    }
}
