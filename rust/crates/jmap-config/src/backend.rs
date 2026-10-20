// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EMailConfigServiceBackend` subclass: the object that the account editor
//! of Evolution uses.
//!
//! The *Receiving Email* page of Evolution is an `EExtensible`. Each mail
//! provider that has a part in account setup registers an `EExtension` of this
//! class against it. The page makes one instance per registered backend. It
//! compares the [`backend_name`][name] of each with the Camel providers it
//! knows, and puts the matches in the provider combo. After that, each step of
//! the setup is a vfunc on this class: the initial account, the widgets of the
//! server page, the sensitivity of *Next*, and the data that a commit writes.
//!
//! [name]: crate::mail::MAIL_BACKEND_NAME
//!
//! The other modules of this crate keep this file thin. Each vfunc body uses a
//! decision that [`account`](crate::account), [`mail`](crate::mail),
//! [`defaults`](crate::defaults) or [`complete`](crate::complete) makes, and
//! tests, over a plain `ESource`. This file is the GObject that calls them.
//!
//! ## What is installed, and what is deliberately left inherited
//!
//! - **`backend_name`**: not a vfunc, but the field that the page *finds* this
//!   backend by. If it is NULL, there is no error, but JMAP never appears in
//!   the account type list.
//! - **`new_collection`**: see `new_collection` below. The Evolution default
//!   returns NULL. That is correct for POP3 and wrong for a provider with
//!   child sources.
//! - **`setup_defaults`**: see `setup_defaults` below. The Evolution default is
//!   an empty function (read from the installed library, not assumed). With it,
//!   the server settings page opens empty, although the assistant knows the
//!   address.
//! - **`check_complete`**: see `check_complete` below. The Evolution default is
//!   `return TRUE` (read from the installed library, not assumed). With it,
//!   *Next* is sensitive for an account with no address and no server.
//! - **`commit_changes`**: see `commit_changes` below. The Evolution default
//!   does nothing. That is correct for a provider whose server the user types
//!   on this page. It is wrong for a provider whose server is on the account:
//!   the mail source then has a protocol and no host.
//! - **`insert_widgets`**: see `insert_widgets` below. The Evolution default is
//!   an empty function. With it, the server settings page has no widgets to
//!   correct a wrong offer.
//! - **`get_selectable`** is deliberately not overridden. The default returns
//!   "yes, but only on the receiving page if this provider is both a store and
//!   a transport". The JMAP provider *is* both
//!   ([`jmap_mail`'s `PROTOCOL`][protocol] registers a store type and a
//!   transport type), thus the inherited answer is correct. An unconditional
//!   `TRUE` would show JMAP again in the *Sending Email* combo, as an account
//!   type that the user can select but not configure.
//!
//! [protocol]: ../../jmap_mail/provider/constant.PROTOCOL.html
//!
//! ## What is not here yet
//!
//! `insert_widgets` builds each field that
//! [`Connection`](jmap_collection_sync::child_source::Connection) contains: the
//! server, the port, the login name, the authentication method and the
//! encryption setting. Each widget is bound to the collection that
//! `check_complete` and `commit_changes` read and write. A status label shows
//! the refusal reason of [`Incomplete`](crate::complete::Incomplete) (empty and
//! hidden when a commit accepts the account).
//! Only **the consent browser round trip** is missing. The authentication combo
//! lets a user *select* OAuth 2.0 for an account. It does not prove that the
//! `ECredentialsPrompterImplOAuth2` of EDS reaches a real provider and returns
//! for the registered client of this project. That test needs a human and a
//! real deployment. The docs of [`config_lookup`](crate::config_lookup) identify
//! the same gap for their part of the OAuth 2.0 setup.
//!
//! ## The resulting state of the dialog
//!
//! For an account whose address the assistant knows, the server settings page
//! opens with data: the address, the server that its domain implies, and the
//! offered login name. The page has three entries, an authentication-method
//! combo and a check button to correct these values. They are bound live to
//! the account that `check_complete` and `commit_changes` read. A label below
//! them tells why *Next* is not sensitive, when it is not.
//!
//! [`evo-sys`]: ../../evo_sys/index.html

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ESource,
    e_binding_bind_property, e_binding_bind_property_full, e_source_authentication_get_type,
    e_source_get_extension, e_source_new, e_source_security_get_type,
};
use evo_sys::{
    E_CONFIG_LOOKUP_RESULT_COLLECTION, EConfigLookup, EMailConfigPage, EMailConfigServiceBackend,
    EMailConfigServiceBackendClass, EMailConfigServicePage, GtkBox, GtkWidget,
    e_mail_config_page_changed, e_mail_config_service_backend_auto_configure_for_kind,
    e_mail_config_service_backend_get_collection, e_mail_config_service_backend_get_page,
    e_mail_config_service_backend_get_source, e_mail_config_service_backend_get_type,
    e_mail_config_service_page_get_email_address, gtk_box_pack_start,
    gtk_check_button_new_with_mnemonic, gtk_combo_box_text_append, gtk_combo_box_text_new,
    gtk_entry_new, gtk_grid_attach, gtk_grid_new, gtk_grid_set_column_spacing,
    gtk_grid_set_row_spacing, gtk_label_new, gtk_label_new_with_mnemonic,
    gtk_label_set_mnemonic_widget, gtk_label_set_text, gtk_label_set_xalign,
    gtk_widget_set_hexpand, gtk_widget_set_visible, gtk_widget_show_all,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_free, gboolean, gint, gpointer};
use gobject_sys::{
    G_BINDING_BIDIRECTIONAL, G_BINDING_SYNC_CREATE, G_CONNECT_DEFAULT, GBinding, GObject,
    GParamSpec, GValue, g_object_get_data, g_object_set_data, g_signal_connect_object,
    g_value_get_string, g_value_get_uint, g_value_set_string, g_value_set_uint,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate};
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::account::{apply, read};
use crate::complete::{check, status_message};
use crate::defaults::from_identity;
use crate::mail::{MAIL_BACKEND_NAME, apply_server};

/// The JMAP account setup backend.
#[repr(C)]
pub struct JmapConfigServiceBackend {
    /// Owned by Evolution. This code never reads it; it only returns it as the
    /// instance pointer.
    parent: EMailConfigServiceBackend,
}

/// The class struct. It has no fields of this crate. GObject needs it for the
/// allocation size, the name and the vfunc slots.
#[repr(C)]
pub struct JmapConfigServiceBackendClass {
    pub parent_class: EMailConfigServiceBackendClass,
}

impl JmapConfigServiceBackend {
    /// An instance outside the GObject type system: zeroed parent bytes, as
    /// `instance_init` leaves them, but with no GObject.
    ///
    /// As in [`jmap-backend-collection`][detached], a real instance is not
    /// possible here. The `constructed` of Evolution extends an
    /// `EMailConfigServicePage`, which is a `GtkWidget` and needs a display.
    /// This VM has no display. Do not use the result for anything: the parent
    /// bytes are a valid bit pattern but not a GObject. Thus, to give it to an
    /// Evolution function is undefined behaviour.
    ///
    /// It is sound for only one vfunc of this class: `new_collection`, whose
    /// answer depends on no input. `tests/backend.rs` uses it for that vfunc.
    ///
    /// [detached]: ../../jmap_backend_collection/backend/struct.JmapCollectionBackend.html#method.detached
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: each field of the parent is a pointer or an integer, and
        // all-zero is a valid value for these.
        unsafe { zeroed_box() }
    }
}

