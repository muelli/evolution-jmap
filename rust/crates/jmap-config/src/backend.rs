// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EMailConfigServiceBackend` subclass — the object Evolution's account
//! editor talks to.
//!
//! Evolution's *Receiving Email* page is an `EExtensible`, and every mail
//! provider that wants a say in account setup registers an `EExtension` of
//! this class against it. The page instantiates one per registered backend,
//! matches each one's [`backend_name`][name] against the Camel providers it
//! knows, and puts the ones that match in the provider combo. From then on
//! everything the setup does — what an account starts as, which widgets the
//! server page shows, whether *Next* is sensitive, what a commit writes — is a
//! vfunc on this class.
//!
//! [name]: crate::mail::MAIL_BACKEND_NAME
//!
//! The rest of this crate exists so that this file can stay thin. Each vfunc
//! body is a decision that was made, and tested, over a plain `ESource` in
//! [`account`](crate::account), [`mail`](crate::mail),
//! [`defaults`](crate::defaults) or [`complete`](crate::complete); what is
//! here is the GObject the decisions are reached through.
//!
//! ## What is installed, and what is deliberately left inherited
//!
//! - **`backend_name`** — not a vfunc but the field the page *finds* this
//!   backend by. Left NULL it is not an error: it is a JMAP entry that never
//!   appears in the account type list.
//! - **`new_collection`** — see `new_collection` below. Evolution's own answers
//!   NULL, which is right for POP3 and wrong for anything that fans out.
//! - **`setup_defaults`** — see `setup_defaults` below. Evolution's own is an
//!   empty function (read off the installed library, not assumed), which is a
//!   server settings page that opens blank over an address the assistant
//!   already knows.
//! - **`check_complete`** — see `check_complete` below. Evolution's own is
//!   `return TRUE` (read off the installed library, not assumed), which is an
//!   assistant whose *Next* is sensitive over an account with no address and no
//!   server.
//! - **`commit_changes`** — see `commit_changes` below. Evolution's own does
//!   nothing, which is right for every provider whose server was typed on this
//!   page and wrong for one whose server is on the account: it leaves the mail
//!   source naming a protocol and no host.
//! - **`insert_widgets`** — see `insert_widgets` below. Evolution's own is an
//!   empty function, which is a server settings page with nothing on it to
//!   correct a wrong offer with.
//! - **`get_selectable`** is left alone on purpose. Its default answers "yes,
//!   unless this provider is both a store and a transport, in which case only
//!   on the receiving page" — and the JMAP provider *is* both
//!   ([`jmap_mail`'s `PROTOCOL`][protocol] registers a store and a transport
//!   type), so the inherited answer is already the correct one. Overriding it
//!   with an unconditional `TRUE` would offer JMAP a second time in the
//!   *Sending Email* combo, as an account type the user can pick and then not
//!   configure.
//!
//! [protocol]: ../../jmap_mail/provider/constant.PROTOCOL.html
//!
//! ## What is not here yet
//!
//! `insert_widgets` now builds every field
//! [`Connection`](jmap_collection_sync::child_source::Connection) carries —
//! the server, the port, the login name, the authentication method and
//! whether the connection is encrypted — bound to the collection the same way
//! `check_complete` and `commit_changes` already read and write it, plus a
//! status label showing [`Incomplete`](crate::complete::Incomplete)'s refusal
//! reason (empty, and hidden, once the account is one a commit would accept).
//! What is still missing is only:
//!
//! - **Verification in a real Evolution.** GTK 3 will not construct a widget
//!   without a display connection, so nothing on this machine has run
//!   `insert_widgets` and looked at the result — see its own docs for exactly
//!   what a human still has to confirm, and `docs/NIGHT-LOG.md` for the
//!   session that wrote it saying so.
//! - **The consent browser round trip.** The authentication combo lets a
//!   user *say* an account is OAuth 2.0; it does not by itself prove EDS's
//!   `ECredentialsPrompterImplOAuth2` reaches a real provider and back for
//!   this project's registered client — that needs a human and a real
//!   deployment, the same gap
//!   [`config_lookup`](crate::config_lookup)'s own docs name for its half of
//!   OAuth 2.0 setup.
//!
//! ## The state this leaves the dialog in, said plainly
//!
//! An account whose address the assistant already knows arrives on the server
//! settings page filled in — the address, the server its domain implies and
//! the login name it offers — and now carries three entries, an
//! authentication-method combo and a check button to correct any of those
//! from, bound live to the account `check_complete` and `commit_changes` both
//! read, and a label underneath that says why *Next* refuses to light up when
//! it does.
//!
//! [`evo-sys`]: ../../evo_sys/index.html

use std::ffi::CStr;
#[cfg(feature = "testing")]
use std::mem::MaybeUninit;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ESource,
    e_binding_bind_property, e_binding_bind_property_full, e_source_authentication_get_type,
    e_source_get_extension, e_source_new, e_source_security_get_type,
};
use evo_sys::{
    EMailConfigPage, EMailConfigServiceBackend, EMailConfigServiceBackendClass,
    EMailConfigServicePage, GtkBox, GtkWidget, e_mail_config_page_changed,
    e_mail_config_service_backend_get_collection, e_mail_config_service_backend_get_page,
    e_mail_config_service_backend_get_source, e_mail_config_service_backend_get_type,
    e_mail_config_service_page_get_email_address, gtk_box_pack_start,
    gtk_check_button_new_with_mnemonic, gtk_combo_box_text_append, gtk_combo_box_text_new,
    gtk_entry_new, gtk_grid_attach, gtk_grid_new, gtk_grid_set_column_spacing,
    gtk_grid_set_row_spacing, gtk_label_new, gtk_label_new_with_mnemonic,
    gtk_label_set_mnemonic_widget, gtk_label_set_text, gtk_label_set_xalign,
    gtk_widget_set_hexpand, gtk_widget_set_visible, gtk_widget_show_all,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_free, gboolean, gpointer};
