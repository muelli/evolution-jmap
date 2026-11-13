// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which command opens the script text in an external editor (item 180
//! step (d)).
//!
//! Pure precedence logic only: no GSettings read, process spawn or file
//! monitor here. A later step wires [`choose_command`] to the External
//! Editor plugin's own `org.gnome.evolution.plugin.external-editor`
//! GSettings key and to `$VISUAL`/`$EDITOR`, then spawns the result.

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
}