// SAFETY: both structs are #[repr(C)] and start with the
// EMailConfigServiceBackend instance struct and class struct respectively, and
// EMailConfigServiceBackend derives from GObject (through EExtension).
unsafe impl ObjectSubclass for JmapConfigServiceBackend {
    const NAME: &'static CStr = c"EMailConfigServiceBackendJmap";
    type Instance = JmapConfigServiceBackend;
    type Class = JmapConfigServiceBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe { e_mail_config_service_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a newly allocated class struct of this
        // crate, which starts with the parent's; both fields below are in it.
        let class = unsafe { &mut (*class).parent_class };
        // The only field that is not a function, and the page reads it before
        // it calls a vfunc. The page compares it (`strcmp`) with the protocol of
        // each Camel provider, thus it must be *the provider's* spelling, not a
        // description. It is a `'static` pointer, as the field requires:
        // Evolution keeps the class for the life of the process and never
        // frees this.
        class.backend_name = MAIL_BACKEND_NAME.as_ptr();
        class.new_collection = Some(new_collection);
        class.insert_widgets = Some(insert_widgets);
        class.setup_defaults = Some(setup_defaults);
        class.check_complete = Some(check_complete);
        class.commit_changes = Some(commit_changes);
        class.auto_configure = Some(auto_configure);
    }
}

/// The vfunc that "Look Up Account Details" of the assistant calls on each
/// registered backend after the `EConfigLookup` workers finish.
/// `e_mail_config_service_page_auto_configure` gets the answer of each backend
/// and selects the lowest-priority TRUE as the active provider of the page.
///
/// The parent implementation always returns FALSE. Without this override, the
/// wizard never offers JMAP, whatever the worker of
/// [`config_lookup`](crate::config_lookup) found. Live test against Fastmail
/// (2026-08-23): SRV, RFC 8414 discovery and RFC 7591 registration were all
/// successful, the COLLECTION result was added and complete, but the combo
/// showed only imapx/smtp.
///
/// The body is the same one-line call as in `evolution-ews`
/// (`mail_config_ews_backend_auto_configure`). It gives the
/// `auto_configure_for_kind` helper of Evolution the COLLECTION kind, a NULL
/// protocol and the collection source of the backend. With a NULL protocol,
/// the helper compares the results with `class.backend_name` ("jmap", the value
/// in the worker's results). The helper writes the winning result onto the
/// collection source.
///
/// FALSE on a panic, which is the safe direction: the assistant uses the offer
/// of a different backend and not a partly applied configuration.
unsafe extern "C" fn auto_configure(
    backend: *mut EMailConfigServiceBackend,
    config_lookup: *mut EConfigLookup,
    out_priority: *mut gint,
    out_is_complete: *mut gboolean,
) -> gboolean {
    guard(
        "JmapConfigServiceBackend::auto_configure",
        GFALSE,
        || unsafe {
            e_mail_config_service_backend_auto_configure_for_kind(
                backend,
                config_lookup,
                E_CONFIG_LOOKUP_RESULT_COLLECTION,
                ptr::null(),
                e_mail_config_service_backend_get_collection(backend),
                out_priority,
                out_is_complete,
            )
        },
    )
}

/// Evolution calls this once, from `constructed`, to learn what an account of
/// this type *is*. For a groupware provider, the answer is also the source
/// that all other parts of the account attach to.
///
/// The answer is `(transfer full)`. Evolution keeps it as the `collection`
/// property of the backend and releases the reference with the backend. When
/// the user *edits* an account, Evolution discards it immediately: the page
/// replaces the property with the source of the existing account. Thus this
/// applies only to a new account.
///
/// ## Why the full default account and not only the backend name
///
/// evolution-ews writes one property here (the collection backend name) and
/// lets `setup_defaults` write the rest. This function writes all of
/// [`from_identity("")`](crate::defaults::from_identity): the same account,
/// but with an empty value in the one field that needs an address.
///
/// The reason: `setup_defaults` possibly has not run when something reads this
/// source, and a missing field is not neutral. `[Collection] MailEnabled` and
/// its two related keys are *false* when not written. A collection with only a
/// backend name thus reads back as a JMAP account with mail, contacts and
/// calendars all off. `tests/backend.rs` checks this through the reader of the
/// registry. The user did not ask for that account and the dialog does not
/// show it. The only symptom is an account with no children.
///
/// Thus, from its creation, the source is the initial account of the dialog:
/// all three parts on, TLS on, and no user or server yet.
/// `setup_defaults` narrows it to the address that the user typed. Until then,
/// [`complete::check`](crate::complete::check) refuses this account for
/// the missing field.
///
/// ## Failure
///
/// NULL. The Evolution default returns NULL and thus Evolution handles it: the
/// backend has no collection. The result is bad (an account committed as a
/// mail source only), but the vfunc has no `GError` and no other way to tell
/// this. Thus both paths to NULL log a critical: a panic, which the guard
/// catches, and a failed `e_source_new`.
unsafe extern "C" fn new_collection(backend: *mut EMailConfigServiceBackend) -> *mut ESource {
    // Not read. Unlike EWS, this implementation takes the backend name from the
    // constant that initialised the class, not from
    // `GET_CLASS (backend)->backend_name`. Thus a test can call it without an
    // instance. `tests/backend.rs` asserts that the two strings are equal.
    let _ = backend;

    guard("new_collection", ptr::null_mut(), || {
        let mut error = ptr::null_mut();
        // A scratch source with no D-Bus object: a local `ESource` with a
        // generated uid. The account editor of Evolution uses it until the
        // registry creates the real source.
        // SAFETY: the documented arguments: a NULL `GDBusObject`, the default
        // main context, and a `GError` out-parameter.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        if source.is_null() {
            // SAFETY: the out-parameter of a call that failed, thus it is NULL
            // or a `GError` that this caller owns; it is consumed only once.
            let reason = unsafe { take_message(error) };
            log_critical(&format!(
                "new_collection: could not create the account source: {reason}"
            ));
            return ptr::null_mut();
        }

        // SAFETY: the source is new and nothing else holds it.
        unsafe { apply(source, &from_identity("")) };
        source
    })
}

/// Tells if an [`ENTRY_ROWS`] row is bound directly or through a transform.
///
/// [`Text`](RowKind::Text) is for `host` and `user`: the `text` of the entry
/// and the `ESourceAuthentication` property are both strings, thus
/// [`e_binding_bind_property`] binds them with no transform function.
/// `port` is different: a `GtkEntry` has no integer property for a `guint16`.
/// Thus it uses [`e_binding_bind_property_full`] with
/// [`port_to_text`]/[`text_to_port`].
enum RowKind {
    Text,
    Port,
}

/// The `[Authentication]` fields that the user can correct in this dialog, in
/// page order. Each row has:
///
/// - the translatable text of the mnemonic label (marked with [`N_`] and
///   looked up with [`translate`]: a `GtkLabel` copies the string, thus no
///   per-call pointer must stay alive, which is the purpose of
///   [`i18n::translate_static`](jmap_backend_core::i18n::translate_static)),
/// - the `ESourceAuthentication` property that the entry is bound to,
/// - the type of binding.
const ENTRY_ROWS: [(&CStr, &CStr, RowKind); 3] = [
    (N_(c"_Server:"), c"host", RowKind::Text),
    (N_(c"_Port:"), c"port", RowKind::Port),
    (N_(c"_Username:"), c"user", RowKind::Text),
];