use gobject_sys::{
    G_BINDING_BIDIRECTIONAL, G_BINDING_SYNC_CREATE, G_CONNECT_DEFAULT, GBinding, GObject,
    GParamSpec, GValue, g_object_get_data, g_object_set_data, g_signal_connect_object,
    g_value_get_string, g_value_get_uint, g_value_set_string, g_value_set_uint,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::account::{apply, read};
use crate::complete::{check, status_message};
use crate::defaults::from_identity;
use crate::mail::{MAIL_BACKEND_NAME, apply_server};
use crate::oauth2_service;

/// The JMAP account setup backend.
#[repr(C)]
pub struct JmapConfigServiceBackend {
    /// Evolution's; never read by this code, only handed back as the instance
    /// pointer it gave us.
    parent: EMailConfigServiceBackend,
}

/// The class struct. Nothing of ours lives in it; it exists because GObject
/// needs a size to allocate and a place to put the name and the vfunc slots.
#[repr(C)]
pub struct JmapConfigServiceBackendClass {
    pub parent_class: EMailConfigServiceBackendClass,
}

impl JmapConfigServiceBackend {
    /// An instance outside the GObject type system: zeroed parent bytes, which
    /// is what `instance_init` leaves behind minus the GObject.
    ///
    /// As in [`jmap-backend-collection`][detached], this exists because a real
    /// instance cannot be built here. Evolution's `constructed` extends an
    /// `EMailConfigServicePage`, which is a `GtkWidget` and so needs a display
    /// this VM does not have. Nothing may be touched through the result: the
    /// parent bytes are a valid bit pattern but they are not a GObject, so
    /// passing one to any Evolution function is undefined behaviour.
    ///
    /// It is sound for exactly one of this class's vfuncs — `new_collection`,
    /// whose answer is a function of nothing at all — and that is the one
    /// `tests/backend.rs` drives with it.
    ///
    /// [detached]: ../../jmap_backend_collection/backend/struct.JmapCollectionBackend.html#method.detached
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value.
        Box::new(unsafe { MaybeUninit::zeroed().assume_init() })
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the
// EMailConfigServiceBackend instance and class structs respectively, and
// EMailConfigServiceBackend derives from GObject (via EExtension).
unsafe impl ObjectSubclass for JmapConfigServiceBackend {
    const NAME: &'static CStr = c"EMailConfigServiceBackendJmap";
    type Instance = JmapConfigServiceBackend;
    type Class = JmapConfigServiceBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe { e_mail_config_service_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; both fields below are in that half.
        let class = unsafe { &mut (*class).parent_class };
        // The one field that is not a function, and the one the page reads
        // before it calls anything: it is `strcmp`ed against each Camel
        // provider's protocol, so it has to be *the provider's* spelling and
        // not a description. A `'static` pointer, which is what the field
        // means — Evolution keeps the class for the life of the process and
        // never frees this.
        class.backend_name = MAIL_BACKEND_NAME.as_ptr();
        class.new_collection = Some(new_collection);
        class.insert_widgets = Some(insert_widgets);
        class.setup_defaults = Some(setup_defaults);
        class.check_complete = Some(check_complete);
        class.commit_changes = Some(commit_changes);
    }
}

/// What Evolution calls once, from `constructed`, to find out what an account
/// of this type *is* — and, for a groupware provider, the source everything
/// else about the account hangs off.
///
/// The answer is `(transfer full)`: Evolution stores it as the backend's
/// `collection` property and drops the reference with the backend. For an
/// account being *edited* rather than created it is thrown away again
/// immediately, because the page overrides the property with the existing
/// account's own source; this is only ever the new-account case.
///
/// ## Why the whole default account and not just the backend name
///
/// evolution-ews writes one property here — the collection backend name — and
/// leaves the rest to `setup_defaults`. This writes the whole of
/// [`from_identity("")`](crate::defaults::from_identity): the same account,
/// with the one field that needs an address left empty.
///
/// The reason is that `setup_defaults` is not guaranteed to have run by the
/// time anything reads this source, and the fields it would fill are not
/// neutral when absent. `[Collection] MailEnabled` and its two siblings are
/// *false* when unwritten, so a collection carrying only a backend name reads
/// back — through the registry's own reader, which is what
/// `tests/backend.rs` checks it with — as a JMAP account with mail, contacts
/// and calendars all switched off. That is not the account the user asked for
/// and it is not what the dialog shows; it is a difference that would only
/// surface as an account with no children.
///
/// So the source is, from the moment it exists, the account the dialog starts
/// from: all three parts on, TLS on, and nobody and nowhere named yet.
/// `setup_defaults` will narrow it to the address the user typed, and until it
/// exists, [`complete::check`](crate::complete::check) refuses this account for
/// exactly the field that is missing.
///
/// ## Failure
///
/// NULL, which is what Evolution's own implementation returns and what it
/// therefore handles — the backend simply has no collection. It is a bad
/// outcome (an account committed as a lone mail source), but the vfunc has no
/// `GError` and no other way to say so, so both paths that can reach it leave a
/// critical behind: a panic, caught by the guard, and an `e_source_new` that
/// failed.
unsafe extern "C" fn new_collection(backend: *mut EMailConfigServiceBackend) -> *mut ESource {
    // Not read: unlike EWS's, this implementation takes the backend name from
    // the constant the class was initialised from rather than from
    // `GET_CLASS (backend)->backend_name`, which is what lets it be driven
    // without an instance. The two are the same string, asserted in
    // `tests/backend.rs`.
    let _ = backend;

    guard("new_collection", ptr::null_mut(), || {
        let mut error = ptr::null_mut();
        // A scratch source: no D-Bus object, so it is a local `ESource` with a
        // generated uid, which is what Evolution's account editor works on
        // until the registry is asked to create the real one.
        // SAFETY: the documented arguments — a NULL `GDBusObject`, the default
        // main context, and a `GError` out-parameter.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        if source.is_null() {
            // SAFETY: the out-parameter of a call that just failed, so it is
            // NULL or a `GError` this caller owns; consumed exactly once.
            let reason = unsafe { take_message(error) };
            log_critical(&format!(
                "new_collection: could not create the account source: {reason}"
            ));
            return ptr::null_mut();
        }

        // SAFETY: the source was just created and nothing else holds it.
        unsafe { apply(source, &from_identity("")) };
        source
    })
}

/// Whether an [`ENTRY_ROWS`] row is bound straight through or needs a
/// transform first.
///
/// [`Text`](RowKind::Text) covers `host` and `user`: both the entry's `text`
/// and the `ESourceAuthentication` property are strings, so
/// [`e_binding_bind_property`] joins them with no transform function at all.
/// `port` is the one row that is not — a `GtkEntry` has no integer property to
/// bind a `guint16` to — so it is its own case, bound with
/// [`e_binding_bind_property_full`] and [`port_to_text`]/[`text_to_port`].
enum RowKind {
    Text,
    Port,
}

/// The `[Authentication]` fields this dialog lets the user correct, in the
/// order they appear on the page: the mnemonic label's translatable text
/// (marked with [`N_`], looked up with [`translate`] since a `GtkLabel`
/// copies the string GTK's own way and there is no per-call pointer to keep
/// alive as [`i18n::translate_static`](jmap_backend_core::i18n::translate_static)
/// exists for), the `ESourceAuthentication` property the entry is bound to,
/// and how that binding is made.
const ENTRY_ROWS: [(&CStr, &CStr, RowKind); 3] = [
    (N_(c"_Server:"), c"host", RowKind::Text),
    (N_(c"_Port:"), c"port", RowKind::Port),
    (N_(c"_Username:"), c"user", RowKind::Text),
];

/// The mnemonic label of the security check button — its own constant rather
/// than a fourth [`ENTRY_ROWS`] entry, since it is a single check button and
/// not a label-and-entry pair, and binds to `[Security]`, not
/// `[Authentication]`.
const SECURE_LABEL: &CStr = N_(c"Use a _secure connection (TLS)");

/// The mnemonic label of the authentication-method combo — its own constant
/// for the same reason [`SECURE_LABEL`] is one: a single combo, not a
/// label-and-entry pair.
const AUTH_LABEL: &CStr = N_(c"A_uthentication:");

/// The combo's entries: the id [`insert_entries`] writes to
/// `ESourceAuthentication:method` paired with the translatable text the combo
/// shows for it.
///
/// Both ids are load-bearing elsewhere, not chosen for this dialog: `"none"`
/// is `ESourceAuthentication:method`'s own default and the one
/// `crate::account`'s own doc pins as "ask for a password the ordinary way",
/// and [`oauth2_service::NAME`] is the exact string
/// `EOAuth2Service::can_process`'s default implementation compares `method`
/// against (see that module's own doc) — the only spelling of "use OAuth 2.0"
/// `e_source_get_oauth2_access_token_sync` will actually honour for this
/// account, as opposed to the generic `"OAuth2"` alias
/// [`jmap_backend_core::oauth2::method_is_oauth2`] also accepts but
/// `can_process` does not. This is the manual counterpart to
/// [`config_lookup`](crate::config_lookup)'s automatic discovery: a user who
/// skips or fails "Look Up Account Details" still has a way to say a server
/// is OAuth 2.0, and one who used it sees here what it chose.
const AUTH_CHOICES: [(&CStr, &CStr); 2] = [
    (c"none", N_(c"Password")),
    (oauth2_service::NAME, N_(c"OAuth 2.0")),
];

/// The keys [`insert_entries`] stashes the status label and the collection it
/// is computed from under, as `page`'s own qdata — see
/// [`insert_widgets`]'s docs on why [`on_extension_changed`] needs a way to
/// reach them that does not depend on a signal argument it is not handed.
const STATUS_LABEL_KEY: &CStr = c"jmap-config-status-label";
const STATUS_COLLECTION_KEY: &CStr = c"jmap-config-status-collection";

/// What Evolution calls when the *Receiving Email* page is built for this
/// provider: put the entries the user corrects an account's server and login
/// name with into `parent`.
///
/// ## Why this binds to the collection, not to `get_settings()`
///
/// Evolution's own service backends (`e-mail-config-remote-accounts.c`,
/// upstream 3.52.3, read rather than assumed) bind their entries to
/// `e_mail_config_service_backend_get_settings(backend)` — a `CamelSettings`
/// of the *scratch mail source* the page is building. That is right for a
/// provider whose server lives on that source. JMAP's does not:
/// [`check_complete`] and [`commit_changes`] both read and write the
/// *collection* (see their docs for why), so an entry bound to the mail
/// source's settings would show and edit a value neither vfunc looks at. The
/// widgets have to be bound to the same source the rest of this class
/// already agreed the account is.
///
/// ## Why the page is told about a change by hand
///
/// The same upstream file's `mail_config_service_page_new_candidate` connects
/// a `notify` handler on `get_settings()` that calls
/// `e_mail_config_page_changed`, which is how the assistant's *Next* button
/// (and the account editor's *Apply*) learns to ask [`check_complete`] again
/// after a keystroke. That handler watches `get_settings()`, which is exactly
/// the object this class deliberately does not bind to, so this connects its
/// own `notify` handler on the collection's `[Authentication]` extension
/// instead and calls `e_mail_config_page_changed` from it — the same effect,
/// aimed at the object this dialog actually edits.
///
/// `g_signal_connect_object`, not a plain `g_signal_connect`: it disconnects
/// itself the instant either the extension or `page` is finalized, which is
/// what lets this skip a stored handler id and a `dispose` override — neither
/// object's lifetime relative to the other has to be reasoned about for the
/// connection to stay safe.
///
/// ## Where the status label's own refresh keeps its state
///
/// The same `notify` handler ([`on_extension_changed`]) is also what keeps
/// the status label in step after the first fill: it needs the label and the
/// collection, and it is invoked by GLib with neither — only the extension
/// that changed and the `page` this was connected against. So both are
/// stashed as `page`'s own qdata (`g_object_set_data`, already available from
/// `gobject-sys` — no new FFI for it) the moment the label is built, and
/// [`on_extension_changed`] reads them back with `g_object_get_data`. `page`
/// is the right place to hang them: it already outlives every `notify` this
/// class connects, for the reason above.
///
/// ## Untestable here, like the rest of this vfunc
///
/// GTK 3 will not construct a widget without a display connection this
/// machine does not have (see [`evo_sys`]'s module docs). Every call below is
/// one `evo-sys`'s `tests/gtk.rs` and `tests/page.rs` already hold against the
/// linked library and the types it takes; what no test here can do is run
/// this function and see the result. It needs a real Evolution session to
/// confirm the page actually shows three entries, an authentication combo and
/// a check button filled with what `setup_defaults` offered, and that editing
/// any of them toggles *Next* — M9's Xvfb tier launches Evolution but seeds a
/// pre-built `.source` file rather than driving this page, so only a human
/// running the account assistant exercises it. Two operator rounds in real
/// Evolution have now confirmed exactly that (`docs/NIGHT-LOG.md`,
/// `docs/MILESTONES.md`'s `M7 COMPLETE`).
///
/// ## Failure
///
/// Nothing: the vfunc returns void. A NULL collection ([`new_collection`]
/// having failed, which already logged why) leaves the page with no entries
/// at all rather than entries bound to nothing; a panic is caught by the
/// guard and leaves the page in whatever state it reached first.
unsafe extern "C" fn insert_widgets(backend: *mut EMailConfigServiceBackend, parent: *mut GtkBox) {
    guard("insert_widgets", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both come back `(transfer none)` —
        // the backend's own references, which outlive this call.
        let (collection, page) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_page(backend),
            )
        };
        if collection.is_null() {
            return;
        }

        // SAFETY: `collection` is non-NULL and a valid source, just checked;
        // `page` is NULL or the backend's own live page; `parent` is the
        // `GtkBox` Evolution handed this vfunc, which is exactly what
        // `insert_entries` documents it takes.
        unsafe { insert_entries(collection, page, parent) };
    });
}

