// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This build script follows the same pattern as eds-sys's build.rs, in the
// sibling crate one level up. It generates the bindings at build time and
// does not check them into git. Evolution's class structs change between
// releases. A stale checked-in binding would give a silently wrong ABI
// instead of a build failure. tests/layout.rs checks the generated result
// against the running GObject type system.

use std::env;
use std::path::PathBuf;

/// Evolution 3.52 is the minimum supported version. Evolution 3.56 replaced
/// GtkUIManager. A lower minimum would claim support for a release never
/// tested against this crate.
const MIN_EVO: &str = "3.52";

/// EUIManager replaced GtkUIManager in Evolution 3.55.1, a development
/// release in the 3.56 cycle. `jmap-ui` picks between
/// `GtkAction`/`GtkActionGroup`/`GtkUIManager` and
/// `EUIAction`/`EUIActionGroup`/`EUIManager` at compile time. It uses the
/// `evolution_eui_manager` cfg flag. This file emits the flag. See `main`.
/// The two VMs this project targets sit on each side of this version
/// boundary.
const MIN_EVO_EUI_MANAGER: (u32, u32) = (3, 55);

/// Parses the major and minor numbers from a dotted version string. This
/// function never compares the patch level.
fn version_major_minor(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

/// This crate subclasses these classes, or reads fields from them. Bindgen
/// must generate their real layout. It must not leave them opaque.
///
/// `EMailConfigServiceBackend` backs the mail provider page.
/// `ESourceConfigBackend` backs the *New Address Book* and *New Calendar*
/// dialogs. It is the more load-bearing of the two classes: its
/// `class_init` writes four vfunc slots, and `tests/source_config.rs` checks
/// each one. `ESourceConfig` itself stays an [`EVO_HANDLES`] entry instead.
/// It is a `GtkBox`, and nothing here subclasses or allocates one.
///
/// `EConfigLookupWorker` is an interface this crate implements, for OAuth
/// 2.0 setup-time discovery. `jmap-config/src/oauth2_service.rs` uses the
/// same pattern for `EOAuth2Service`. Its vtable layout matters for this
/// reason. `EConfigLookup` does not join this allowlist: nothing here
/// subclasses it, so it stays an [`EVO_HANDLES`] entry. `EConfigLookupResult`
/// (and `EConfigLookupResultSimple`) is the worker's result type, built with
/// plain `g_object_new`. `EConfigLookupResultKind` is its constructor's enum
/// argument. Both facts come from reading `e-config-lookup-result-simple.c`
/// in EDS `master`, not from assumption.
const ALLOWED_TYPES: &[&str] = &[
    "EMailConfigServiceBackend.*",
    "EConfigLookupWorker.*",
    "EConfigLookupResult.*",
    // `e_config_lookup_get_source`'s argument type. It names which of the
    // assistant's sources a worker asks for. A plain C enum, like
    // `ECalClientSourceType`.
    "EConfigLookupSourceKind",
    "ESourceConfigBackend.*",
    // jmap-ui's vacation page implements this interface. `interface_init`
    // writes `submit` and `submit_finish` into its fields, so the layout
    // matters. This is an exact name, not a prefix. `EMailConfigPage` itself
    // stays opaque.
    "EMailConfigPageInterface",
    // `e_cal_source_config_get_source_type`'s return type. It is a plain C
    // enum. Unlike an object, a second generated copy cannot disagree in
    // layout.
    "ECalClientSourceType",
    // The EUIManager-era struct. jmap-ui builds an array of these by value
    // for `e_ui_manager_add_actions_with_eui_data`. It is present only when
    // Evolution's headers define it, from version 3.55 (see
    // `MIN_EVO_EUI_MANAGER`).
    "EUIActionEntry",
];

/// Two sets of C `#define` constants, generated as `&CStr`. The first set,
/// from `e-config-lookup.h`, names the parameters `EConfigLookupWorker::run`
/// reads its email address and server list under. The second set, from
/// `e-config-lookup-result.h`, lists result priorities. `run()` derives its
/// own priority from these.
const ALLOWED_VARS: &[&str] = &[
    "E_CONFIG_LOOKUP_PARAM_.*",
    "E_CONFIG_LOOKUP_RESULT_PRIORITY_.*",
];

/// The class's own accessors, plus the one function this crate calls on the
/// page it extends. This crate does not subclass the page, so it lists each
/// function by name instead of by prefix. The prefix
/// `e_mail_config_service_page_.*` would also pull in the page's scratch
/// sources, backend lookup, and auto-configuration functions.
const ALLOWED_FUNCTIONS: &[&str] = &[
    "e_mail_config_service_backend_.*",
    "e_mail_config_service_page_get_email_address",
    // `insert_widgets` calls this back on the page when an entry changes.
    // It asks the page to run `check_complete` again.
    "e_mail_config_page_changed",
    // `EConfigLookupWorker`'s own wrapper functions: `get_type`, the
    // display-name dispatch, and `run`.
    "e_config_lookup_worker_.*",
    // The `EConfigLookup` calls `JmapConfigLookup` needs. It registers
    // itself in `constructed()`, then hands back a result in `run()`.
    "e_config_lookup_get_type",
    "e_config_lookup_register_worker",
    "e_config_lookup_add_result",
    // The account a re-run lookup targets. `run()` seeds its probe from the
    // host that account already names.
    "e_config_lookup_get_source",
    // `EConfigLookupResultSimple`'s constructor, plus the setters `run()`
    // uses: a string setter for each `[Collection]`, `[Authentication]`,
    // `[Security]`, and `[JMAP OAuth2]` field; a boolean setter for the
    // three `*-enabled` flags; a uint setter for `[Authentication] port`.
    // No field of this worker's result is an int, double, enum, or
    // arbitrary value, so this list omits those setters.
    "e_config_lookup_result_simple_new",
    "e_config_lookup_result_simple_add_string",
    "e_config_lookup_result_simple_add_boolean",
    "e_config_lookup_result_simple_add_uint",
    // `ESourceConfigBackend`'s own wrappers: the class this crate subclasses.
    "e_source_config_backend_.*",
    // Only `get_type` is here. `tests/source_config.rs` uses it to confirm
    // that the [`EVO_HANDLES`] entry really is this `GtkBox`. The rest of
    // `ESourceConfig` can join this list once `insert_widgets` needs it.
    "e_source_config_get_type",
    // The two derived config classes. This crate uses the GType that
    // `class_init` writes into `EExtensionClass::extensible_type`, and one
    // vfunc call, `_get_source_type` (the whole content of `allow_creation`
    // on the calendar side). This list omits `_add_offline_toggle` and
    // `_add_autocomplete_check`: no vfunc here builds a widget.
    "e_book_source_config_get_type",
    "e_cal_source_config_get_type",
    "e_cal_source_config_get_source_type",
    // The notebook: its GType, a function to add a page, and the two
    // sources the vacation extension gates on (account) and connects with
    // (collection).
    "e_mail_config_notebook_get_type",
    "e_mail_config_notebook_add_page",
    "e_mail_config_notebook_get_account_source",
    "e_mail_config_notebook_get_collection_source",
    // The GType the vacation page registers itself as implementing.
    "e_mail_config_page_get_type",
    // The composer, for jmap-ui's scheduled send. This crate uses its
    // GType, the editor the UI merge goes through, the header table the
    // From gate reads, and the async function pair that builds the message
    // the same way Send itself does.
    "e_msg_composer_get_type",
    "e_msg_composer_get_editor",
    "e_msg_composer_get_header_table",
    "e_msg_composer_get_message",
    "e_msg_composer_get_message_finish",
    "e_html_editor_get_ui_manager",
    // This is for Evolution 3.52's composer path only. See `gtk_action_new`
    // below for 3.55 and later.
    "e_html_editor_get_action_group",
    // The From gate's registry, reached through the header table. The
    // header table has no registry accessor of its own (checked against
    // `e-composer-header-table.c`, which keeps an `EClientCache` instead).
    "e_composer_header_table_get_header",
    "e_composer_header_get_registry",
    // The identity the From line shows. This crate re-reads it on every
    // `notify::identity-uid` signal.
    "e_composer_header_table_dup_identity_uid",
    // The snooze surfaces. For the shell view: its GType, the name check
    // for "is this the mail view", and (on 3.52) the window that holds the
    // action and menu layer.
    "e_shell_view_get_type",
    "e_shell_view_get_name",
    "e_shell_view_get_shell_content",
    // Evolution 3.52's path to the composer/reader menu layer: the
    // window's own `GtkUIManager`, and its pre-existing "mail" action
    // group.
    "e_shell_view_get_shell_window",
    "e_shell_window_get_ui_manager",
    "e_shell_window_get_action_group",
    // Evolution 3.55 and later's path. The shell view answers for its own
    // `EUIManager` directly. Checked against
    // `e-rss-shell-view-extension.c`'s own use of it.
    "e_shell_view_get_ui_manager",
    // The registry the snooze gate resolves the folder's account source
    // against. The shell singleton is the one place certain to hold one.
    "e_shell_get_default",
    "e_shell_get_registry",
    // The detached message window: its own `GtkUIManager` (on 3.52), and
    // its own `EMailReader`.
    "e_mail_browser_get_type",
    "e_mail_browser_get_ui_manager",
    // The reader interface both surfaces hand the snooze action. This crate
    // reads the selection, its folder, and the action/UI group:
    // per-surface on 3.52, or one shared accessor on 3.55 and later.
    "e_mail_reader_get_selected_uids",
    "e_mail_reader_ref_folder",
    "e_mail_reader_get_action_group",
    "e_mail_reader_get_ui_manager",
    // The EUIManager-era call that registers a batch of actions and merges
    // `.eui` XML in one step. It needs no pre-existing action group and no
    // separate merge step. This crate then looks up the submenu's own
    // action, to set its sensitivity, tooltip, and label.
    "e_ui_manager_add_actions_with_eui_data",
    "e_ui_manager_get_action_group",
    "e_ui_action_group_get_action",
    "e_ui_action_set_sensitive",
    "e_ui_action_set_tooltip",
    "e_ui_action_set_label",
    // The vacation page's from/to fields. The key fact is "no date set":
    // this is how a nullable `fromDate`/`toDate` is expressed without the
    // page parsing typed text. This list is not `e_date_edit_.*`: a
    // date-only field leaves the class's time half, popup range, and
    // week-start setting untouched.
    "e_date_edit_get_type",
    "e_date_edit_new",
    "e_date_edit_get_date",
    "e_date_edit_set_date",
    "e_date_edit_set_show_time",
    "e_date_edit_set_allow_no_date_set",
    // Clears the field to "no date". This crate writes the `time_t` value
    // -1, the same value the getter returns, because there is no
    // `_set_date_to_none`.
    "e_date_edit_set_time",
    // The custom-time dialog's moment, as a `time_t` value (date and time
    // together).
    "e_date_edit_get_time",
    // The info bar after a collection delete: an alert with an Undo button and
    // a timer, shown in the content of the shell view.
    "e_alert_new",
    "e_alert_set_primary_text",
    "e_alert_add_widget",
    "e_alert_start_timer",
    "e_alert_response",
    "e_alert_get_type",
    "e_alert_sink_get_type",
    "e_alert_sink_submit_alert",
    // Only for `tests/alert.rs`: the shell content is the sink of the alert.
    "e_shell_content_get_type",
];

/// The GTK calls, named one at a time instead of by prefix. A prefix like
/// `gtk_(grid|label|entry)_.*` would pull in several hundred functions this
/// crate never calls. This list is the whole set M7's `insert_widgets`
/// needs to build its page: a grid in a box, with a right-aligned mnemonic
/// label beside an expanding entry for each setting. `tests/gtk.rs` names
/// each one back. Widget *property* bindings are not in this list
/// (`g_object_bind_property` on `CamelSettings`, already available from
/// gobject-sys): a `GtkEntry`'s `text` property needs no GTK call.
const ALLOWED_GTK_FUNCTIONS: &[&str] = &[
    // The container Evolution hands to `insert_widgets`, and the grid
    // placed in it.
    "gtk_box_pack_start",
    "gtk_grid_new",
    "gtk_grid_attach",
    "gtk_grid_set_row_spacing",
    "gtk_grid_set_column_spacing",
    // One label per setting. `_with_mnemonic` creates it with the
    // translated `_("_Server:")` shortcut. `set_mnemonic_widget` links it
    // to the entry beside it. `set_xalign` right-aligns it.
    "gtk_label_new_with_mnemonic",
    "gtk_label_set_mnemonic_widget",
    "gtk_label_set_xalign",
    "gtk_entry_new",
    "gtk_widget_set_hexpand",
    "gtk_widget_show_all",
    // The security toggle. It binds via property binding, like the
    // entries, so this crate needs no `gtk_toggle_button_.*` call.
    "gtk_check_button_new_with_mnemonic",
    // The status label. This crate fills it once, rewrites it on every
    // keystroke `on_extension_changed` reacts to, and hides it when there
    // is nothing to say.
    "gtk_label_new",
    "gtk_label_set_text",
    "gtk_widget_set_visible",
    // The authentication-method combo. Its choices are fixed: `"none"` and
    // this project's `EOAuth2Service` name. So `_append` is the only
    // mutator this crate needs.
    "gtk_combo_box_text_new",
    "gtk_combo_box_text_append",
    // The vacation page's multi-line body. Content goes through the
    // buffer's `text` property, so no `GtkTextIter` crosses the FFI
    // boundary.
    "gtk_text_view_new",
    "gtk_text_view_get_buffer",
    // The page is a `GtkScrolledWindow` subclass. Its one child is the
    // vertical box these two functions build and add. GTK3 wraps a
    // non-scrollable child in a viewport on its own.
    "gtk_scrolled_window_get_type",
    "gtk_box_new",
    "gtk_container_add",
    // A control for an absent or not-yet-known server feature is
    // desensitized, not hidden. Its tooltip says why.
    "gtk_widget_set_sensitive",
    "gtk_widget_set_tooltip_text",
    // The body takes the page's spare height. Everything else keeps its
    // natural size.
    "gtk_widget_set_vexpand",
    // Scheduled-send and snooze actions on Evolution before 3.55, the
    // GtkAction era (`evolution_eui_manager` picks the functions above on
    // 3.55 and later). This crate builds each action one at a time, not
    // through `gtk_action_group_add_actions`'s `GtkActionEntry` array: the
    // blocklist keeps that struct's layout out.
    "gtk_action_new",
    "gtk_action_group_add_action",
    "gtk_action_set_sensitive",
    "gtk_ui_manager_add_ui_from_string",
    "gtk_ui_manager_ensure_update",
    // The refusal dialog a failed scheduled send reports through, and its
    // teardown. This also closes a scheduled composer window on success.
    "gtk_message_dialog_new",
    "gtk_dialog_run",
    "gtk_widget_destroy",
    // The custom-time dialog. It is built via `_add_button`, not the
    // variadic `_new_with_buttons`.
    "gtk_dialog_new",
    "gtk_dialog_add_button",
    "gtk_dialog_get_content_area",
    "gtk_window_set_title",
    "gtk_window_set_transient_for",
    // One `activate` handler serves the whole submenu. It asks the action
    // which item fired. Labels are re-filled per gate, found back from the
    // group by name.
    "gtk_action_get_name",
    "gtk_action_group_get_action",
    // This module never calls these. `tests/gtk.rs` uses them to confirm
    // the opaque handles above really are these classes.
    "gtk_widget_get_type",
    "gtk_container_get_type",
    "gtk_box_get_type",
    "gtk_grid_get_type",
    "gtk_label_get_type",
    "gtk_entry_get_type",
    "gtk_check_button_get_type",
    "gtk_combo_box_text_get_type",
    "gtk_text_view_get_type",
    "gtk_text_buffer_get_type",
    // The Undo button of the info bar after a collection delete.
    "gtk_button_new_with_mnemonic",
    "gtk_button_get_type",
    "gtk_widget_show",
];

/// Types that already exist in eds-sys. This crate must not generate them a
/// second time: a regenerated `GObject` has the right layout but the wrong
/// identity. The `G*` entries are eds-sys's own blocklist, re-exported from
/// the gtk-rs sys crates. The `E*` and `Camel*` entries are
/// `EMailConfigServiceBackend`'s own `ESource`, `CamelProvider`, and
/// `CamelSettings` types. eds-sys generates these from the same headers and
/// re-exports them, so both crates share one type.
///
/// `ESourceConfigBackend` is the one `ESource*` name that is not eds-sys's.
/// [`blocked_source_types`] builds its exemption; this list does not name it
/// directly.
const BLOCKED_TYPES: &[&str] = &[
    "G[A-Z].*",
    "_G[A-Z].*",
    "g[a-z]+",
    "va_list",
    "__va_list_tag",
    "_?EExtension[A-Za-z]*",
    "Camel.*",
    "_Camel.*",
    // eds-sys already generates this type and allowlists it there. A
    // second copy here would disagree with every other EDS-facing crate's
    // copy.
    "_?ENamedParameters",
];

/// The one `ESource*` name eds-sys does not generate: Evolution's own
/// `ESourceConfigBackend`, from `libevolution-util`. This is a prefix, so
/// `*Class` and `*Private` are exempt too.
const EXEMPT_FROM_SOURCE_BLOCK: &str = "ESourceConfigBackend";

/// Builds the `ESource` half of `E(Extension|Source)[A-Za-z]*`, minus
/// [`EXEMPT_FROM_SOURCE_BLOCK`].
///
/// bindgen's blocklist overrides its allowlist: a name matched by both
/// compiles to nothing. bindgen also matches the blocklist regex against
/// the whole name, and the `regex` crate has no negative lookahead. So the
/// exemption cannot be written as one more allowlist entry.
///
/// Instead, this function emits two regex alternatives for every proper
/// prefix of the exempt name, starting from `ESource`: the prefix by
/// itself, and the prefix followed by any character other than the one the
/// exempt name continues with. The union of these alternatives matches
/// every `ESource*` name except names that begin with the exempt name
/// itself. `ESourceConfig` is one such name: it differs from
/// `ESourceConfigBackend` at the next character, so it stays blocked here,
/// and joins [`EVO_HANDLES`] instead (it is a `GtkBox`).
fn blocked_source_types() -> Vec<String> {
    let exempt = EXEMPT_FROM_SOURCE_BLOCK;
    let mut out = Vec::new();
    for len in "ESource".len()..exempt.len() {
        let prefix = &exempt[..len];
        let next = &exempt[len..len + 1];
        for underscore in ["", "_"] {
            out.push(format!("{underscore}{prefix}"));
            out.push(format!("{underscore}{prefix}[^{next}][A-Za-z]*"));
        }
    }
    out
}

/// GTK's *types*. This crate does not generate them, even though it now
/// calls GTK functions. No gtk-rs `gtk-sys` version is compatible with this
/// workspace's `glib-sys` 0.22: GTK 3's sys crate is frozen at the 0.18
/// generation. Depending on both would put two incompatible `GObject`
/// implementations in one process.
///
/// Instead, this crate generates the widget calls in
/// [`ALLOWED_GTK_FUNCTIONS`] from these same headers, and declares their
/// classes opaque with [`GTK_HANDLES`]. No GTK class here is subclassed or
/// allocated. So a generated layout would be a claim about a struct that
/// nothing reads a field of. Unlike the EDS structs `tests/layout.rs`
/// cross-checks, nothing would hold such a claim accountable.
const BLOCKED_GTK_TYPES: &[&str] = &["Gtk.*", "_Gtk.*"];

/// Evolution's own classes, reached only as a pointer. Nothing here
/// subclasses them, so a generated layout would be a claim about a struct
/// nobody reads a field of. `EMailConfigServicePage` is the clearest case:
/// it is a `GtkScrolledWindow` two classes down, so generating it would
/// pull in the GTK structs [`BLOCKED_GTK_TYPES`] keeps out. The backend
/// class next to it stays generated instead, because this crate does
/// subclass it, and `tests/layout.rs` checks its layout against
/// `g_type_query`.
const BLOCKED_EVO_TYPES: &[&str] = &[
    // Spelled exactly, with no wildcard tail, so `EMailConfigPageInterface`
    // stays generatable. That interface is the one piece of the page this
    // crate reads fields of.
    "_?EMailConfigPage",
    "_?EMailConfig(ActivityPage|ServicePage)[A-Za-z]*",
    // The account editor's notebook. It is a `GtkNotebook` two classes
    // down.
    "_?EMailConfigNotebook[A-Za-z]*",
    // The composer, and what its accessors return. The scheduled-send
    // extension reaches all of these only as pointers.
    "_?EMsgComposer[A-Za-z]*",
    "_?EHTMLEditor[A-Za-z]*",
    "_?EComposerHeaderTable[A-Za-z]*",
    // Spelled without a wildcard tail, so `EComposerHeaderType` (a value
    // type) stays generatable.
    "_?EComposerHeader",
    "_?EComposerHeader(Class|Private)",
    // The shell family and the two mail-window classes the snooze
    // extensions reach only as pointers. `EMailReader` appears twice,
    // spelled exactly, so `EMailReaderActionGroup` stays generatable.
    "_?EShell[A-Za-z]*",
    "_?EMailBrowser[A-Za-z]*",
    "_?EMailReader",
    "_?EMailReaderInterface",
    // The EUIManager era's own trio, from 3.55 and later (see
    // `MIN_EVO_EUI_MANAGER`), reached only as pointers. Spelled exactly, so
    // `EUIActionEntry` stays generatable: jmap-ui writes its fields.
    "_?EUIManager",
    "_?EUIAction",
    "_?EUIActionGroup",
    // A `GtkHBox` subclass.
    "_?EDateEdit[A-Za-z]*",
    // The alert and the interface of its sink, reached only as pointers.
    "_?EAlert[A-Za-z]*",
    // The object an `EConfigLookupWorker` registers against. No wildcard
    // tail, so `EConfigLookupWorker` and
    // `EConfigLookupResult`/`EConfigLookupResultSimple` stay generatable.
    "_?EConfigLookup",
    // `EBookSourceConfig` and `ECalSourceConfig`. Each has an
    // `ESourceConfig` (`GtkBox`) parent, so generating either would
    // mislocate every field after it. The calendar class is only ever the
    // pointer `e_source_config_backend_get_config` returns.
    "_?E(Book|Cal)SourceConfig[A-Za-z]*",
];

/// Evolution's classes reached only through a pointer, as opaque handles.
/// This crate gives them the same zero-sized `#[repr(C)]` treatment as
/// [`GTK_HANDLES`]: nothing here subclasses any of them, or reads a field of
/// one.
const EVO_HANDLES: &[&str] = &[
    // The subject of `tests/page.rs`. It is what
    // `e_mail_config_service_backend_get_page` and
    // `e_mail_config_service_page_get_email_address` work with.
    "EMailConfigServicePage",
    // The interface `insert_widgets` calls `e_mail_config_page_changed` on.
    // Every config page implements it, not only this provider's subclass.
    // See `e-mail-config-service-page.c`'s `G_IMPLEMENT_INTERFACE`, tag
    // 3.52.3.
    "EMailConfigPage",
    // What `EConfigLookupWorker::run` receives, and calls back onto.
    "EConfigLookup",
    // A `GtkBox`. `e_source_config_backend_get_config` hands it to a
    // backend's vfuncs, the same way the page is handed to the mail one.
    // `tests/source_config.rs` confirms this against the running type
    // system.
    "ESourceConfig",
    // The account editor's page container. It is a `GtkNotebook` two
    // classes down.
    "EMailConfigNotebook",
    // Both are `ESourceConfig`s, so both are `GtkBox`es.
    // `tests/source_config.rs` confirms they derive from it. That is the
    // one fact `allow_creation`'s `E_CAL_SOURCE_CONFIG()`-shaped cast
    // assumes.
    "EBookSourceConfig",
    "ECalSourceConfig",
    // The composer trio the scheduled-send extension reaches only as
    // pointers. This crate reads `identity-uid` and `registry` as plain
    // properties.
    "EMsgComposer",
    "EHTMLEditor",
    "EComposerHeaderTable",
    "EComposerHeader",
    // The snooze surfaces: the shell singleton, its view chain, the
    // detached message window, and the reader interface both surfaces
    // share.
    "EShell",
    "EShellView",
    "EShellWindow",
    "EShellContent",
    "EMailBrowser",
    "EMailReader",
    // The EUIManager-era menu/action trio, from 3.55 and later. Listing
    // them unconditionally is harmless: nothing constructs one on an older
    // Evolution.
    "EUIManager",
    "EUIAction",
    "EUIActionGroup",
    // The vacation page's date fields.
    "EDateEdit",
    // The info bar after a collection delete.
    "EAlert",
    "EAlertSink",
];

/// The GTK classes the calls above mention, as opaque handles. Each is one
/// zero-sized `#[repr(C)]` struct with a private field, so a pointer to any
/// of these is only ever handed straight back to GTK. `tests/gtk.rs`
/// confirms they stay zero-sized.
///
/// This crate uses separate types rather than one `c_void` alias. Passing a
/// `GtkBox` where `gtk_grid_attach` wants a `GtkGrid` is then a type error at
/// the `.cast()` call site, not undefined behaviour. `tests/gtk.rs` confirms
/// GTK really does relate the classes the way those casts assume.
const GTK_HANDLES: &[&str] = &[
    "GtkWidget",
    "GtkBox",
    "GtkGrid",
    "GtkLabel",
    "GtkEntry",
    "GtkComboBoxText",
    "GtkTextView",
    "GtkTextBuffer",
    "GtkContainer",
    // The GtkAction era's trio, still what 3.52's composer merges through.
    "GtkAction",
    "GtkActionGroup",
    "GtkUIManager",
    // The refusal dialog. This crate constructs, runs, and parents it
    // through three names for the same pointer.
    "GtkMessageDialog",
    "GtkDialog",
    "GtkWindow",
    // The Undo button of the info bar after a collection delete.
    "GtkButton",
];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    // Declares this cfg up front, regardless of which branch below fires.
    // As of the 2024 edition's lint, rustc warns (`unexpected_cfgs`) about a
    // custom cfg nobody declared. jmap-ui, a different crate than the one
    // that emits this cfg, asks about it.
    println!("cargo:rustc-check-cfg=cfg(evolution_eui_manager)");

    let mut clang_args = Vec::new();
    let mut link_paths: Vec<String> = Vec::new();
    let mut evo_version: Option<String> = None;
    for pkg in ["evolution-shell-3.0", "evolution-mail-3.0"] {
        let lib = pkg_config::Config::new()
            .atleast_version(MIN_EVO)
            .probe(pkg)
            .unwrap_or_else(|e| panic!("{pkg} >= {MIN_EVO} not found: {e}"));
        if evo_version.is_none() {
            evo_version = Some(lib.version.clone());
        }
        clang_args.extend(
            lib.include_paths
                .iter()
                .map(|p| format!("-I{}", p.display())),
        );
        // Evolution's libraries live in `/usr/lib/evolution`, not a system
        // directory. A linking binary needs its RUNPATH recorded, or it
        // will not start. Both `.pc` files carry `-Wl,-R<libdir>` for this.
        // `pkg_config` forwards it as a link argument.
        //
        // That forwarding stops at this crate. `rustc-link-arg` is scoped
        // to the crate that emits it, so a dependent crate links fine but
        // gets no RUNPATH. This crate republishes the directories as
        // build-script metadata, `DEP_EVOLUTION_SHELL_LIBDIRS`, through its
        // own `links` key. `jmap-config`'s own `build.rs` turns them back
        // into an rpath.
        //
        // This loop reads `lib.ld_args`, the actual `-Wl,...` arguments the
        // `.pc` files specify. It does not read `lib.link_paths`, every `-L`
        // that pkg-config's recursive `Requires:` resolution finds: that
        // list also included `/usr/lib` in the RUNPATH. lintian's
        // `custom-library-search-path` check flags `/usr/lib` regardless of
        // whether the directory is also a default one.
        for arg in &lib.ld_args {
            let mut words = arg.iter();
            while let Some(word) = words.next() {
                let dir = if let Some(dir) = word.strip_prefix("-R") {
                    Some(dir)
                } else if let Some(dir) = word.strip_prefix("-rpath=") {
                    Some(dir)
                } else if word == "-rpath" {
                    words.next().map(String::as_str)
                } else {
                    None
                };
                if let Some(dir) = dir.filter(|dir| !dir.is_empty()) {
                    let dir = dir.to_string();
                    if !link_paths.contains(&dir) {
                        link_paths.push(dir);
                    }
                }
            }
        }
    }

    // Colon-separated, like every other library path in the loader's world.
    println!("cargo:libdirs={}", link_paths.join(":"));

    // This decides which menu/action layer `wrapper.h`'s
    // `JMAP_EVO_EUI_MANAGER` picks between (see `MIN_EVO_EUI_MANAGER`).
    // jmap-ui's own `#[cfg]`s key off this value too.
    let eui_manager = evo_version
        .as_deref()
        .map(version_major_minor)
        .is_some_and(|version| version >= MIN_EVO_EUI_MANAGER);
    clang_args.push(format!("-DJMAP_EVO_EUI_MANAGER={}", u8::from(eui_manager)));
    if eui_manager {
        println!("cargo:rustc-cfg=evolution_eui_manager");
    }
    // `cargo:rustc-cfg` reaches only this crate's own compilation. jmap-ui
    // gets the same answer through `DEP_EVOLUTION_SHELL_EUI_MANAGER`, the
    // same `links` channel that carries `DEP_EVOLUTION_SHELL_LIBDIRS`.
    println!("cargo:eui_manager={}", u8::from(eui_manager));

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(clang_args)
        .clang_arg("-Wno-deprecated-declarations")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Reproducible builds, as in eds-sys.
        .generate_comments(false)
        .layout_tests(false)
        .derive_default(false)
        .prepend_enum_name(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .generate_cstr(true)
        .wrap_unsafe_ops(true)
        // This is one glob import, not four. eds-sys re-exports glib-sys,
        // gobject-sys, and gio-sys itself. Taking its whole namespace is
        // what makes an `ESource *` here the same Rust type as an `ESource
        // *` in the backends.
        .raw_line("pub use eds_sys::*;");

    // The GTK classes these headers mention, as opaque handles. See
    // `GTK_HANDLES`.
    for t in GTK_HANDLES {
        builder = builder.raw_line(format!(
            "#[repr(C)]\npub struct {t} {{\n    _opaque: [u8; 0],\n}}"
        ));
    }

    // Two GTK *value* types, supplied here rather than generated. `Gtk.*`
    // is blocklisted wholesale, but a plain C enum has no layout to
    // disagree about. `GtkAssistantPageType` is a field of
    // `EMailConfigPageInterface`; this crate never writes it.
    // `GtkOrientation` is what `gtk_box_new` takes.
    builder = builder
        .raw_line("pub type GtkAssistantPageType = ::std::os::raw::c_uint;")
        .raw_line("pub type GtkOrientation = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_ORIENTATION_VERTICAL: GtkOrientation = 1;")
        // `gtk_message_dialog_new`'s three enum arguments. This crate names
        // only the values the refusal dialog passes.
        .raw_line("pub type GtkDialogFlags = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_DIALOG_MODAL: GtkDialogFlags = 1;")
        .raw_line("pub const GTK_DIALOG_DESTROY_WITH_PARENT: GtkDialogFlags = 2;")
        .raw_line("pub type GtkMessageType = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_MESSAGE_ERROR: GtkMessageType = 3;")
        .raw_line("pub type GtkButtonsType = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_BUTTONS_CLOSE: GtkButtonsType = 2;")
        // `gtk_dialog_run`'s answer, for the custom-time dialog.
        .raw_line("pub const GTK_RESPONSE_OK: ::std::os::raw::c_int = -5;")
        .raw_line("pub const GTK_RESPONSE_CANCEL: ::std::os::raw::c_int = -6;")
        // The response that closes an alert in the info bar.
        .raw_line("pub const GTK_RESPONSE_CLOSE: ::std::os::raw::c_int = -7;");

    // Evolution's handles need the struct tag as well as the typedef. A
    // blocklisted `EMailConfigServicePage` comes through as
    // `_EMailConfigServicePage`, the header's own `typedef struct` spelling.
    // So this defines the handle under the tag, then aliases it to the
    // public name.
    for t in EVO_HANDLES {
        builder = builder.raw_line(format!(
            "#[repr(C)]\npub struct _{t} {{\n    _opaque: [u8; 0],\n}}\npub type {t} = _{t};"
        ));
    }

    for t in ALLOWED_TYPES {
        builder = builder.allowlist_type(t);
    }
    for f in ALLOWED_FUNCTIONS.iter().chain(ALLOWED_GTK_FUNCTIONS) {
        builder = builder.allowlist_function(f);
    }
    for v in ALLOWED_VARS {
        builder = builder.allowlist_var(v);
    }
    let source_types = blocked_source_types();
    for t in BLOCKED_TYPES
        .iter()
        .copied()
        .chain(BLOCKED_GTK_TYPES.iter().copied())
        .chain(BLOCKED_EVO_TYPES.iter().copied())
        .chain(source_types.iter().map(String::as_str))
    {
        builder = builder.blocklist_type(t);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed on the Evolution headers");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("could not write bindings.rs");
}