/// The mnemonic label of the security check button. It is not a fourth
/// [`ENTRY_ROWS`] row: it is one check button, not a label and an entry, and
/// it binds to `[Security]`, not `[Authentication]`.
const SECURE_LABEL: &CStr = N_(c"Use a _secure connection (TLS)");

/// The mnemonic label of the authentication-method combo. It is a separate
/// constant for the same reason as [`SECURE_LABEL`]: one combo, not a label
/// and an entry.
const AUTH_LABEL: &CStr = N_(c"A_uthentication:");

/// The combo entries: each id that [`insert_entries`] writes to
/// `ESourceAuthentication:method`, with the translatable text that the combo
/// shows for it.
///
/// Other code needs each of the three ids; this dialog did not select them:
///
/// - `"none"` is the default of `ESourceAuthentication:method`. The doc of
///   `crate::account` defines it as "ask for a password the ordinary way".
/// - [`OAUTH2_AUTH_METHOD`] is the exact string that the default
///   implementation of `EOAuth2Service::can_process` compares `method` with
///   (see the doc of that constant). It is the only spelling of "use OAuth 2.0"
///   that `e_source_get_oauth2_access_token_sync` accepts for this account.
///   [`jmap_backend_core::oauth2::method_is_oauth2`] also accepts the generic
///   `"OAuth2"` alias, but `can_process` does not.
/// - [`jmap_backend_core::api_token::API_TOKEN_METHOD`] is the value that the
///   third branch of `connect_with` looks for. Only the connect paths of this
///   project use this spelling: no other code registers a service with that
///   name.
///
/// `"OAuth 2.0"` is the manual alternative to the automatic discovery of
/// [`config_lookup`](crate::config_lookup). A user who skips "Look Up Account
/// Details", or for whom it fails, can still select OAuth 2.0 for a server. A
/// user who used it sees its result here. **"API Token"** is for a provider
/// that uses Bearer authentication but has no OAuth 2.0 consent flow to
/// discover. The JMAP API of Fastmail is an example. The user types the token
/// into the same password prompt that Basic uses (the module docs of
/// [`jmap_backend_core::api_token`] tell why).
const AUTH_CHOICES: [(&CStr, &CStr); 3] = [
    (c"none", N_(c"Password")),
    (OAUTH2_AUTH_METHOD, N_(c"OAuth 2.0")),
    (
        jmap_backend_core::api_token::API_TOKEN_METHOD,
        N_(c"API Token"),
    ),
];

/// The qdata keys on `page` under which [`insert_entries`] keeps the status
/// label and its collection. The docs of [`insert_widgets`] tell why
/// [`on_extension_changed`] needs them: the signal does not give it these
/// objects.
const STATUS_LABEL_KEY: &CStr = c"jmap-config-status-label";
const STATUS_COLLECTION_KEY: &CStr = c"jmap-config-status-collection";

/// Evolution calls this when it builds the *Receiving Email* page for this
/// provider. It puts into `parent` the entries that the user can use to
/// correct the server and login name of an account.
///
/// ## Why this binds to the collection, not to `get_settings()`
///
/// The service backends of Evolution (`e-mail-config-remote-accounts.c`,
/// upstream 3.52.3, read and not assumed) bind their entries to
/// `e_mail_config_service_backend_get_settings(backend)`: a `CamelSettings` of
/// the *scratch mail source* that the page builds. That is correct for a
/// provider whose server is on that source. For JMAP, it is not:
/// [`check_complete`] and [`commit_changes`] both read and write the
/// *collection* (their docs tell why). An entry bound to the settings of the
/// mail source would show and change a value that neither vfunc reads. Thus
/// the widgets must bind to the source that the rest of this class uses as the
/// account.
///
/// ## Why this function tells the page about a change
///
/// In the same upstream file, `mail_config_service_page_new_candidate`
/// connects a `notify` handler on `get_settings()` that calls
/// `e_mail_config_page_changed`. Thus *Next* in the assistant (and *Apply* in
/// the account editor) asks [`check_complete`] again after a keystroke. That
/// handler monitors `get_settings()`, which is the object that this class
/// deliberately does not bind to. Thus this function connects its own `notify`
/// handler on the `[Authentication]` and `[Security]` extensions of the
/// collection, and that handler calls `e_mail_config_page_changed`. The effect
/// is the same, but on the object that this dialog changes.
///
/// It uses `g_signal_connect_object`, not `g_signal_connect`: the connection
/// disconnects itself immediately when the extension or `page` is finalized.
/// Thus no stored handler id and no `dispose` override are necessary, and the
/// connection stays safe whatever the lifetimes of the two objects are.
///
/// ## Where the status label refresh keeps its state
///
/// The same `notify` handler ([`on_extension_changed`]) also updates the status
/// label after the first fill. It needs the label and the collection, but GLib
/// gives it neither: only the changed extension and the connected `page`. Thus,
/// when this function builds the label, it keeps both pointers as qdata of
/// `page` (`g_object_set_data`, which `gobject-sys` already has, thus no new
/// FFI). [`on_extension_changed`] reads them back with `g_object_get_data`.
/// `page` is the correct owner: it lives longer than each `notify` that this
/// class connects, for the reason above.
///
/// ## Not tested here, as the rest of this vfunc
///
/// GTK 3 does not construct a widget without a display connection, and this
/// machine has none (see the module docs of [`evo_sys`]). The `tests/gtk.rs`
/// and `tests/page.rs` of `evo-sys` already check each call below against the
/// linked library and its types. No test here can run this function and
/// examine the result. A real Evolution session must confirm that the page
/// shows three entries, an authentication combo and a check button with the
/// values from `setup_defaults`, and that a change to each toggles *Next*. The
/// Xvfb tier of M9 starts Evolution, but with a pre-built `.source` file and
/// not through this page. Thus only a human who uses the account assistant
/// tests it. Two operator rounds in real Evolution confirmed this behaviour.
///
/// ## Failure
///
/// None: the vfunc returns void. With a NULL collection (a failed
/// [`new_collection`], which logged the cause), the page has no entries, not
/// entries bound to nothing. The guard catches a panic, and the page stays in
/// the state that it had at that time.
unsafe extern "C" fn insert_widgets(backend: *mut EMailConfigServiceBackend, parent: *mut GtkBox) {
    guard("insert_widgets", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both are `(transfer none)`: the
        // backend's own references, which live longer than this call.
        let (collection, page) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_page(backend),
            )
        };
        if collection.is_null() {
            return;
        }

        // SAFETY: `collection` is non-NULL (checked above) and a valid source;
        // `page` is NULL or the backend's own live page; `parent` is the
        // `GtkBox` that Evolution gave this vfunc, which is the type that
        // `insert_entries` documents.
        unsafe { insert_entries(collection, page, parent) };
    });
}