/// The half of [`insert_widgets`] that touches real objects rather than
/// deciding whether to — see that vfunc's docs for the design this follows.
///
/// # Safety
///
/// `collection` must be a valid `ESource` — the backend's collection, kept
/// alive by the backend for at least the life of the page. `page` must be
/// NULL or a valid `EMailConfigServicePage`. `parent` must be a valid
/// `GtkBox`, the container Evolution handed `insert_widgets`.
unsafe fn insert_entries(
    collection: *mut ESource,
    page: *mut EMailConfigServicePage,
    parent: *mut GtkBox,
) {
    // SAFETY: no arguments; registers the extension types the lookups below
    // need, the same calls `crate::account::apply`/`read` make before every
    // lookup of their own.
    unsafe {
        e_source_authentication_get_type();
        e_source_security_get_type();
    }
    // SAFETY: `collection` is valid by this function's contract, and header
    // constants naming extensions whose types are registered above; each
    // extension is created on demand and owned by the source.
    let (authentication, security) = unsafe {
        (
            e_source_get_extension(collection, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()),
            e_source_get_extension(collection, E_SOURCE_EXTENSION_SECURITY.as_ptr()),
        )
    };

    // SAFETY: no arguments; every GTK call in this function is one
    // `evo-sys`'s `tests/gtk.rs` already resolves against the linked
    // library and holds the types of against the running GTK.
    let grid = unsafe { gtk_grid_new() };
    // SAFETY: `grid` was just constructed above and is a `GtkGrid`.
    unsafe {
        gtk_grid_set_row_spacing(grid.cast(), 6);
        gtk_grid_set_column_spacing(grid.cast(), 6);
    }

    for (row, (label_text, property, kind)) in ENTRY_ROWS.into_iter().enumerate() {
        let label_text = cstring_lossy(&translate(label_text));
        // SAFETY: `label_text` outlives this call, which is all
        // `gtk_label_new_with_mnemonic` needs — it copies the string into
        // the label it returns.
        let label = unsafe { gtk_label_new_with_mnemonic(label_text.as_ptr()) };
        // SAFETY: `label` was just constructed above and is a `GtkLabel`.
        unsafe { gtk_label_set_xalign(label.cast(), 1.0) };

        // SAFETY: no arguments.
        let entry = unsafe { gtk_entry_new() };
        // SAFETY: `label` and `entry` were both just constructed above and
        // are, respectively, a `GtkLabel` and a `GtkWidget`.
        unsafe {
            gtk_label_set_mnemonic_widget(label.cast(), entry);
            gtk_widget_set_hexpand(entry, GTRUE);
            gtk_grid_attach(grid.cast(), label, 0, row as i32, 1, 1);
            gtk_grid_attach(grid.cast(), entry, 1, row as i32, 1, 1);
        }

        // SAFETY: `authentication` is NULL or the collection's own extension
        // (created above, and owned by `collection`), and `entry` is a live
        // `GtkEntry` with a string `text` property; the binding this creates
        // is `(transfer none)`, owned by the two objects it joins, and
        // outlives this call on its own. For `RowKind::Port` the target
        // property is `guint16`, not a string, which is exactly what
        // `port_to_text`/`text_to_port` bridge — their own docs give the
        // `GBindingTransformFunc` contract this satisfies.
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

    // The authentication-method combo: a label-and-widget row like the three
    // entries above, on the row right after them, but bound by `active-id`
    // rather than `text` — see `AUTH_CHOICES`'s own doc for what the two ids
    // mean and why they are the whole list.
    let auth_row = ENTRY_ROWS.len() as i32;
    let label_text = cstring_lossy(&translate(AUTH_LABEL));
    // SAFETY: `label_text` outlives the call, which is all
    // `gtk_label_new_with_mnemonic` needs — it copies the string.
    let auth_label = unsafe { gtk_label_new_with_mnemonic(label_text.as_ptr()) };
    // SAFETY: `auth_label` was just constructed above and is a `GtkLabel`.
    unsafe { gtk_label_set_xalign(auth_label.cast(), 1.0) };

    // SAFETY: no arguments.
    let auth_combo = unsafe { gtk_combo_box_text_new() };
    for (id, text) in AUTH_CHOICES {
        let text = cstring_lossy(&translate(text));
        // SAFETY: `auth_combo` was just constructed above and is a
        // `GtkComboBoxText`; `id` is a `'static` C string and `text` outlives
        // the call — `_append` copies both.
        unsafe { gtk_combo_box_text_append(auth_combo.cast(), id.as_ptr(), text.as_ptr()) };
    }
    // SAFETY: `auth_label` and `auth_combo` were both just constructed above.
    unsafe {
        gtk_label_set_mnemonic_widget(auth_label.cast(), auth_combo.cast());
        gtk_widget_set_hexpand(auth_combo.cast(), GTRUE);
        gtk_grid_attach(grid.cast(), auth_label.cast(), 0, auth_row, 1, 1);
        gtk_grid_attach(grid.cast(), auth_combo.cast(), 1, auth_row, 1, 1);
    }
    // SAFETY: `authentication` is NULL or the collection's own extension
    // (created above, and owned by `collection`); `auth_combo` is a live
    // `GtkComboBox` (a `GtkComboBoxText` is one) with a string `active-id`
    // property, and `AUTH_CHOICES` gave it exactly the two ids `method` can
    // hold, so no transform is needed here either.
    unsafe {
        e_binding_bind_property(
            authentication,
            c"method".as_ptr(),
            auth_combo.cast(),
            c"active-id".as_ptr(),
            G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
        );
    }

    // The security toggle: one check button spanning both columns, on the row
    // after the authentication combo.
    // SAFETY: `label_text` outlives the call, which is all
    // `gtk_check_button_new_with_mnemonic` needs — it copies the string.
    let label_text = cstring_lossy(&translate(SECURE_LABEL));
    let check = unsafe { gtk_check_button_new_with_mnemonic(label_text.as_ptr()) };
    // SAFETY: `check` was just constructed above and is a `GtkWidget`; `grid`
    // is the live grid every other row was attached to.
    unsafe { gtk_grid_attach(grid.cast(), check, 0, auth_row + 1, 2, 1) };
    // SAFETY: `security` is NULL or the collection's own extension (created
    // above, and owned by `collection`), and `check` is a live
    // `GtkToggleButton` with a boolean `active` property — the same shape as
    // `ESourceSecurity:secure`, so no transform is needed here either.
    unsafe {
        e_binding_bind_property(
            security,
            c"secure".as_ptr(),
            check.cast(),
            c"active".as_ptr(),
            G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
        );
    }

    // The status label: a single label spanning both columns, on the row
    // after the check button. `gtk_label_new`, not `_with_mnemonic` — this
    // text is `Incomplete`'s own and not this module's to add a keyboard
    // shortcut to.
    // SAFETY: no arguments.
    let status_label = unsafe { gtk_label_new(ptr::null()) };
    // SAFETY: `status_label` was just constructed above and is a `GtkWidget`;
    // `grid` is the live grid every other row was attached to.
    unsafe {
        gtk_grid_attach(grid.cast(), status_label, 0, auth_row + 2, 2, 1);
    }
    // SAFETY: `status_label` is a live `GtkLabel`, just constructed, and
    // `collection` is a valid source by this function's contract.
    unsafe { set_status_text(status_label, collection) };

    // SAFETY: `grid` is the live `GtkWidget` constructed above, and `parent`
    // is a valid `GtkBox` by this function's contract.
    unsafe {
        gtk_box_pack_start(parent, grid, GFALSE, GFALSE, 0);
        gtk_widget_show_all(grid);
    }

    if !page.is_null() {
        // The status label's own refresh needs to reach it and the
        // collection from `on_extension_changed`, which is handed neither —
        // see `insert_widgets`'s docs on why they are cached as `page`'s own
        // qdata rather than threaded through the signal.
        // SAFETY: `page` is a valid, live GObject by this function's
        // contract, just checked non-NULL; `status_label` and `collection`
        // both outlive it — the label is owned by `grid`, itself now owned by
        // `parent`, and `collection` is the backend's own reference, which
        // outlives the page it built. Neither call transfers ownership (no
        // `GDestroyNotify` is given): this is a cache of pointers already
        // kept alive elsewhere, not a new reference either has to be dropped.
        unsafe {
            g_object_set_data(page.cast(), STATUS_LABEL_KEY.as_ptr(), status_label.cast());
            g_object_set_data(
                page.cast(),
                STATUS_COLLECTION_KEY.as_ptr(),
                collection.cast(),
            );
        }

        // Both extensions get their own connection: `secure` lives on
        // `[Security]`, not `[Authentication]`, and it changes
        // `check_complete`'s answer exactly as much as `host` does — `origin`
        // (`jmap_backend_core::source`) refuses plaintext to a non-loopback
        // host, so turning the toggle off can turn a complete account
        // incomplete. One handler covers both; see its own docs.
        for extension in [authentication, security] {
            if extension.is_null() {
                continue;
            }
            // SAFETY: `extension` is the collection's own live extension,
            // just checked non-NULL, and `page` is a valid page by this
            // function's contract, also just checked non-NULL. The
            // callback's signature matches what a `notify` handler is
            // actually invoked with (the emitting `GObject`, the changed
            // property's `GParamSpec`, and the connection's own data
            // pointer), so the transmute of its type to `GCallback` — an
            // erased function pointer — is the same one every GObject
            // binding spells this, `cancel.rs`'s own `g_cancellable_connect`
            // call included.
            // `g_signal_connect_object` rather than `g_signal_connect_data`:
            // see `insert_widgets`'s docs on why nothing here tracks the
            // handler id.
            unsafe {
                g_signal_connect_object(
                    extension,
                    c"notify".as_ptr(),
                    Some(std::mem::transmute::<
                        unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
                        unsafe extern "C" fn(),
                    >(on_extension_changed)),
                    page.cast(),
                    G_CONNECT_DEFAULT,
                );
            }
        }
    }
}

/// The `notify` handler [`insert_entries`] connects on the collection's
/// `[Authentication]` and `[Security]` extensions: tell the page an entry or
/// the security toggle changed, so `check_complete` is asked again, and
/// refresh the status label to match.
///
/// # Safety
///
/// Called by GLib's signal-emission machinery with the arguments a `notify`
/// handler is always invoked with: the object whose property changed
/// (unused — this handler answers for the connection as a whole, the same
/// way upstream's own `mail_config_service_page_settings_notify_cb` does),
/// the `GParamSpec` of the property that changed (also unused, for the same
/// reason), and — because this was connected with `g_signal_connect_object`
/// against `page` — `page` itself, still alive for the duration of this call
/// (that object's own guarantee, not this function's), and the object
/// [`insert_entries`] stashed the status label and collection pointers on
/// with `g_object_set_data`.
unsafe extern "C" fn on_extension_changed(
    _extension: *mut GObject,
    _pspec: *mut GParamSpec,
    page: gpointer,
) {
    guard("on_extension_changed", (), || {
        // SAFETY: `page` is the `EMailConfigServicePage` this was connected
        // against, by this function's contract, and every `EMailConfigPage`
        // implementor answers to `e_mail_config_page_changed` — see
        // `evo-sys`'s `build.rs` for the upstream reading that makes the
        // cast sound.
        unsafe { e_mail_config_page_changed(page.cast::<EMailConfigPage>()) };

        // SAFETY: `page` is the same object `insert_entries` stashed both
        // keys on, by this function's contract. `g_object_get_data` answers
        // NULL for a key nothing set, which this treats the same as "no
        // label to update" rather than a fault — the only way to reach this
        // handler at all is through the connection `insert_entries` makes
        // right after stashing them, but nothing here has to assume that
        // held.
        let label = unsafe { g_object_get_data(page.cast(), STATUS_LABEL_KEY.as_ptr()) };
        if !label.is_null() {
            // SAFETY: set alongside `label` above, by the same code, so
            // either both are present or neither is.
            let collection =
                unsafe { g_object_get_data(page.cast(), STATUS_COLLECTION_KEY.as_ptr()) };
            // SAFETY: `label` is non-NULL, just checked, and was a live
            // `GtkLabel` when stashed; `collection` is NULL or a valid
            // `ESource`, [`set_status_text`]'s own contract.
            unsafe { set_status_text(label.cast(), collection.cast()) };
        }
    });
}

/// Refreshes `label`'s text from the account `collection` currently says —
/// called once by [`insert_entries`] to fill the label in, and again by
/// [`on_extension_changed`] after every keystroke that can change the answer.
/// Empty, and hidden, once the account is one a commit would accept;
/// [`Incomplete`](crate::complete::Incomplete)'s own text otherwise.
///
/// # Safety
///
/// `label` must be a valid `GtkLabel`. `collection` must be NULL or a valid
/// `ESource`.
unsafe fn set_status_text(label: *mut GtkWidget, collection: *mut ESource) {
    let text = if collection.is_null() {
        // The same silence `is_complete`'s own doc gives for a NULL
        // collection: `new_collection` having failed already logged a
        // critical, and this is not a second place to explain it.
        String::new()
    } else {
        // SAFETY: non-NULL and a valid source by this function's contract.
        status_message(&unsafe { read(collection) })
    };
    let visible = !text.is_empty();
    let text = cstring_lossy(&text);
    // SAFETY: `label` is a live `GtkLabel` by this function's contract;
    // `gtk_label_set_text` copies the string it is given, so `text` need not
    // outlive this call.
    unsafe {
        gtk_label_set_text(label.cast(), text.as_ptr());
        gtk_widget_set_visible(label, if visible { GTRUE } else { GFALSE });
    }
}

/// The `port` row's `GBindingTransformFunc` from the `ESourceAuthentication`
/// side: a `guint16` becomes the text a `GtkEntry` shows, with 0 — the
/// keyfile's spelling of "not set", per `crate::account`'s own doc — shown as
/// an empty entry rather than a literal `0` nobody chose.
///
/// A panic becomes `FALSE`, which is how a `GBindingTransformFunc` says "no
/// value": the binding leaves the entry's text alone rather than writing a
/// half-computed one, and a panic must not cross from here into GLib's
/// binding machinery.
///
/// # Safety
///
/// The arguments are `GBindingTransformFunc`'s: `from_value` holds the source
/// property's value — a `guint`, since this is only ever installed on
/// `ESourceAuthentication:port` — and `to_value` is initialised to the target
/// property's type, a string.
unsafe extern "C" fn port_to_text(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("port_to_text", GFALSE, || {
        // SAFETY: a `GValue` holding the guint this binding's source
        // property is, by the contract above.
        let port = unsafe { g_value_get_uint(from_value) };
        let text = if port == 0 {
            String::new()
        } else {
            port.to_string()
        };
        let text = cstring_lossy(&text);
        // SAFETY: a `GValue` initialised to the string type the target
        // property is; `g_value_set_string` copies the string it is given,
        // so `text` need not outlive this call.
        unsafe { g_value_set_string(to_value, text.as_ptr()) };
        GTRUE
    })
}

/// The `port` row's `GBindingTransformFunc` from the `GtkEntry` side: the
/// text back to a `guint16`, the inverse of [`port_to_text`].
///
/// An empty entry is `0` — "not set", the same as [`port_to_text`] shows it —
/// and anything that is not a bare number in `guint16`'s range is refused by
/// returning `FALSE`, which is how a `GBindingTransformFunc` says "no value":
/// the binding leaves `ESourceAuthentication:port` at whatever it already was
/// rather than writing a port nobody typed. A panic becomes the same `FALSE`,
/// for the reason [`port_to_text`] gives.
///
/// # Safety
///
/// The arguments are `GBindingTransformFunc`'s: `from_value` holds the
/// target property's value — a string, since this is only ever installed
/// with `text` as the target of the binding [`port_to_text`] is the other
/// half of — and `to_value` is initialised to the source property's type, a
/// `guint`.
unsafe extern "C" fn text_to_port(
    binding: *mut GBinding,
    from_value: *const GValue,
    to_value: *mut GValue,
    user_data: gpointer,
) -> gboolean {
    let _ = (binding, user_data);

    guard("text_to_port", GFALSE, || {
        // SAFETY: a `GValue` holding a NUL-terminated string this binding's
        // target property is, by the contract above; the string is the
        // entry's own and outlives only this call, so it is copied rather
        // than held.
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
        // SAFETY: a `GValue` initialised to the guint type the source
        // property is.
        unsafe { g_value_set_uint(to_value, u32::from(port)) };
        GTRUE
    })
}

/// What Evolution calls when the user reaches the server settings page: fill in
/// what can be said about the account from the address alone.
///
/// How many times it is called is Evolution's business, and not something that
/// can be checked here — the assistant's page-preparation order is in a source
/// this machine does not have and a dialog it cannot run. So [`setup`] is
/// written to be right either way: called once it fills the page in, and called
/// again after the user has stepped back to the identity page and forward it
/// keeps whatever they have since typed, unless the address itself has changed.
///
/// ## Where the address comes from
///
/// The `EMailConfigServicePage` this extension extends, which is the one thing
/// any vfunc here asks of Evolution rather than of an `ESource`: the assistant's
/// identity page writes the address onto the page, and
/// `e_mail_config_service_page_get_email_address` is where every provider's
/// `setup_defaults` reads it. A backend with no page — which is not a state
/// Evolution produces, since `constructed` is what sets it — is the empty
/// address, and [`from_identity`] answers that with the account
/// [`new_collection`] already wrote.
///
/// ## Failure
///
/// Nothing: the vfunc returns void and there is nothing to report to. A panic
/// is caught by the guard and leaves a critical; the page then opens on
/// whatever the collection already said, which is [`new_collection`]'s account,
/// and `check_complete` refuses it until the user fills the address in
/// themselves.
unsafe extern "C" fn setup_defaults(backend: *mut EMailConfigServiceBackend) {
    guard("setup_defaults", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both come back `(transfer none)` — the
        // backend's own references, which outlive this call.
        let (page, collection) = unsafe {
            (
                e_mail_config_service_backend_get_page(backend),
                e_mail_config_service_backend_get_collection(backend),
            )
        };

        let address = if page.is_null() {
            None
        } else {
            // SAFETY: a live page, and the address comes back `(transfer none)`
            // as a NUL-terminated string the page owns — copied here rather
            // than held.
            unsafe { read_string(e_mail_config_service_page_get_email_address(page)) }
        };

        // SAFETY: NULL or the backend's live collection source, which is what
        // `setup` documents it takes.
        unsafe { setup(collection, address.as_deref().unwrap_or_default()) };
    });
}

