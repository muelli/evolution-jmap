// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The same arrangement as eds-sys's build.rs, one library up: generated at
// build time rather than checked in, because Evolution's class structs change
// between releases and a stale committed binding is a silently-wrong ABI
// rather than a build failure. tests/layout.rs cross-checks the result against
// the running GObject type system.

use std::env;
use std::path::PathBuf;

/// Evolution 3.52 is the floor: 3.56 replaced GtkUIManager, so a lower floor
/// would claim support for a release this crate was never held against.
const MIN_EVO: &str = "3.52";

/// EUIManager replaced GtkUIManager in 3.55.1 (a 3.56 development release).
/// `jmap-ui` picks between `GtkAction`/`GtkActionGroup`/`GtkUIManager` and
/// `EUIAction`/`EUIActionGroup`/`EUIManager` at compile time via the
/// `evolution_eui_manager` cfg this file emits (see `main`), since the two
/// VMs this project targets sit one on each side of the boundary.
const MIN_EVO_EUI_MANAGER: (u32, u32) = (3, 55);

/// Parses a dotted version's major.minor; a patch level is never compared.
fn version_major_minor(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

/// Classes this crate subclasses or reads fields of, so their layout must be
/// generated rather than left opaque. `EMailConfigServiceBackend` backs the
/// mail provider page; `ESourceConfigBackend` backs the *New Address Book*/
/// *New Calendar* dialogs and is the more load-bearing of the two (four vfunc
/// slots our `class_init` writes, each pinned by `tests/source_config.rs`).
/// `ESourceConfig` itself stays a [`EVO_HANDLES`] entry instead: it is a
/// `GtkBox`, and nothing here subclasses or allocates one.
///
/// `EConfigLookupWorker` is an interface this crate implements (OAuth 2.0
/// setup-time discovery, the same pattern `jmap-config/src/oauth2_service.rs`
/// uses for `EOAuth2Service`), so its vtable layout matters too.
/// `EConfigLookup` does not join it: nothing here subclasses it, so it is an
/// [`EVO_HANDLES`] entry. `EConfigLookupResult(Simple)?` and
/// `EConfigLookupResultKind` are the worker's plain-`g_object_new` result
/// type and its constructor's enum argument; read against
/// `e-config-lookup-result-simple.c` (EDS `master`) rather than assumed.
const ALLOWED_TYPES: &[&str] = &[
    "EMailConfigServiceBackend.*",
    "EConfigLookupWorker.*",
    "EConfigLookupResult.*",
    "ESourceConfigBackend.*",
    // jmap-ui's vacation page implements this interface; `interface_init`
    // writes `submit`/`submit_finish` into its fields, so the layout matters.
    // Exact name, not a prefix: `EMailConfigPage` itself stays opaque.
    "EMailConfigPageInterface",
    // `e_cal_source_config_get_source_type`'s return type: a plain C enum, so
    // (unlike an object) a second generated copy cannot disagree in layout.
    "ECalClientSourceType",
    // The EUIManager-era struct jmap-ui builds an array of by value for
    // `e_ui_manager_add_actions_with_eui_data`. Present only when Evolution's
    // headers define it (`>= 3.55`, see `MIN_EVO_EUI_MANAGER`).
    "EUIActionEntry",
];

/// `e-config-lookup.h`'s `#define`d parameter names `EConfigLookupWorker::run`
/// reads its email address and server list under, generated as `&CStr`.
const ALLOWED_VARS: &[&str] = &["E_CONFIG_LOOKUP_PARAM_.*"];

/// The class's own accessors, and the one thing asked of the page it extends.
/// The page is not subclassed, so it is named a function at a time rather
/// than by prefix: `e_mail_config_service_page_.*` would also take on its
/// scratch sources, backend lookup and auto-configuration.
const ALLOWED_FUNCTIONS: &[&str] = &[
    "e_mail_config_service_backend_.*",
    "e_mail_config_service_page_get_email_address",
    // `insert_widgets`'s call back onto the page: an entry changed, so
    // `check_complete` is asked again.
    "e_mail_config_page_changed",
    // `EConfigLookupWorker`'s own wrappers: get_type, display-name dispatch,
    // and `run`.
    "e_config_lookup_worker_.*",
    // `EConfigLookup` calls `JmapConfigLookup` needs: register itself in
    // `constructed()`, hand back a result in `run()`.
    "e_config_lookup_get_type",
    "e_config_lookup_register_worker",
    "e_config_lookup_add_result",
    // `EConfigLookupResultSimple`'s constructor and the setters `run()` uses:
    // a string per `[Collection]`/`[Authentication]`/`[Security]`/
    // `[JMAP OAuth2]` field, a boolean for the three `*-enabled` flags, a
    // uint for `[Authentication] port`. No field of this worker's result is
    // an int, double, enum or arbitrary value, so those setters stay out.
    "e_config_lookup_result_simple_new",
    "e_config_lookup_result_simple_add_string",
    "e_config_lookup_result_simple_add_boolean",
    "e_config_lookup_result_simple_add_uint",
    // `ESourceConfigBackend`'s own wrappers: the class this crate subclasses.
    "e_source_config_backend_.*",
    // `get_type` only, so `tests/source_config.rs` can confirm the
    // [`EVO_HANDLES`] entry really is this `GtkBox`; the rest of
    // `ESourceConfig` joins this list as `insert_widgets` comes to need it.
    "e_source_config_get_type",
    // The two derived config classes: the GType `class_init` writes into
    // `EExtensionClass::extensible_type`, and the one vfunc call,
    // `_get_source_type` (`allow_creation`'s whole content on the calendar
    // side). Not `_add_offline_toggle`/`_add_autocomplete_check`: no vfunc
    // here builds a widget.
    "e_book_source_config_get_type",
    "e_cal_source_config_get_type",
    "e_cal_source_config_get_source_type",
    // The notebook: its GType, adding a page, and the two sources the
    // vacation extension gates on (account) and connects with (collection).
    "e_mail_config_notebook_get_type",
    "e_mail_config_notebook_add_page",
    "e_mail_config_notebook_get_account_source",
    "e_mail_config_notebook_get_collection_source",
    // The GType the vacation page registers as an implementer of.
    "e_mail_config_page_get_type",
    // The composer, for jmap-ui's scheduled send: its GType, the editor the
    // UI merge goes through, the header table the From gate reads, and the
    // async pair that builds the message the way Send itself would.
    "e_msg_composer_get_type",
    "e_msg_composer_get_editor",
    "e_msg_composer_get_header_table",
    "e_msg_composer_get_message",
    "e_msg_composer_get_message_finish",
    "e_html_editor_get_ui_manager",
    // 3.52's composer path only; see `gtk_action_new` below for 3.55+.
    "e_html_editor_get_action_group",
    // The From gate's registry, reached through the header table (which has
    // no registry accessor of its own; checked against
    // e-composer-header-table.c, which keeps an EClientCache instead).
    "e_composer_header_table_get_header",
    "e_composer_header_get_registry",
    // Which identity the From line stands at, re-read on every
    // `notify::identity-uid`.
    "e_composer_header_table_dup_identity_uid",
    // The snooze surfaces. The shell view: GType, the "is this the mail
    // view" name gate, and (3.52) the window the action/menu layer lives on.
    "e_shell_view_get_type",
    "e_shell_view_get_name",
    "e_shell_view_get_shell_content",
    // 3.52's path to the composer/reader menu layer: the window's own
    // GtkUIManager and one of its pre-existing action groups ("mail").
    "e_shell_view_get_shell_window",
    "e_shell_window_get_ui_manager",
    "e_shell_window_get_action_group",
    // 3.55+'s path: the shell view answers for its own EUIManager directly
    // (confirmed against `e-rss-shell-view-extension.c`'s own use of it).
    "e_shell_view_get_ui_manager",
    // The registry the snooze gate resolves the folder's account source
    // against; the shell singleton is the one place to be sure of one.
    "e_shell_get_default",
    "e_shell_get_registry",
    // The detached message window: its own GtkUIManager (3.52), and its own
    // EMailReader.
    "e_mail_browser_get_type",
    "e_mail_browser_get_ui_manager",
    // The reader face both surfaces hand the snooze action: the selection,
    // its folder, and (3.52, per-surface) or 3.55+ (one shared accessor) the
    // action/UI group.
    "e_mail_reader_get_selected_uids",
    "e_mail_reader_ref_folder",
    "e_mail_reader_get_action_group",
    "e_mail_reader_get_ui_manager",
    // EUIManager-era batch action registration plus `.eui` XML merge in one
    // call (no pre-existing action group, no separate merge step), and
    // finding the submenu's own action back afterwards to gate its
    // sensitivity, tooltip and label.
    "e_ui_manager_add_actions_with_eui_data",
    "e_ui_manager_get_action_group",
    "e_ui_action_group_get_action",
    "e_ui_action_set_sensitive",
    "e_ui_action_set_tooltip",
    "e_ui_action_set_label",
    // The vacation page's from/to fields. "No date set" is the load-bearing
    // part: how a nullable `fromDate`/`toDate` is expressed without the page
    // parsing typed text. Not `e_date_edit_.*`: the class's time half, popup
    // range and week-start setting are untouched by a date-only field.
    "e_date_edit_get_type",
    "e_date_edit_new",
    "e_date_edit_get_date",
    "e_date_edit_set_date",
    "e_date_edit_set_show_time",
    "e_date_edit_set_allow_no_date_set",
    // Clearing to "no date": spelled as the time_t -1 the getter answers with
    // (there is no `_set_date_to_none`).
    "e_date_edit_set_time",
    // The custom-time dialog's moment, as a time_t (date and time together).
    "e_date_edit_get_time",
];

/// The GTK calls, named one at a time rather than by prefix (e.g.
/// `gtk_(grid|label|entry)_.*`, which would pull in several hundred
/// functions nothing here exercises): the whole of what M7's
/// `insert_widgets` needs to build its page, a grid in a box with a
/// right-aligned mnemonic label beside an expanding entry per setting.
/// `tests/gtk.rs` names each one back. Widget *property* bindings
/// (`g_object_bind_property` on `CamelSettings`, already available from
/// gobject-sys) are not here: a `GtkEntry`'s `text` needs no GTK call.
const ALLOWED_GTK_FUNCTIONS: &[&str] = &[
    // The container Evolution hands `insert_widgets`, and the grid in it.
    "gtk_box_pack_start",
    "gtk_grid_new",
    "gtk_grid_attach",
    "gtk_grid_set_row_spacing",
    "gtk_grid_set_column_spacing",
    // A label per setting: `_with_mnemonic` for the translated `_("_Server:")`
    // shortcut, `set_mnemonic_widget` to land it on the entry beside it,
    // `set_xalign` to right-align.
    "gtk_label_new_with_mnemonic",
    "gtk_label_set_mnemonic_widget",
    "gtk_label_set_xalign",
    "gtk_entry_new",
    "gtk_widget_set_hexpand",
    "gtk_widget_show_all",
    // The security toggle; bound via property binding like the entries, so
    // no `gtk_toggle_button_.*` call is needed.
    "gtk_check_button_new_with_mnemonic",
    // The status label: filled once, then rewritten on every keystroke
    // `on_extension_changed` reacts to, and hidden while there is nothing to
    // say.
    "gtk_label_new",
    "gtk_label_set_text",
    "gtk_widget_set_visible",
    // The authentication-method combo: fixed choices (`"none"` and this
    // project's `EOAuth2Service` name), so `_append` is the only mutator
    // needed.
    "gtk_combo_box_text_new",
    "gtk_combo_box_text_append",
    // The vacation page's multi-line body: content goes through the
    // buffer's `text` property, so no `GtkTextIter` crosses the boundary.
    "gtk_text_view_new",
    "gtk_text_view_get_buffer",
    // The page is a GtkScrolledWindow subclass whose one child is the
    // vertical box these two build and add; GTK3 wraps a non-scrollable
    // child in a viewport on its own.
    "gtk_scrolled_window_get_type",
    "gtk_box_new",
    "gtk_container_add",
    // A control for an absent or not-yet-known server feature is
    // desensitized, never hidden, with a tooltip saying why.
    "gtk_widget_set_sensitive",
    "gtk_widget_set_tooltip_text",
    // The body takes the page's spare height; everything else keeps its
    // natural size.
    "gtk_widget_set_vexpand",
    // Scheduled-send and snooze actions on Evolution < 3.55 (the GtkAction
    // era; `evolution_eui_manager` picks the functions above on 3.55+).
    // Built one at a time, not via `gtk_action_group_add_actions`'s
    // `GtkActionEntry` array, a struct layout the blocklist keeps out.
    "gtk_action_new",
    "gtk_action_group_add_action",
    "gtk_action_set_sensitive",
    "gtk_ui_manager_add_ui_from_string",
    "gtk_ui_manager_ensure_update",
    // The refusal dialog a failed scheduled send is reported through, and
    // its teardown; also closes a scheduled composer window on success.
    "gtk_message_dialog_new",
    "gtk_dialog_run",
    "gtk_widget_destroy",
    // The custom-time dialog, built via `_add_button` rather than the
    // variadic `_new_with_buttons`.
    "gtk_dialog_new",
    "gtk_dialog_add_button",
    "gtk_dialog_get_content_area",
    "gtk_window_set_title",
    "gtk_window_set_transient_for",
    // One `activate` handler serves the whole submenu, so it asks the
    // action which item it is; labels are re-filled per gate, found back out
    // of the group by name.
    "gtk_action_get_name",
    "gtk_action_group_get_action",
    // Not called by the module: `tests/gtk.rs` uses these to confirm the
    // opaque handles above really are these classes.
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
];

/// Types that already exist in eds-sys and must not be minted a second time:
/// a regenerated `GObject` has the right layout and the wrong identity. The
/// `G*` spellings are eds-sys's own blocklist, re-exported from the gtk-rs
/// sys crates; the `E*`/`Camel*` ones are `EMailConfigServiceBackend`'s own
/// `ESource`/`CamelProvider`/`CamelSettings`, generated by eds-sys from the
/// same headers and re-exported from there, so both crates share one type.
///
/// `ESourceConfigBackend` is the one `ESource*` name that is *not* eds-sys's;
/// its exemption is built by [`blocked_source_types`] rather than listed
/// here.
const BLOCKED_TYPES: &[&str] = &[
    "G[A-Z].*",
    "_G[A-Z].*",
    "g[a-z]+",
    "va_list",
    "__va_list_tag",
    "_?EExtension[A-Za-z]*",
    "Camel.*",
    "_Camel.*",
    // eds-sys already generates this one (allowlisted there); a second copy
    // here would disagree with every other EDS-facing crate's.
    "_?ENamedParameters",
];

/// The one `ESource*` name eds-sys does not generate: Evolution's own
/// `ESourceConfigBackend` (`libevolution-util`). A prefix, so
/// `*Class`/`*Private` are exempt too.
const EXEMPT_FROM_SOURCE_BLOCK: &str = "ESourceConfigBackend";

/// `E(Extension|Source)[A-Za-z]*`'s `ESource` half, minus
/// [`EXEMPT_FROM_SOURCE_BLOCK`].
///
/// bindgen's blocklist beats its allowlist (a name in both compiles to
/// nothing), and it matches a whole name against `regex`, which has no
/// negative lookahead, so the exemption cannot be one more allowlist entry.
/// Instead, for every proper prefix of the exempt name from `ESource`
/// onwards, this emits two alternatives: the prefix exactly, and the prefix
/// followed by any character *other* than the one the exempt name continues
/// with. Their union is every `ESource*` name except ones beginning with the
/// exempt name itself - `ESourceConfig` included, which differs at the next
/// character and so stays blocked (and joins [`EVO_HANDLES`], being a
/// `GtkBox`).
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

/// GTK's *types*, not generated even though this crate now calls GTK
/// functions: there is no gtk-rs `gtk-sys` compatible with this workspace's
/// `glib-sys` 0.22 (GTK 3's sys crate is frozen at the 0.18 generation), so
/// depending on both would put two incompatible `GObject`s in one process.
/// The widget calls in [`ALLOWED_GTK_FUNCTIONS`] are instead generated from
/// these same headers, with their classes declared opaque by [`GTK_HANDLES`]:
/// no GTK class here is subclassed or allocated, so a generated layout would
/// be a claim about a struct nothing reads a field of, and nothing (unlike
/// the EDS structs `tests/layout.rs` cross-checks) would hold it accountable.
const BLOCKED_GTK_TYPES: &[&str] = &["Gtk.*", "_Gtk.*"];

/// Evolution's own classes reached only as a pointer: nothing here
/// subclasses them, so a generated layout would be a claim about a struct
/// nobody reads a field of. `EMailConfigServicePage` is the clearest case (a
/// `GtkScrolledWindow` two classes down, so generating it would pull in the
/// GTK structs [`BLOCKED_GTK_TYPES`] keeps out); the backend class next door
/// stays generated instead, since it *is* subclassed here and
/// `tests/layout.rs` holds its layout against `g_type_query`.
const BLOCKED_EVO_TYPES: &[&str] = &[
    // Spelled exactly (no wildcard tail) so `EMailConfigPageInterface` (the
    // one piece of the page this crate reads fields of) stays generatable.
    "_?EMailConfigPage",
    "_?EMailConfig(ActivityPage|ServicePage)[A-Za-z]*",
    // The account editor's notebook: a GtkNotebook two classes down.
    "_?EMailConfigNotebook[A-Za-z]*",
    // The composer and what its accessors hand back, all reached only as
    // pointers by the scheduled-send extension.
    "_?EMsgComposer[A-Za-z]*",
    "_?EHTMLEditor[A-Za-z]*",
    "_?EComposerHeaderTable[A-Za-z]*",
    // Spelled without a tail so `EComposerHeaderType` (a value type) stays
    // generatable.
    "_?EComposerHeader",
    "_?EComposerHeader(Class|Private)",
    // The shell family and the two mail-window classes the snooze extensions
    // reach only as pointers. `EMailReader` spelled exactly, twice, so
    // `EMailReaderActionGroup` stays generatable.
    "_?EShell[A-Za-z]*",
    "_?EMailBrowser[A-Za-z]*",
    "_?EMailReader",
    "_?EMailReaderInterface",
    // The EUIManager era's own trio (3.55+; see `MIN_EVO_EUI_MANAGER`),
    // reached only as pointers. Spelled exactly so `EUIActionEntry`, whose
    // fields jmap-ui writes, stays generatable.
    "_?EUIManager",
    "_?EUIAction",
    "_?EUIActionGroup",
    // A GtkHBox subclass.
    "_?EDateEdit[A-Za-z]*",
    // The object an `EConfigLookupWorker` is registered against. No
    // wildcard tail, so `EConfigLookupWorker`/`EConfigLookupResult(Simple)?`
    // stay generatable.
    "_?EConfigLookup",
    // `EBookSourceConfig`/`ECalSourceConfig`: each has an `ESourceConfig`
    // (`GtkBox`) parent, so generating either would mislocate every field
    // after it. The calendar one is only ever the pointer
    // `e_source_config_backend_get_config` hands back.
    "_?E(Book|Cal)SourceConfig[A-Za-z]*",
];

/// Evolution's classes reached only through a pointer, as opaque handles,
/// the same zero-sized `#[repr(C)]` treatment as [`GTK_HANDLES`]: nothing
/// here subclasses any of them or reads a field of one.
const EVO_HANDLES: &[&str] = &[
    // `tests/page.rs`'s subject: what `e_mail_config_service_backend_get_page`
    // and `e_mail_config_service_page_get_email_address` speak of.
    "EMailConfigServicePage",
    // The interface `insert_widgets` calls `e_mail_config_page_changed` on;
    // every config page implements it, not just this provider's subclass
    // (`e-mail-config-service-page.c`'s `G_IMPLEMENT_INTERFACE`, tag 3.52.3).
    "EMailConfigPage",
    // What `EConfigLookupWorker::run` receives and calls back onto.
    "EConfigLookup",
    // A `GtkBox`; `e_source_config_backend_get_config` hands it to a
    // backend's vfuncs, the way the page is handed to the mail one.
    // `tests/source_config.rs` confirms it against the running type system.
    "ESourceConfig",
    // The account editor's page container; a GtkNotebook two classes down.
    "EMailConfigNotebook",
    // Both `ESourceConfig`s (so `GtkBox`es); `tests/source_config.rs`
    // confirms they derive from it, the one thing `allow_creation`'s
    // `E_CAL_SOURCE_CONFIG()`-shaped cast assumes.
    "EBookSourceConfig",
    "ECalSourceConfig",
    // The composer trio the scheduled-send extension reaches only as
    // pointers; `identity-uid`/`registry` are read as plain properties.
    "EMsgComposer",
    "EHTMLEditor",
    "EComposerHeaderTable",
    "EComposerHeader",
    // The snooze surfaces: the shell singleton, its view chain, the
    // detached message window, and the reader face both share.
    "EShell",
    "EShellView",
    "EShellWindow",
    "EShellContent",
    "EMailBrowser",
    "EMailReader",
    // The EUIManager-era menu/action trio (3.55+); harmless to list
    // unconditionally, since nothing constructs one on an older Evolution.
    "EUIManager",
    "EUIAction",
    "EUIActionGroup",
    // The vacation page's date fields.
    "EDateEdit",
];

/// The GTK classes the calls above mention, as opaque handles: one
/// `#[repr(C)]` zero-sized struct each, with a private field, so a pointer
/// to any of these is only ever handed straight back to GTK. `tests/gtk.rs`
/// confirms they stay zero-sized.
///
/// Separate types rather than one `c_void` alias, so that passing a
/// `GtkBox` where `gtk_grid_attach` wants a `GtkGrid` is a type error at the
/// `.cast()` call site rather than undefined behaviour; `tests/gtk.rs`
/// confirms GTK really does relate the classes the way those casts assume.
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
    // The refusal dialog: constructed, run and parented through three names
    // for the same pointer.
    "GtkMessageDialog",
    "GtkDialog",
    "GtkWindow",
];

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    // Declared up front regardless of which branch below fires: a custom cfg
    // rustc has not seen asked-about is a warning (`unexpected_cfgs`) as of
    // the 2024 edition's lint, and this one is asked about from jmap-ui, a
    // different crate than the one emitting it.
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
        // directory, so a linking binary needs its RUNPATH recorded or it
        // will not start; both `.pc` files carry `-Wl,-R<libdir>` for this,
        // forwarded by `pkg_config` as a link argument.
        //
        // That forwarding stops at this crate: `rustc-link-arg` is scoped to
        // the crate that emitted it, so a dependent links fine but gets no
        // RUNPATH. The directories are republished as build-script metadata
        // (`DEP_EVOLUTION_SHELL_LIBDIRS`, via this crate's `links` key), and
        // `jmap-config`'s own `build.rs` turns them back into an rpath.
        //
        // Read from `lib.ld_args` (the actual `-Wl,...` the `.pc` files spell
        // out), not `lib.link_paths` (every `-L` pkg-config's recursive
        // `Requires:` resolution turns up): that came out with `/usr/lib` in
        // the RUNPATH too, which lintian's `custom-library-search-path`
        // flags regardless of whether the directory is also a default one.
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

    // Colon-separated, as every other library path in the loader's world is.
    println!("cargo:libdirs={}", link_paths.join(":"));

    // Which menu/action layer `wrapper.h`'s `JMAP_EVO_EUI_MANAGER` picks
    // between (see `MIN_EVO_EUI_MANAGER`), and what jmap-ui's own `#[cfg]`s
    // key off.
    let eui_manager = evo_version
        .as_deref()
        .map(version_major_minor)
        .is_some_and(|version| version >= MIN_EVO_EUI_MANAGER);
    clang_args.push(format!("-DJMAP_EVO_EUI_MANAGER={}", u8::from(eui_manager)));
    if eui_manager {
        println!("cargo:rustc-cfg=evolution_eui_manager");
    }
    // `cargo:rustc-cfg` only reaches this crate's own compilation; jmap-ui
    // gets the same answer via `DEP_EVOLUTION_SHELL_EUI_MANAGER`, the same
    // `links` channel that carries `DEP_EVOLUTION_SHELL_LIBDIRS`.
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
        // One glob, not four: eds-sys re-exports glib-sys, gobject-sys and
        // gio-sys itself, so taking its namespace whole is what makes an
        // `ESource *` here the same Rust type as an `ESource *` in the
        // backends.
        .raw_line("pub use eds_sys::*;");

    // The GTK classes these headers mention, as opaque handles; see GTK_HANDLES.
    for t in GTK_HANDLES {
        builder = builder.raw_line(format!(
            "#[repr(C)]\npub struct {t} {{\n    _opaque: [u8; 0],\n}}"
        ));
    }

    // Two GTK *value* types, supplied rather than generated (`Gtk.*` is
    // blocklisted wholesale, but a plain C enum has no layout to disagree
    // about). `GtkAssistantPageType` is a field of `EMailConfigPageInterface`,
    // never written here. `GtkOrientation` is what `gtk_box_new` takes.
    builder = builder
        .raw_line("pub type GtkAssistantPageType = ::std::os::raw::c_uint;")
        .raw_line("pub type GtkOrientation = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_ORIENTATION_VERTICAL: GtkOrientation = 1;")
        // `gtk_message_dialog_new`'s three enum arguments; only the values
        // the refusal dialog passes are named.
        .raw_line("pub type GtkDialogFlags = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_DIALOG_MODAL: GtkDialogFlags = 1;")
        .raw_line("pub const GTK_DIALOG_DESTROY_WITH_PARENT: GtkDialogFlags = 2;")
        .raw_line("pub type GtkMessageType = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_MESSAGE_ERROR: GtkMessageType = 3;")
        .raw_line("pub type GtkButtonsType = ::std::os::raw::c_uint;")
        .raw_line("pub const GTK_BUTTONS_CLOSE: GtkButtonsType = 2;")
        // `gtk_dialog_run`'s answer, for the custom-time dialog.
        .raw_line("pub const GTK_RESPONSE_OK: ::std::os::raw::c_int = -5;")
        .raw_line("pub const GTK_RESPONSE_CANCEL: ::std::os::raw::c_int = -6;");

    // Evolution's handles need the struct tag as well as the typedef: a
    // blocklisted `EMailConfigServicePage` comes through as
    // `_EMailConfigServicePage`, the header's own `typedef struct` spelling,
    // so the handle is defined under the tag and aliased to the public name.
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