/// The part of [`insert_widgets`] that changes real objects. The docs of that
/// vfunc give the design.
///
/// # Safety
///
/// `collection` must be a valid `ESource`: the backend's collection, which the
/// backend keeps alive for at least the life of the page. `page` must be
/// NULL or a valid `EMailConfigServicePage`. `parent` must be a valid
/// `GtkBox`: the container that Evolution gave `insert_widgets`.
unsafe fn insert_entries(
    collection: *mut ESource,
    page: *mut EMailConfigServicePage,
    parent: *mut GtkBox,
) {
    // SAFETY: no arguments; registers the extension types for the lookups
    // below. `crate::account::apply`/`read` make the same calls before each
    // of their lookups.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }
    // SAFETY: `collection` is valid by the contract of this function, and the
    // header constants name extensions whose types are registered above; the
    // source creates each extension on demand and owns it.
    let (authentication, security) = unsafe {
        (
            e_source_get_extension(collection, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()),
            e_source_get_extension(collection, E_SOURCE_EXTENSION_SECURITY.as_ptr()),
        )
    };

    // SAFETY: no arguments; `tests/gtk.rs` of `evo-sys` already resolves each
    // GTK call in this function against the linked library and checks its
    // types against the running GTK.
    let grid = unsafe { gtk_grid_new() };
    // SAFETY: `grid` is newly constructed above and is a `GtkGrid`.
    unsafe {
        gtk_grid_set_row_spacing(grid.cast(), 6);
        gtk_grid_set_column_spacing(grid.cast(), 6);
    }

    for (row, (label_text, property, kind)) in ENTRY_ROWS.into_iter().enumerate() {
        let label_text = cstring_lossy(&translate(label_text));
        // SAFETY: `label_text` lives longer than this call, which is all that
        // `gtk_label_new_with_mnemonic` needs: it copies the string into the
        // label that it returns.
        let label = unsafe { gtk_label_new_with_mnemonic(label_text.as_ptr()) };
        // SAFETY: `label` is newly constructed above and is a `GtkLabel`.
        unsafe { gtk_label_set_xalign(label.cast(), 1.0) };

        // SAFETY: no arguments.
        let entry = unsafe { gtk_entry_new() };
        // SAFETY: `label` and `entry` are both newly constructed above and
        // are, respectively, a `GtkLabel` and a `GtkWidget`.
        unsafe {
            gtk_label_set_mnemonic_widget(label.cast(), entry);
            gtk_widget_set_hexpand(entry, GTRUE);
            gtk_grid_attach(grid.cast(), label, 0, row as i32, 1, 1);
            gtk_grid_attach(grid.cast(), entry, 1, row as i32, 1, 1);
        }

        // With a NULL extension, GLib refuses the binding with a critical.
        // Thus the entry stays unbound, as the `notify` loop below leaves it
        // unmonitored.
        if authentication.is_null() {
            continue;
        }
        // SAFETY: `authentication` is the collection's own extension, checked
        // non-NULL above (created above, and owned by `collection`), and
        // `entry` is a live `GtkEntry` with a string `text` property; the
        // binding is `(transfer none)`, owned by the two objects that it
        // connects, and it lives longer than this call. For `RowKind::Port`
        // the target property is `guint16`, not a string, and
        // `port_to_text`/`text_to_port` convert between the two; their docs
        // give the `GBindingTransformFunc` contract that they satisfy.
        match kind {
            RowKind::Text => unsafe {
                e_binding_bind_property(
                    authentication,
                    property.as_ptr(),
                    entry.cast(),
                    c"text".as_ptr(),
                    G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
                );
            },
            RowKind::Port => unsafe {
                e_binding_bind_property_full(
                    authentication,
                    property.as_ptr(),
                    entry.cast(),
                    c"text".as_ptr(),
                    G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
                    Some(port_to_text),
                    Some(text_to_port),
                    ptr::null_mut(),
                    None,
                );
            },
        }
    }

    // The authentication-method combo: a label-and-widget row after the three
    // entries, but bound by `active-id`, not `text`. The doc of `AUTH_CHOICES`
    // tells what the three ids mean and why the list is complete.
    let auth_row = ENTRY_ROWS.len() as i32;
    let label_text = cstring_lossy(&translate(AUTH_LABEL));
    // SAFETY: `label_text` lives longer than the call, which is all that
    // `gtk_label_new_with_mnemonic` needs: it copies the string.
    let auth_label = unsafe { gtk_label_new_with_mnemonic(label_text.as_ptr()) };
    // SAFETY: `auth_label` is newly constructed above and is a `GtkLabel`.
    unsafe { gtk_label_set_xalign(auth_label.cast(), 1.0) };

    // SAFETY: no arguments.
    let auth_combo = unsafe { gtk_combo_box_text_new() };
    for (id, text) in AUTH_CHOICES {
        let text = cstring_lossy(&translate(text));
        // SAFETY: `auth_combo` is newly constructed above and is a
        // `GtkComboBoxText`; `id` is a `'static` C string and `text` lives
        // longer than the call; `_append` copies both.
        unsafe { gtk_combo_box_text_append(auth_combo.cast(), id.as_ptr(), text.as_ptr()) };
    }
    // SAFETY: `auth_label` and `auth_combo` are both newly constructed above.
    unsafe {
        gtk_label_set_mnemonic_widget(auth_label.cast(), auth_combo.cast());
        gtk_widget_set_hexpand(auth_combo.cast(), GTRUE);
        gtk_grid_attach(grid.cast(), auth_label.cast(), 0, auth_row, 1, 1);
        gtk_grid_attach(grid.cast(), auth_combo.cast(), 1, auth_row, 1, 1);
    }
    if !authentication.is_null() {
        // SAFETY: `authentication` is the collection's own extension, checked
        // non-NULL above (created above, and owned by `collection`);
        // `auth_combo` is a live `GtkComboBox` (a `GtkComboBoxText` is one) with
        // a string `active-id` property, and `AUTH_CHOICES` gave it exactly the
        // three ids that `method` can hold, thus no transform is necessary.
        unsafe {
            e_binding_bind_property(
                authentication,
                c"method".as_ptr(),
                auth_combo.cast(),
                c"active-id".as_ptr(),
                G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
            );
        }
    }

    // The security toggle: one check button across both columns, on the row
    // after the authentication combo.
    // SAFETY: `label_text` lives longer than the call, which is all that
    // `gtk_check_button_new_with_mnemonic` needs: it copies the string.
    let label_text = cstring_lossy(&translate(SECURE_LABEL));
    let check = unsafe { gtk_check_button_new_with_mnemonic(label_text.as_ptr()) };
    // SAFETY: `check` is newly constructed above and is a `GtkWidget`; `grid`
    // is the live grid that holds all other rows.
    unsafe { gtk_grid_attach(grid.cast(), check, 0, auth_row + 1, 2, 1) };
    if !security.is_null() {
        // SAFETY: `security` is the collection's own extension, checked
        // non-NULL above (created above, and owned by `collection`), and `check`
        // is a live `GtkToggleButton` with a boolean `active` property, the same
        // type as `ESourceSecurity:secure`, thus no transform is necessary.
        unsafe {
            e_binding_bind_property(
                security,
                c"secure".as_ptr(),
                check.cast(),
                c"active".as_ptr(),
                G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
            );
        }
    }

    // The status label: one label across both columns, on the row after the
    // check button. It uses `gtk_label_new`, not `_with_mnemonic`: the text
    // comes from `Incomplete`, and this module must not add a keyboard
    // shortcut to it.
    // SAFETY: no arguments.
    let status_label = unsafe { gtk_label_new(ptr::null()) };
    // SAFETY: `status_label` is newly constructed above and is a `GtkWidget`;
    // `grid` is the live grid that holds all other rows.
    unsafe {
        gtk_grid_attach(grid.cast(), status_label, 0, auth_row + 2, 2, 1);
    }
    // SAFETY: `status_label` is a live `GtkLabel`, newly constructed, and
    // `collection` is a valid source by the contract of this function.
    unsafe { set_status_text(status_label, collection) };

    // SAFETY: `grid` is the live `GtkWidget` constructed above, and `parent`
    // is a valid `GtkBox` by the contract of this function.
    unsafe {
        gtk_box_pack_start(parent, grid, GFALSE, GFALSE, 0);
        gtk_widget_show_all(grid);
    }

    if !page.is_null() {
        // `on_extension_changed` must refresh the status label, but it does
        // not get the label or the collection. The docs of `insert_widgets`
        // tell why they are kept as qdata of `page` and not passed through
        // the signal.
        // SAFETY: `page` is a valid, live GObject by the contract of this
        // function, checked non-NULL above; `status_label` and `collection`
        // both live longer than it: `grid` owns the label, `parent` now owns
        // `grid`, and `collection` is the backend's own reference, which lives
        // longer than the page that it built. Neither call transfers ownership
        // (no `GDestroyNotify`): this is a cache of pointers that other owners
        // keep alive, not a new reference that must be released.
        unsafe {
            g_object_set_data(page.cast(), STATUS_LABEL_KEY.as_ptr(), status_label.cast());
            g_object_set_data(
                page.cast(),
                STATUS_COLLECTION_KEY.as_ptr(),
                collection.cast(),
            );
        }

        // Each extension gets its own connection. `secure` is on `[Security]`,
        // not `[Authentication]`, and it changes the answer of
        // `check_complete` as much as `host` does: `origin`
        // (`jmap_backend_core::source`) refuses plaintext to a non-loopback
        // host. Thus, to clear the toggle can make a complete account
        // incomplete. One handler is for both; see its docs.
        for extension in [authentication, security] {
            if extension.is_null() {
                continue;
            }
            // SAFETY: `extension` is the collection's own live extension,
            // checked non-NULL above, and `page` is a valid page by the
            // contract of this function, also checked non-NULL above. The
            // signature of the callback matches the arguments of a `notify`
            // handler (the emitting `GObject`, the `GParamSpec` of the
            // changed property, and the data pointer of the connection).
            // `g_signal_connect_object`, not `g_signal_connect_data`: the
            // docs of `insert_widgets` tell why no code here keeps the
            // handler id.
            unsafe {
                g_signal_connect_object(
                    extension,
                    c"notify".as_ptr(),
                    gcallback(
                        on_extension_changed
                            as unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
                    ),
                    page.cast(),
                    G_CONNECT_DEFAULT,
                );
            }
        }
    }
}