/// Writes onto `collection` what the address implies about the account — the
/// deciding half of the `setup_defaults` vfunc, and the half that can be tested.
///
/// Answers whether it wrote, which is what the tests ask it; the vfunc has
/// nowhere to put the answer and drops it.
///
/// ## Why this is not simply [`from_identity`] applied
///
/// Because it may run more than once. `from_identity` describes a whole account —
/// the address, the server it implies, the login name it offers, *and* the three
/// parts and the TLS switch — and only the first three of those are things the
/// address says. The other two were written by `new_collection` before the
/// user saw the page, and by the time this runs a second time they may be
/// answers the user gave: a *Calendars* box they unticked, on a page they
/// stepped away from to fix a typo in their address. Applying the whole default
/// account again would tick it back.
///
/// So the fields the address determines are taken from the offer and the rest of
/// the account is left as it stands. The join is asserted rather than assumed:
/// on a collection fresh from `new_collection` the result is exactly
/// `from_identity(address)`, which is what `tests/backend.rs` reads back through
/// the registry's own reader.
///
/// ## And why an address that has not changed writes nothing at all
///
/// The same reasoning one step further. If the collection already names this
/// address then the defaults for it have already been offered, and anything the
/// account now says about the server is either that offer or the user's
/// correction of it — a JMAP server may perfectly well not live at the domain of
/// the address, RFC 8620 §2.2 only says that is where to *ask*. Re-deriving
/// would overwrite a correction the user typed, and the trigger for it would be
/// nothing more than having looked at the previous page again.
///
/// A *changed* address is the opposite case and is re-derived: the server the
/// user typed was for the address they have just stopped naming.
///
/// # Safety
///
/// `collection` must be NULL or a valid `ESource` — the backend's collection.
/// It is read and written, and nothing here outlives the call.
pub unsafe fn setup(collection: *mut ESource, address: &str) -> bool {
    if collection.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by this function's contract.
    let mut account = unsafe { read(collection) };
    if account.identity == address {
        return false;
    }

    let offered = from_identity(address);
    account.identity = offered.identity;
    account.connection.host = offered.connection.host;
    account.connection.user = offered.connection.user;

    // SAFETY: as above; `apply` borrows the account for the call only.
    unsafe { apply(collection, &account) };
    true
}

