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
//! `insert_widgets` now builds two of the five fields
//! [`Connection`](jmap_collection_sync::child_source::Connection) carries —
//! the server and the login name — bound to the collection the same way
//! `check_complete` and `commit_changes` already read and write it. Three
//! more things are still missing, and are recorded here rather than silently
//! absent:
//!
//! - **A port entry and a security toggle.** `port` and `secure` stay at
//!   [`from_identity`]'s offer (443, TLS) for
//!   every account this dialog can create; a deployment on a non-standard
//!   port or without TLS cannot be set up here yet.
//! - **A status label.** [`Incomplete`](crate::complete::Incomplete)'s refusal
//!   reason is computed by [`is_complete`] and thrown away — there is nowhere
//!   on the page for it to land, so the user sees *Next* refuse to light up
//!   with no explanation of why.
//! - **Verification in a real Evolution.** GTK 3 will not construct a widget
//!   without a display connection, so nothing on this machine has run
//!   `insert_widgets` and looked at the result — see its own docs for exactly
//!   what a human still has to confirm, and `docs/NIGHT-LOG.md` for the
//!   session that wrote it saying so.
//!
//! ## The state this leaves the dialog in, said plainly
//!
//! An account whose address the assistant already knows arrives on the server
//! settings page filled in — the address, the server its domain implies and
//! the login name it offers — and now carries two entries to correct either
//! of those from, bound live to the account `check_complete` and
//! `commit_changes` both read. What is still missing is a way to correct the
//! port or turn TLS off, and any explanation on the page itself for why *Next*
//! refuses to light up when it does.
//!
//! [`evo-sys`]: ../../evo_sys/index.html

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, e_binding_bind_property,
    e_source_authentication_get_type, e_source_get_extension, e_source_new,
};
use evo_sys::{
    EMailConfigPage, EMailConfigServiceBackend, EMailConfigServiceBackendClass,
    EMailConfigServicePage, GtkBox, e_mail_config_page_changed,
    e_mail_config_service_backend_get_collection, e_mail_config_service_backend_get_page,
    e_mail_config_service_backend_get_source, e_mail_config_service_backend_get_type,
    e_mail_config_service_page_get_email_address, gtk_box_pack_start, gtk_entry_new,
    gtk_grid_attach, gtk_grid_new, gtk_grid_set_column_spacing, gtk_grid_set_row_spacing,
    gtk_label_new_with_mnemonic, gtk_label_set_mnemonic_widget, gtk_label_set_xalign,
    gtk_widget_set_hexpand, gtk_widget_show_all,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_free, gboolean, gpointer};