/// The `notify` handler that [`insert_entries`] connects on the
/// `[Authentication]` and `[Security]` extensions of the collection. It tells
/// the page that an entry or the security toggle changed, thus
/// `check_complete` runs again. It also refreshes the status label.
///
/// # Safety
///
/// GLib signal emission calls this with the usual `notify` handler arguments:
///
/// - the object whose property changed (not used: this handler answers for
///   the full connection, as the upstream
///   `mail_config_service_page_settings_notify_cb` does),
/// - the `GParamSpec` of the changed property (not used, for the same reason),
/// - `page`, because the connection uses `g_signal_connect_object` against
///   `page`. `page` stays alive during this call (a guarantee of that
///   function, not of this one). It is the object on which [`insert_entries`]
///   kept the status label and collection pointers with `g_object_set_data`.
unsafe extern "C" fn on_extension_changed(
    _extension: *mut GObject,
    _pspec: *mut GParamSpec,
    page: gpointer,
) {
    guard("on_extension_changed", (), || {
        // SAFETY: `page` is the `EMailConfigServicePage` of the connection, by
        // the contract of this function, and each `EMailConfigPage`
        // implementor accepts `e_mail_config_page_changed`; the `build.rs` of
        // `evo-sys` gives the upstream reading that makes the cast sound.
        unsafe { e_mail_config_page_changed(page.cast::<EMailConfigPage>()) };

        // SAFETY: `page` is the object on which `insert_entries` set both
        // keys, by the contract of this function. `g_object_get_data` returns
        // NULL for a key that is not set, and this code treats that as "no
        // label to update", not as a fault. Only the connection that
        // `insert_entries` makes after it sets the keys can call this
        // handler, but this code does not assume it.
        let label = unsafe { g_object_get_data(page.cast(), STATUS_LABEL_KEY.as_ptr()) };
        if !label.is_null() {
            // SAFETY: the same code sets this key together with `label`, thus
            // both are present or neither is.
            let collection =
                unsafe { g_object_get_data(page.cast(), STATUS_COLLECTION_KEY.as_ptr()) };
            // SAFETY: `label` is non-NULL (checked above), and was a live
            // `GtkLabel` when kept; `collection` is NULL or a valid
            // `ESource`, as the contract of [`set_status_text`] requires.
            unsafe { set_status_text(label.cast(), collection.cast()) };
        }
    });
}

/// Refreshes the text of `label` from the account that `collection` contains.
/// [`insert_entries`] calls it once for the first fill, and
/// [`on_extension_changed`] calls it after each keystroke that can change the
/// answer. Empty and hidden when a commit accepts the account; otherwise the
/// text of [`Incomplete`](crate::complete::Incomplete).
///
/// # Safety
///
/// `label` must be a valid `GtkLabel`. `collection` must be NULL or a valid
/// `ESource`.
unsafe fn set_status_text(label: *mut GtkWidget, collection: *mut ESource) {
    let text = if collection.is_null() {
        // The same silence as the doc of `is_complete` gives for a NULL
        // collection: the failed `new_collection` already logged a critical,
        // and this is not a second location to explain it.
        String::new()
    } else {
        // SAFETY: non-NULL and a valid source by the contract of this function.
        status_message(&unsafe { read(collection) })
    };
    let visible = !text.is_empty();
    let text = cstring_lossy(&text);
    // SAFETY: `label` is a live `GtkLabel` by the contract of this function;
    // `gtk_label_set_text` copies the string, thus `text` need not live
    // longer than this call.
    unsafe {
        gtk_label_set_text(label.cast(), text.as_ptr());
        gtk_widget_set_visible(label, if visible { GTRUE } else { GFALSE });
    }
}