/// What Evolution asks before it lets the assistant move on, and again on every
/// keystroke: is this account finished?
///
/// The answer decides whether *Next* (or *Apply*, on the account editor) is
/// sensitive. It is asked often and it is asked while the user is still typing,
/// so it must be cheap and it must never reach the network —
/// [`complete`](crate::complete) says the same of the decision it makes.
///
/// ## Which source it is a question about
///
/// The *collection*, not the mail source: for a groupware provider the account
/// is the collection [`new_collection`] answered, and the three mail sources are
/// written from it at commit time ([`crate::mail`]). So this reads the account
/// the collection currently says, which is the account a commit would write.
///
/// evolution-ews asks its `CamelEwsSettings` instead, because that is what its
/// entries are bound to. Both are defensible and this crate has picked the
/// collection throughout: it is what [`apply`](crate::account::apply) writes,
/// what [`read`](crate::account::read) reads, and the one description of an
/// account that the collection backend then reads back in another process.
/// `insert_widgets` will have to bind the entries to that same source, and the
/// note is here because a widget bound to anything else would leave this vfunc
/// answering questions about a source nobody was editing.
///
/// ## Failure
///
/// FALSE — the assistant stays where it is, which is the safe direction: a
/// `check_complete` that answered TRUE on a panic would let a half-read account
/// be committed.
unsafe extern "C" fn check_complete(backend: *mut EMailConfigServiceBackend) -> gboolean {
    guard("check_complete", GFALSE, || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both come back `(transfer none)` — the
        // backend's own references, which outlive this call.
        let (collection, page) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_page(backend),
            )
        };

        // Narrow the collection to the address the user typed, if the page knows
        // it. `setup_defaults` is Evolution's intended hook for this, but it does
        // not fire when the user reaches this page by revising an autoconfig
        // result and switching the server type to JMAP, and at `insert_widgets`
        // construction time the page's address is not set yet. `check_complete`
        // is asked again every time the page changes — i.e. after the address has
        // propagated — so it is where the narrow reliably lands on that path.
        // `setup` writes only when the address differs from the one the collection
        // already names: a no-op once narrowed, and it never overwrites a host the
        // user corrected by hand unless they also changed the address it was for.
        if !page.is_null() {
            // SAFETY: `page` is the backend's own live page, just checked
            // non-NULL; the address returns `(transfer none)` as a NUL-terminated
            // string the page owns, copied out by `read_string`.
            let address =
                unsafe { read_string(e_mail_config_service_page_get_email_address(page)) };
            if let Some(address) = address.filter(|a| !a.trim().is_empty()) {
                // SAFETY: `collection` is NULL or the backend's live collection
                // source; `setup` handles NULL and nothing here outlives the call.
                unsafe { setup(collection, &address) };
            }
        }

        // SAFETY: NULL or the backend's live collection source, which is
        // exactly what `is_complete` documents it takes.
        if unsafe { is_complete(collection) } {
            GTRUE
        } else {
            GFALSE
        }
    })
}

