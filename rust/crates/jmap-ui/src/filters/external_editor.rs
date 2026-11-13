// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which command opens the script text in an external editor (item 180
//! step (d)).
//!
//! Pure precedence and argv logic only: no GSettings read, process spawn
//! or file monitor here. A later step wires [`choose_command`] to the
//! External Editor plugin's own
//! `org.gnome.evolution.plugin.external-editor` GSettings key and to
//! `$VISUAL`/`$EDITOR`, builds the temp file, and spawns [`build_argv`]'s
//! result.

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