/// The `GBindingTransformFunc` of the `port` row from the
/// `ESourceAuthentication` side: it converts a `guint16` to the text of a
/// `GtkEntry`. 0 (the keyfile spelling of "not set", as the doc of
/// `crate::account` tells) shows as an empty entry, not as a literal `0`.
///
/// A panic becomes `FALSE`, which is the "no value" answer of a
/// `GBindingTransformFunc`: the binding does not change the text of the entry
/// and does not write a partly computed value. A panic must not go from here
/// into the binding code of GLib.
///
/// # Safety
///
/// The arguments are those of `GBindingTransformFunc`: `from_value` holds the
/// value of the source property (a `guint`, because only
/// `ESourceAuthentication:port` uses this function), and `to_value` is
/// initialised to the type of the target property, a string.
unsafe extern "C" fn port_to_text(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("port_to_text", GFALSE, || {
        // SAFETY: a `GValue` that holds the guint of the source property of
        // this binding, by the contract above.
        let port = unsafe { g_value_get_uint(from_value) };
        let text = if port == 0 {
            String::new()
        } else {
            port.to_string()
        };
        let text = cstring_lossy(&text);
        // SAFETY: a `GValue` initialised to the string type of the target
        // property; `g_value_set_string` copies the string, thus `text` need
        // not live longer than this call.
        unsafe { g_value_set_string(to_value, text.as_ptr()) };
        GTRUE
    })
}

/// The `GBindingTransformFunc` of the `port` row from the `GtkEntry` side: it
/// converts the text back to a `guint16`, the inverse of [`port_to_text`].
///
/// An empty entry is `0` ("not set", as [`port_to_text`] shows it). Text that
/// is not a bare number in the `guint16` range gets `FALSE`, the "no value"
/// answer of a `GBindingTransformFunc`. The binding then does not change
/// `ESourceAuthentication:port` and does not write a port that the user did
/// not type. A panic also becomes `FALSE`, for the reason that
/// [`port_to_text`] gives.
///
/// # Safety
///
/// The arguments are those of `GBindingTransformFunc`: `from_value` holds the
/// value of the target property (a string, because only the `text` target of
/// the binding that [`port_to_text`] is the other half of uses this function),
/// and `to_value` is initialised to the type of the source property, a
/// `guint`.
unsafe extern "C" fn text_to_port(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("text_to_port", GFALSE, || {
        // SAFETY: a `GValue` that holds the NUL-terminated string of the
        // target property of this binding, by the contract above; the entry
        // owns the string and it is valid only during this call, thus it is
        // copied, not held.
        let text = unsafe { read_string(g_value_get_string(from_value)) }.unwrap_or_default();
        let text = text.trim();
        let port: u16 = if text.is_empty() {
            0
        } else {
            match text.parse() {
                Ok(port) => port,
                Err(_) => return GFALSE,
            }
        };
        // SAFETY: a `GValue` initialised to the guint type of the source
        // property.
        unsafe { g_value_set_uint(to_value, u32::from(port)) };
        GTRUE
    })
}

/// Evolution calls this when the user opens the server settings page. It
/// writes the account data that the address alone gives.
///
/// Evolution decides how many times it calls this, and it is not possible to
/// examine that here: the page-preparation order of the assistant is in a
/// source that this machine does not have, and in a dialog that it cannot run.
/// Thus [`setup`] is correct for both cases. On the first call, it fills the
/// page. When the user returns to the identity page and then to this page, it
/// keeps the values that the user typed, unless the address changed.
///
/// ## The source of the address
///
/// The `EMailConfigServicePage` that this extension extends. This is the only
/// data that a vfunc here gets from Evolution and not from an `ESource`: the
/// identity page of the assistant writes the address onto the page, and the
/// `setup_defaults` of each provider reads it with
/// `e_mail_config_service_page_get_email_address`. A backend with no page
/// (Evolution does not make this state, because `constructed` sets the page)
/// gives the empty address. For that, [`from_identity`] gives the account that
/// [`new_collection`] already wrote.
///
/// ## Failure
///
/// None: the vfunc returns void and cannot report an error. The guard catches
/// a panic and logs a critical. The page then shows the existing data of the
/// collection, which is the account of [`new_collection`], and
/// `check_complete` refuses it until the user fills in the address.
unsafe extern "C" fn setup_defaults(backend: *mut EMailConfigServiceBackend) {
    guard("setup_defaults", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both are `(transfer none)`: the
        // backend's own references, which live longer than this call.
        let (page, collection) = unsafe {
            (
                e_mail_config_service_backend_get_page(backend),
                e_mail_config_service_backend_get_collection(backend),
            )
        };

        let address = if page.is_null() {
            None
        } else {
            // SAFETY: a live page, and the address is `(transfer none)`: a
            // NUL-terminated string that the page owns, copied here and not
            // held.
            unsafe { read_string(e_mail_config_service_page_get_email_address(page)) }
        };

        // SAFETY: NULL or the backend's live collection source, which is what
        // `setup` documents.
        unsafe { setup(collection, address.as_deref().unwrap_or_default()) };
    });
}

/// Writes onto `collection` the account data that the address implies. This is
/// the deciding part of the `setup_defaults` vfunc, and the part that tests can
/// call.
///
/// Returns true if it wrote; the tests use this answer. The vfunc cannot use
/// the answer and discards it.
///
/// ## Why this does not only apply [`from_identity`]
///
/// Because it can run more than once. `from_identity` gives a full account: the
/// address, the server that it implies, the login name that it offers, *and*
/// the three parts and the TLS switch. Only the first three come from the
/// address. `new_collection` wrote the other two before the user saw the page.
/// On a second run, they can be values that the user selected, for example a
/// cleared *Calendars* box on a page that the user left to correct the
/// address. A second application of the full default account would set the
/// box again.
///
/// Thus this function takes from the offer only the fields that the address
/// gives, and does not change the rest of the account. A test asserts the
/// result: on a collection from `new_collection`, the result is exactly
/// `from_identity(address)`. `tests/backend.rs` reads it back through the
/// reader of the registry.
///
/// ## Why an unchanged address writes nothing
///
/// The same reason, one step further. If the collection already has this
/// address, the defaults for it are already offered. The server data in the
/// account is then that offer, or a correction that the user made. A JMAP
/// server is not necessarily at the domain of the address: RFC 8620 §2.2 only
/// says to *ask* there. A new derivation would overwrite the correction of the
/// user, only because the user looked at the previous page again.
///
/// A *changed* address is the opposite case, and gets a new derivation: the
/// server that the user typed was for the old address.
///
/// # Safety
///
/// `collection` must be NULL or a valid `ESource`: the backend's collection.
/// This function reads and writes it, and keeps nothing after the call.
pub unsafe fn setup(collection: *mut ESource, address: &str) -> bool {
    if collection.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by the contract of this function.
    let mut account = unsafe { read(collection) };
    if account.identity == address {
        return false;
    }

    let offered = from_identity(address);
    account.identity = offered.identity;
    account.connection.host = offered.connection.host;
    account.connection.user = offered.connection.user;

    // SAFETY: as above; `apply` borrows the account only for the call.
    unsafe { apply(collection, &account) };
    true
}