/// Whether the account `collection` says is one the setup may commit — the
/// deciding half of the `check_complete` vfunc, and the half that can be tested.
///
/// [`read`] and then [`check`], which is the whole of it. The two exist
/// separately because the account in a dialog being typed into is usually not an
/// account yet: the reader is total and says what the source holds, and the
/// check is what has an opinion about it.
///
/// ## A NULL collection is FALSE, and says nothing about it
///
/// There is one way to get here without a collection: `new_collection`
/// failed. It logs a critical when it does, which is where the failure happened
/// and the only place it can be explained; repeating it here would be the same
/// message once per keystroke, burying the original in copies of itself. So this
/// is silent, and answers the refusal that keeps an unreadable account from
/// being committed.
///
/// The refusal of a *legitimately* unfinished account is silent for a different
/// reason: [`Incomplete`](crate::complete::Incomplete) is written to be read by
/// the person who typed the answer, in the entry they typed it into, and this
/// vfunc has no entry to put it in — it answers a boolean. `insert_widgets`'s
/// status label is where the same reason lands instead, computed separately
/// there ([`crate::complete::status_message`]) rather than threaded through
/// this boolean.
///
/// # Safety
///
/// `collection` must be NULL or a valid `ESource`. It is only read from and
/// nothing here outlives the call.
pub unsafe fn is_complete(collection: *mut ESource) -> bool {
    if collection.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by this function's contract.
    check(&unsafe { read(collection) }).is_ok()
}

