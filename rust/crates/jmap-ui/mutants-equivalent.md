<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-ui`

## `src/undo_delete/shell_ext.rs`

`<impl Drop for UndoData>::drop` survives replaced with `()`. The body releases
the references to the source and the shell content that `show` took. Under the
mutant, each closed info bar leaks these two objects. Only `show` makes an
`UndoData`, and `show` needs a live `EShellView` with its shell content. No test
in this crate makes a widget (see `evo-sys/tests/gtk.rs`), and `jmap-functional`
runs EDS without Evolution. Thus no test here can see the leak. Out of scope.

## `src/send_later/extension.rs`

`dispatch_activation` survives replaced with `()`. The function only gives the
choice of `chosen` to `activate` or `activate_custom`. Both need a live
composer, and no test in this crate makes a widget. Tests of `chosen` cover the
decision. Out of scope.

The `#[cfg(evolution_eui_manager)]` variant of `ui_definition` survives replaced
with `Default::default()` on a 3.52 build, because that build does not compile
it. On a 3.55+ build, `the_menu_definition_names_every_action` tests it. Out of
scope on 3.52.

## `src/snooze/action.rs`

`warn_dialog` survives replaced with `()`. The function shows a modal GTK
dialog and runs it until the user closes it. No test in this crate makes a
widget, and a modal dialog blocks a test. Tests of `snooze_all` cover the
error that goes into the dialog. Out of scope.

## `src/send_later/schedule.rs`

In `resolve`, the filter `*seconds > 0` survives replaced with `>=`. The two
helpers `next_at_hour` and `next_workday` return only a target that is strictly
later than `now` in whole Unix seconds. Thus `hold` is always 1 or more, and
the filter never sees 0. The filter is a guard against a defective clock.
Equivalent.

## `src/link.rs`

`AccountLink::refresh_token` survives replaced with `false`. To kill it, a test
must get a new OAuth 2.0 access token for a source. Only a running EDS registry
with its OAuth 2.0 service gives one, and `jmap-functional` does not load this
crate. `tests/link.rs` kills the `true` variant. Out of scope.