/// Evolution asks this before the assistant goes to the next page, and again
/// after each keystroke: is this account complete?
///
/// The answer sets the sensitivity of *Next* (or *Apply*, in the account
/// editor). Evolution asks it frequently and while the user types. Thus it
/// must be fast and must never use the network.
/// [`complete`](crate::complete) has the same rule for its decision.
///
/// Before the check, this function narrows the collection to the address on
/// the page (see the comment in the body).
///
/// ## The source that it examines
///
/// The *collection*, not the mail source. For a groupware provider, the account
/// is the collection from [`new_collection`]. The three mail sources get their
/// data from it at commit time ([`crate::mail`]). Thus this function reads the
/// account in the collection, which is the account that a commit writes.
///
/// evolution-ews examines its `CamelEwsSettings`, because its entries are bound
/// to it. Both are correct, and this crate uses the collection everywhere:
/// [`apply`](crate::account::apply) writes it, [`read`](crate::account::read)
/// reads it, and the collection backend reads it back in a different process
/// as the only description of the account. `insert_widgets` binds its entries
/// to the same source. With widgets bound to a different object, this vfunc
/// would examine a source that the user does not change.
///
/// ## Failure
///
/// FALSE. The assistant stays on the page, which is the safe direction. TRUE
/// on a panic would let the user commit a partly read account.
unsafe extern "C" fn check_complete(backend: *mut EMailConfigServiceBackend) -> gboolean {
    guard("check_complete", GFALSE, || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both are `(transfer none)`: the
        // backend's own references, which live longer than this call.
        let (collection, page) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_page(backend),
            )
        };

        // Narrow the collection to the address that the user typed, if the page
        // has it. `setup_defaults` is the Evolution hook for this, but it does
        // not run when the user edits an autoconfig result and changes the
        // server type to JMAP. Also, when `insert_widgets` constructs the
        // widgets, the page has no address yet. Evolution calls
        // `check_complete` after each change to the page, thus after the
        // address is available. On that path, this is the reliable location
        // for the narrow. `setup` writes only when the address is different
        // from the address in the collection. Thus it does nothing after the
        // first narrow. It does not overwrite a host that the user corrected,
        // unless the user also changed the related address.
        if !page.is_null() {
            // SAFETY: `page` is the backend's own live page, checked non-NULL
            // above; the address is `(transfer none)`: a NUL-terminated string
            // that the page owns, which `read_string` copies.
            let address =
                unsafe { read_string(e_mail_config_service_page_get_email_address(page)) };
            if let Some(address) = address.filter(|a| !a.trim().is_empty()) {
                // SAFETY: `collection` is NULL or the backend's live collection
                // source; `setup` accepts NULL and keeps nothing after the call.
                unsafe { setup(collection, &address) };
            }
        }

        // SAFETY: NULL or the backend's live collection source, which is
        // exactly what `is_complete` documents.
        if unsafe { is_complete(collection) } {
            GTRUE
        } else {
            GFALSE
        }
    })
}

/// Tells if a commit can accept the account in `collection`. This is the
/// deciding part of the `check_complete` vfunc, and the part that tests can
/// call.
///
/// It calls [`read`] and then [`check`], nothing more. They are separate
/// because, while the user types, the dialog does not usually contain a full
/// account. The reader accepts all input and returns the contents of the
/// source. The check decides if those contents are acceptable.
///
/// ## A NULL collection gives FALSE, with no message
///
/// There is one cause of a NULL collection: `new_collection` failed. It logs a
/// critical when it fails. That is the location of the failure and the only
/// location that can explain it. A message here would repeat after each
/// keystroke and hide the first message. Thus this function is silent, and it
/// refuses, so that the user cannot commit an account that cannot be read.
///
/// The refusal of an account that is *only* incomplete is silent for a
/// different reason. [`Incomplete`](crate::complete::Incomplete) is text for
/// the user, in the entry that the user typed into. This vfunc returns a
/// boolean and has no entry for the text. The status label of
/// `insert_widgets` shows the same reason. That label computes it separately
/// ([`crate::complete::status_message`]); this boolean does not carry it.
///
/// # Safety
///
/// `collection` must be NULL or a valid `ESource`. This function only reads it
/// and keeps nothing after the call.
pub unsafe fn is_complete(collection: *mut ESource) -> bool {
    if collection.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by the contract of this function.
    check(&unsafe { read(collection) }).is_ok()
}

/// Evolution calls this after the last push of *Next*. It writes the account
/// on the screen into the scratch sources. The assistant then gives these to
/// `e_source_registry_create_sources` as one batch.
///
/// The vfunc does not create or save anything. Each `ESource` that it changes
/// is a scratch source already in the creation queue. Its only purpose is to
/// write the fields that no other code fills in.
///
/// ## The fields for JMAP, and why there are few
///
/// For each provider whose server the user typed on *this* page (IMAP, SMTP,
/// POP3), there is nothing to do, and the Evolution default does nothing. The
/// entries were bound through `CamelSettings` to the `[Authentication]` and
/// `[Security]` of the mail source, and changed it during typing. JMAP asks
/// for a server only once, on the account, because one JMAP session carries
/// the mail, the contacts and the calendars. Thus the mail source is a second
/// file that has not received the server, and to write it is the only task of
/// this vfunc.
///
/// These fields are *not* written here, because other code writes them before
/// this runs:
///
/// - **The service name.** `e_mail_config_assistant` makes one scratch source
///   per provider and writes the protocol into it before a backend sees it.
///   That is how Evolution selected the candidate of [`new_collection`].
/// - **The parent and the two uid links.** The `commit_changes` of
///   `EMailConfigSummaryPage` sets the `Parent` of all three sources to the uid
///   of the collection, the `identity-uid` of the account and the
///   `transport-uid` of the identity. It runs after this one, from the page
///   loop of the assistant, and does the same as [`crate::mail::apply`]. Thus
///   that function writes the *account*, and this vfunc does not call it.
/// - **The address of the identity**, which `EMailConfigIdentityPage` writes.
///
/// ## One backend per page, and the empty account to refuse
///
/// Evolution makes one instance of this class for the *Receiving Email* page
/// and one for *Sending Email*, and `constructed` calls [`new_collection`] on
/// each. Thus the sending instance has its own scratch collection, which no
/// widget fills in and the assistant never queues. A simple copy from that
/// collection would write an empty host onto the transport source. An empty
/// host is not the same as a host that is not written: it reads back as an
/// account with a server.
///
/// Thus the copy occurs only for a collection that the setup can commit:
/// [`is_complete`], the same question that [`check_complete`] answers. It is
/// true for the collection of the receiving instance (that is why *Next* was
/// sensitive), and false for the collection of the sending instance. Silent in
/// both cases: a refusal here is that second instance, which is not a fault,
/// or an account that `check_complete` already kept on the page.
///
/// ## The resulting gap, and where it is closed
///
/// Thus, on the assistant path, the transport source gets the service name
/// `jmap` and no server, but JMAP submission needs a server. No other part of
/// *this* dialog can write it: the sending page is hidden for a
/// store-and-transport provider, and its candidate backend cannot see the
/// account. The collection backend in `evolution-source-registry` can write
/// it: it gets the account and the `child_added` callback for each source with
/// the account as parent. **That increment is now available**:
/// `jmap_backend_collection::mail_child::follow_server` binds the
/// `[Authentication]`/`[Security]` of the transport (and of the mail account)
/// to those of the collection, for this reason. It is the only exception to
/// the usual `child_added` rule "both sides already have it". The module docs
/// tell why the transport needs an exception, and how
/// `[Security]`/`[Authentication] Method` are translated between the two sides
/// (to a `CamelNetworkSecurityMethod` nick and a Bearer/OAuth2 credential
/// selector, respectively, not a plain copy).
unsafe extern "C" fn commit_changes(backend: *mut EMailConfigServiceBackend) {
    guard("commit_changes", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both are `(transfer none)`: the
        // backend's own references, which live longer than this call.
        let (collection, source) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_source(backend),
            )
        };
        // SAFETY: each is NULL or one of the backend's live sources, which is
        // exactly what `commit` documents.
        unsafe { commit(collection, source) };
    });
}