/// What Evolution calls once *Next* has been pressed for the last time: write
/// the account on screen into the scratch sources, which the assistant then
/// hands to `e_source_registry_create_sources` as one batch.
///
/// The vfunc creates nothing and saves nothing. Everything it touches is a
/// scratch `ESource` that is already queued for creation, so what it is for is
/// the fields nobody else fills in.
///
/// ## Which those are, for JMAP, and why they are so few
///
/// For every provider whose server the user typed on *this* page — IMAP, SMTP,
/// POP3 — there is nothing to do at all, and Evolution's own implementation
/// accordingly does nothing: the entries were bound through `CamelSettings` onto
/// the mail source's own `[Authentication]` and `[Security]` as they were typed
/// into. JMAP asks for a server once, on the account, because one JMAP session
/// carries the mail, the contacts and the calendars; so the mail source is a
/// second file that nothing has told, and telling it is this vfunc's whole job.
///
/// What is *not* here, because it is already written by the time this runs:
///
/// - **The service name.** `e_mail_config_assistant` mints one scratch source
///   per provider and writes the protocol into it before any backend sees it —
///   that is how [`new_collection`]'s candidate was picked in the first place.
/// - **The parent and the two uid links.** `EMailConfigSummaryPage`'s own
///   `commit_changes` sets all three sources' `Parent` to the collection's uid,
///   the account's `identity-uid` and the identity's `transport-uid`. It runs
///   after this one, from the assistant's page loop, and it is the same wiring
///   [`crate::mail::apply`] does — which is why that one is the *account's*
///   writer and this is not a call to it.
/// - **The identity's address**, which is `EMailConfigIdentityPage`'s.
///
/// ## One backend per page, and the emptiness that has to be refused
///
/// Evolution instantiates this class once for the *Receiving Email* page and
/// once for *Sending Email*, and `constructed` calls [`new_collection`] on each
/// — so the sending instance holds a scratch collection of its own that no
/// widget will ever fill in and that the assistant will never queue. Reached
/// with that one, a commit that simply copied would write an empty host onto the
/// transport source, and an empty host is not the same as an unwritten one: it
/// reads back as an account that names a server.
///
/// So the copy happens only for a collection that is an account the setup would
/// commit — [`is_complete`], the same question [`check_complete`] answers, which
/// is true of the receiving instance's collection (that is what let *Next* be
/// pressed) and false of the sending instance's. Silent either way: a refusal
/// here is either that second instance, which is not a fault, or an account
/// `check_complete` already refused to let the user leave the page with.
///
/// ## The gap this leaves, said plainly
///
/// On the assistant's path the transport source therefore ends up with the
/// service name `jmap` and no server, and JMAP submission needs one. Nothing
/// else in the dialog is in a position to write it: the sending page is hidden
/// for a store-and-transport provider, and the backend that is its candidate
/// cannot see the account. The place that can is the collection backend, in
/// `evolution-source-registry`, which is handed the account and can walk
/// `e_collection_backend_list_mail_sources()` for the children that hang off it
/// — and that is the next increment, not something to fake here by writing a
/// host this backend does not know.
unsafe extern "C" fn commit_changes(backend: *mut EMailConfigServiceBackend) {
    guard("commit_changes", (), || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. Both come back `(transfer none)` — the
        // backend's own references, which outlive this call.
        let (collection, source) = unsafe {
            (
                e_mail_config_service_backend_get_collection(backend),
                e_mail_config_service_backend_get_source(backend),
            )
        };
        // SAFETY: each is NULL or one of the backend's live sources, which is
        // exactly what `commit` documents it takes.
        unsafe { commit(collection, source) };
    });
}