use gobject_sys::{
    G_BINDING_BIDIRECTIONAL, G_BINDING_SYNC_CREATE, G_CONNECT_DEFAULT, GObject, GParamSpec,
    g_signal_connect_object,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::account::{apply, read};
use crate::complete::check;
use crate::defaults::from_identity;
use crate::mail::{MAIL_BACKEND_NAME, apply_server};

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

/// The two `[Authentication]` fields this dialog lets the user correct, in the
/// order they appear on the page: the mnemonic label's translatable text
/// (marked with [`N_`], looked up with [`translate`] since a `GtkLabel`
/// copies the string GTK's own way and there is no per-call pointer to keep
/// alive as [`i18n::translate_static`](jmap_backend_core::i18n::translate_static)
/// exists for) and the `ESourceAuthentication` property the entry is bound
/// to.
///
/// Two rows and not five: [`crate::account::Connection`]'s `port` and
/// `secure` have no entry yet, and neither does a status label for
/// [`Incomplete`](crate::complete::Incomplete)'s refusal reason — see
/// [`insert_widgets`] for why this increment stops here.
const ENTRY_ROWS: [(&CStr, &CStr); 2] = [(N_(c"_Server:"), c"host"), (N_(c"_Username:"), c"user")];

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
/// ## What is not here yet
///
/// A port entry, a security toggle, and the status label
/// [`Incomplete`](crate::complete::Incomplete)'s refusal reason belongs in —
/// the crate's own top-level docs and this module's say so at length. Port and
/// security stay at [`from_identity`]'s defaults (443, TLS) for this
/// increment; a deployment that genuinely needs a different port cannot be
/// set up through this dialog yet, and that is a real gap this records
/// rather than works around.
///
/// ## Untestable here, like the rest of this vfunc
///
/// GTK 3 will not construct a widget without a display connection this
/// machine does not have (see [`evo_sys`]'s module docs). Every call below is
/// one `evo-sys`'s `tests/gtk.rs` and `tests/page.rs` already hold against the
/// linked library and the types it takes; what no test here can do is run
/// this function and see the result. It needs a real Evolution session (or
/// M9's Xvfb tier) to confirm the page actually shows two entries filled with
/// what `setup_defaults` offered, and that editing either toggles *Next* —
/// recorded in `docs/NIGHT-LOG.md` as exactly that, and not tagged complete
/// until a human confirms it.
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
    // SAFETY: no arguments; registers the extension type the lookup below
    // needs, the same call `crate::account::apply`/`read` make before every
    // lookup of their own.
    unsafe { e_source_authentication_get_type() };
    // SAFETY: `collection` is valid by this function's contract, and a
    // header constant naming an extension whose type is registered above;
    // the extension is created on demand and owned by the source.
    let authentication =
        unsafe { e_source_get_extension(collection, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) };

    // SAFETY: no arguments; every GTK call in this function is one
    // `evo-sys`'s `tests/gtk.rs` already resolves against the linked
    // library and holds the types of against the running GTK.
    let grid = unsafe { gtk_grid_new() };
    // SAFETY: `grid` was just constructed above and is a `GtkGrid`.
    unsafe {
        gtk_grid_set_row_spacing(grid.cast(), 6);
        gtk_grid_set_column_spacing(grid.cast(), 6);
    }

    for (row, (label_text, property)) in ENTRY_ROWS.into_iter().enumerate() {
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
        // outlives this call on its own.
        unsafe {
            e_binding_bind_property(
                authentication,
                property.as_ptr(),
                entry.cast(),
                c"text".as_ptr(),
                G_BINDING_BIDIRECTIONAL | G_BINDING_SYNC_CREATE,
            );
        }
    }

    // SAFETY: `grid` is the live `GtkWidget` constructed above, and `parent`
    // is a valid `GtkBox` by this function's contract.
    unsafe {
        gtk_box_pack_start(parent, grid, GFALSE, GFALSE, 0);
        gtk_widget_show_all(grid);
    }

    if !page.is_null() && !authentication.is_null() {
        // SAFETY: `authentication` is the collection's own live extension,
        // just checked non-NULL, and `page` is a valid page by this
        // function's contract, also just checked non-NULL. The callback's
        // signature matches what a `notify` handler is actually invoked
        // with (the emitting `GObject`, the changed property's
        // `GParamSpec`, and the connection's own data pointer), so the
        // transmute of its type to `GCallback` — an erased function pointer
        // — is the same one every GObject binding spells this, `cancel.rs`'s
        // own `g_cancellable_connect` call included.
        // `g_signal_connect_object` rather than `g_signal_connect_data`: see
        // `insert_widgets`'s docs on why nothing here tracks the handler id.
        unsafe {
            g_signal_connect_object(
                authentication,
                c"notify".as_ptr(),
                Some(std::mem::transmute::<
                    unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
                    unsafe extern "C" fn(),
                >(on_authentication_changed)),
                page.cast(),
                G_CONNECT_DEFAULT,
            );
        }
    }
}

/// The `notify` handler [`insert_entries`] connects on the collection's
/// `[Authentication]` extension: tell the page an entry changed, so
/// `check_complete` is asked again.
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
/// (that object's own guarantee, not this function's).
unsafe extern "C" fn on_authentication_changed(
    _authentication: *mut GObject,
    _pspec: *mut GParamSpec,
    page: gpointer,
) {
    guard("on_authentication_changed", (), || {
        // SAFETY: `page` is the `EMailConfigServicePage` this was connected
        // against, by this function's contract, and every `EMailConfigPage`
        // implementor answers to `e_mail_config_page_changed` — see
        // `evo-sys`'s `build.rs` for the upstream reading that makes the
        // cast sound.
        unsafe { e_mail_config_page_changed(page.cast::<EMailConfigPage>()) };
    });
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
        // dispatches through this slot. The collection comes back
        // `(transfer none)` — the backend's own reference, which outlives this
        // call — and is only read from.
        let collection = unsafe { e_mail_config_service_backend_get_collection(backend) };
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
/// vfunc has no entry to put it in — it answers a boolean. The place for it is
/// the status label `insert_widgets` will add, and until that exists the reason
/// is produced and dropped rather than logged where nobody is looking for it.
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