/// Copies the server of the account onto the mail source of a backend. This is
/// the writing part of the `commit_changes` vfunc, and the part that tests can
/// call.
///
/// Returns true if it wrote; the tests use this answer. The vfunc cannot use
/// the answer and discards it.
///
/// It does nothing if a source is missing or the account is not complete. The
/// `commit_changes` vfunc above tells why the second case is normal and not a
/// failure, and why neither case is logged.
///
/// # Safety
///
/// `collection` and `source` must each be NULL or a valid `ESource`. This
/// function only reads the first and writes the second, and keeps nothing
/// after the call.
pub unsafe fn commit(collection: *mut ESource, source: *mut ESource) -> bool {
    if collection.is_null() || source.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by the contract of this function.
    let account = unsafe { read(collection) };
    if check(&account).is_err() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by the contract of this function;
    // the connection lives longer than the call, which is all that
    // `apply_server` borrows.
    unsafe { apply_server(source, &account.connection) };
    true
}

/// The message of a failed EDS call. Consumes the `GError`.
///
/// # Safety
///
/// `error` must be NULL or a `GError` that this call can consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        return "EDS set no error".to_owned();
    }

    // SAFETY: a live GError; its message is a NUL-terminated string that it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: the out-parameter transferred ownership to this caller.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "EDS gave no message".to_owned())
}

/// Tests for [`port_to_text`] and [`text_to_port`], with manually built
/// `GValue`s and no real `GBinding`. Thus they can run here: they are the only
/// part of `insert_widgets` with no widget, and they need no display
/// connection.
#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::mem::zeroed;

    use gobject_sys::{G_TYPE_STRING, G_TYPE_UINT, g_value_init, g_value_unset};

    use super::*;

    /// A zeroed, initialised `GValue` of the given type. This is the C pattern
    /// `GValue value = G_VALUE_INIT; g_value_init(&value, type);`, as in
    /// `tests/oauth2_service.rs`.
    ///
    /// # Safety
    ///
    /// `gtype` must be a `GType` that `g_value_init` accepts; the caller must
    /// `g_value_unset` the result after use.
    unsafe fn value_of(gtype: glib_sys::GType) -> GValue {
        // SAFETY: a new `GValue` has no prior contents to unset, thus to
        // zero-initialise it before `g_value_init` is exactly the C pattern.
        unsafe {
            let mut value: GValue = zeroed();
            g_value_init(&mut value, gtype);
            value
        }
    }

    /// The exact contents of a string `GValue`. [`read_string`], which this
    /// module uses elsewhere, does not keep an empty string separate from NULL.
    /// `port_to_text` writes a real empty string for "no port", and these
    /// tests must tell it apart from NULL.
    ///
    /// # Safety
    ///
    /// `value` must hold a string, as `g_value_get_string` requires.
    unsafe fn string_of(value: &GValue) -> Option<String> {
        // SAFETY: by the contract of this function.
        let ptr = unsafe { g_value_get_string(value) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string owned by `value`, copied here.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    #[test]
    fn port_to_text_shows_zero_as_an_empty_entry() {
        // SAFETY: `from` is a `guint` GValue, as `port_to_text` requires; `to`
        // is a string GValue, which it fills in; the test unsets both before
        // it ends.
        unsafe {
            let mut from = value_of(G_TYPE_UINT);
            g_value_set_uint(&mut from, 0);
            let mut to = value_of(G_TYPE_STRING);

            let ok = port_to_text(ptr::null_mut(), &from, &mut to, ptr::null_mut());

            assert_eq!(ok, GTRUE, "port_to_text refused a plain zero");
            assert_eq!(string_of(&to), Some(String::new()));

            g_value_unset(&mut from);
            g_value_unset(&mut to);
        }
    }

    #[test]
    fn port_to_text_shows_a_set_port_as_its_number() {
        // SAFETY: as above.
        unsafe {
            let mut from = value_of(G_TYPE_UINT);
            g_value_set_uint(&mut from, 443);
            let mut to = value_of(G_TYPE_STRING);

            let ok = port_to_text(ptr::null_mut(), &from, &mut to, ptr::null_mut());

            assert_eq!(ok, GTRUE);
            assert_eq!(string_of(&to), Some("443".to_owned()));

            g_value_unset(&mut from);
            g_value_unset(&mut to);
        }
    }

    /// The round trip of [`port_to_text`]/[`text_to_port`]: the text that one
    /// shows, the other reads back to the same port.
    #[test]
    fn text_to_port_reverses_port_to_text() {
        for port in [0u16, 1, 443, 8080, u16::MAX] {
            // SAFETY: as above.
            unsafe {
                let mut from = value_of(G_TYPE_UINT);
                g_value_set_uint(&mut from, u32::from(port));
                let mut text = value_of(G_TYPE_STRING);
                assert_eq!(
                    port_to_text(ptr::null_mut(), &from, &mut text, ptr::null_mut()),
                    GTRUE
                );
                g_value_unset(&mut from);

                let mut back = value_of(G_TYPE_UINT);
                let ok = text_to_port(ptr::null_mut(), &text, &mut back, ptr::null_mut());
                g_value_unset(&mut text);

                assert_eq!(ok, GTRUE, "text_to_port refused {port}'s own text");
                assert_eq!(
                    g_value_get_uint(&back),
                    u32::from(port),
                    "port {port} did not round-trip"
                );
                g_value_unset(&mut back);
            }
        }
    }

    #[test]
    fn text_to_port_treats_blank_or_whitespace_only_text_as_unset() {
        for text in ["", "   ", "\t"] {
            // SAFETY: as above; `text` is NUL-terminated for the C string
            // that `g_value_set_string` copies.
            unsafe {
                let text_c = CString::new(text).unwrap();
                let mut from = value_of(G_TYPE_STRING);
                g_value_set_string(&mut from, text_c.as_ptr());
                let mut to = value_of(G_TYPE_UINT);

                let ok = text_to_port(ptr::null_mut(), &from, &mut to, ptr::null_mut());

                assert_eq!(ok, GTRUE, "{text:?} should be accepted as \"not set\"");
                assert_eq!(g_value_get_uint(&to), 0);

                g_value_unset(&mut from);
                g_value_unset(&mut to);
            }
        }
    }

    #[test]
    fn text_to_port_refuses_what_is_not_a_bare_port_number() {
        for text in ["not a number", "443x", "-1", "65536", "1.5"] {
            // SAFETY: as above.
            unsafe {
                let text_c = CString::new(text).unwrap();
                let mut from = value_of(G_TYPE_STRING);
                g_value_set_string(&mut from, text_c.as_ptr());
                let mut to = value_of(G_TYPE_UINT);
                // A sentinel that the binding must not change on refusal. This
                // test only checks the return value, but a real bidirectional
                // `GBinding` needs this behaviour:
                // `FALSE` from `text_to_port` prevents a bad keystroke from
                // reaching `g_object_set_property`.
                g_value_set_uint(&mut to, 12345);

                let ok = text_to_port(ptr::null_mut(), &from, &mut to, ptr::null_mut());

                assert_eq!(ok, GFALSE, "{text:?} should not be a valid port");

                g_value_unset(&mut from);
                g_value_unset(&mut to);
            }
        }
    }
}