/// Copies the server the account names onto the one mail source a backend
/// holds — the writing half of the `commit_changes` vfunc, and the half that can
/// be tested.
///
/// Answers whether it wrote, which is what the tests ask it; the vfunc has
/// nowhere to put the answer and drops it.
///
/// Nothing happens without both sources, and nothing happens for an account that
/// is not finished — see the `commit_changes` vfunc above for why the second is
/// a normal outcome rather than a failure, and why neither is logged.
///
/// # Safety
///
/// `collection` and `source` must each be NULL or a valid `ESource`. The first
/// is only read from, the second is written to, and nothing here outlives the
/// call.
pub unsafe fn commit(collection: *mut ESource, source: *mut ESource) -> bool {
    if collection.is_null() || source.is_null() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by this function's contract.
    let account = unsafe { read(collection) };
    if check(&account).is_err() {
        return false;
    }

    // SAFETY: non-NULL and a valid source by this function's contract; the
    // connection outlives the call, which is all `apply_server` borrows.
    unsafe { apply_server(source, &account.connection) };
    true
}

/// The message a failed EDS call left behind, consuming the `GError`.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        return "EDS set no error".to_owned();
    }

    // SAFETY: a live GError; its message is a NUL-terminated string it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: ownership passed to us with the out-parameter.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "EDS gave no message".to_owned())
}

/// [`port_to_text`] and [`text_to_port`], driven directly with hand-built
/// `GValue`s rather than through a real `GBinding` — which is what lets these
/// run here at all: they are the one piece of `insert_widgets` that touches no
/// widget, so unlike the rest of this module they need no display connection.
#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::mem::zeroed;

    use gobject_sys::{G_TYPE_STRING, G_TYPE_UINT, g_value_init, g_value_unset};

    use super::*;

    /// A zeroed, initialised `GValue` of the given type — the C
    /// `GValue value = G_VALUE_INIT; g_value_init(&value, type);` idiom,
    /// as `tests/oauth2_service.rs` also spells it.
    ///
    /// # Safety
    ///
    /// `gtype` must be a `GType` `g_value_init` accepts; the caller must
    /// `g_value_unset` the result when done with it.
    unsafe fn value_of(gtype: glib_sys::GType) -> GValue {
        // SAFETY: a fresh `GValue` has no prior contents to unset, so
        // zero-initialising it before `g_value_init` is exactly the C idiom.
        unsafe {
            let mut value: GValue = zeroed();
            g_value_init(&mut value, gtype);
            value
        }
    }

    /// A string `GValue`'s contents, exactly — unlike [`read_string`], which
    /// this module otherwise uses throughout, an empty string here is not the
    /// same absence a keyfile's unwritten key is; `port_to_text` writes a real
    /// empty string for "no port", which is exactly what these tests need to
    /// tell apart from a NULL one.
    ///
    /// # Safety
    ///
    /// `value` must hold a string, as `g_value_get_string` requires.
    unsafe fn string_of(value: &GValue) -> Option<String> {
        // SAFETY: by this function's contract.
        let ptr = unsafe { g_value_get_string(value) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string owned by `value`, copied out here.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    #[test]
    fn port_to_text_shows_zero_as_an_empty_entry() {
        // SAFETY: `from` is a `guint` GValue, as `port_to_text` requires; `to`
        // is a string GValue, as it fills in; both are unset before the test
        // ends.
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

    /// The round trip [`port_to_text`]/[`text_to_port`] is meant to be: what
    /// one shows is what the other reads back to the same port.
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
            // `g_value_set_string` copies.
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
                // A sentinel the binding must leave alone on refusal — this
                // test only checks the return value, but a real bidirectional
                // `GBinding` relies on exactly this: `text_to_port` returning
                // `FALSE` is what keeps a bad keystroke from ever reaching
                // `g_object_set_property`.
                g_value_set_uint(&mut to, 12345);

                let ok = text_to_port(ptr::null_mut(), &from, &mut to, ptr::null_mut());

                assert_eq!(ok, GFALSE, "{text:?} should not be a valid port");

                g_value_unset(&mut from);
                g_value_unset(&mut to);
            }
        }
    }
}
