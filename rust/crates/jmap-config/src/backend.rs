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
//! `insert_widgets`, `setup_defaults`, `check_complete` and `commit_changes`.
//! The first two need the `EMailConfigServicePage` this extension extends —
//! for the entries themselves, and for the email address the user typed on the
//! page before, which is [`defaults::from_identity`](crate::defaults::from_identity)'s
//! one input — and reaching either means binding more of Evolution's headers
//! than [`evo-sys`] currently generates, GTK among them. The second two need
//! the inverse of [`account::apply`](crate::account::apply): a read of the
//! account back out of the collection source the widgets have been editing,
//! which this crate does not have yet. All four are the next increments, and
//! none of them is claimed here.
//!
//! [`evo-sys`]: ../../evo_sys/index.html

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;

use eds_sys::{ESource, e_source_new};
use evo_sys::{
    EMailConfigServiceBackend, EMailConfigServiceBackendClass,
    e_mail_config_service_backend_get_type,
};
use glib_sys::{GError, GType, g_error_free};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::account::apply;
use crate::defaults::from_identity;
use crate::mail::MAIL_BACKEND_NAME;

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
