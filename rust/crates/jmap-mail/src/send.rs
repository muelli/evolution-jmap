// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `send_to_sync`: the vfunc a message leaves the account through.
//!
//! It is the only thing `CamelTransportClass` declares, and the only reason
//! [`crate::transport`] is a type of its own. What reaches it is a
//! `CamelMimeMessage` the composer built and two `CamelAddress` lists; what has
//! to come back is TRUE or FALSE, an error, and one out-parameter. Everything
//! in between was built and tested an increment at a time, and this module is
//! the join:
//!
//! 1. [`read_envelope`] turns the two address lists into RFC 8621 §7's
//!    `envelope` — the addresses the mail is actually delivered to, which are
//!    not the ones in the headers.
//! 2. [`write_message`] turns the object into the RFC 5322 bytes that go up,
//!    through Camel's own emitter.
//! 3. [`JmapTransport::send_message`] does the rest over the connection: the
//!    identity, the two mailboxes, the import and the submission.
//!
//! ## The order is the point
//!
//! Both of the first two steps can fail, and neither needs a server, so both
//! happen before the connection is looked for. That is not tidiness: a refusal
//! here costs nothing, whereas the same refusal made after the import would
//! leave a draft behind in the user's account for a send they were told did not
//! happen. [`crate::envelope`] is built on exactly that argument, and this is
//! where it is honoured.
//!
//! The envelope goes first of the two because it is the cheaper: an address
//! list is a handful of strings, and writing the message out allocates the
//! whole of it.
//!
//! ## `out_sent_message_saved`
//!
//! Camel's one out-parameter besides the error, and it is not decoration: it
//! asks whether the transport has already saved the sent copy, and Evolution
//! appends a copy of its own to the account's sent folder when it is told
//! `FALSE`. Both mistakes are visible to the user — `FALSE` when the copy is in
//! Sent gives them two of every message they send, and `TRUE` when it is not
//! loses the copy — so it is answered from what actually happened to the
//! message, [`Sent::saved`](crate::transport::Sent::saved), and not from
//! whether a mailbox move was needed.
//!
//! It is written before anything can fail, as well as on success. Camel's own
//! `camel_transport_send_to_sync` clears it first, but the vfunc is a slot any
//! caller may dispatch directly, and an out-parameter this one left untouched
//! on a failure path would be whatever the caller's stack held.
//!
//! ## What is not here
//!
//! Nothing about cancellation beyond the scope every vfunc installs. A send is
//! an upload and two requests, so it is one of the longest operations this
//! provider makes, and the `cancellable` Camel passes is [`observe`]d for the
//! length of the call exactly as [`crate::append`]'s is.
//!
//! Nor any retry. A submission the server refused is a message safe in the
//! staging mailbox and a sentence for the user; a transport that tried again on
//! their behalf would risk sending twice what it could not prove was not sent
//! once.

use std::ffi::c_int;

use eds_sys::{
    CAMEL_SERVICE_ERROR_INVALID, CamelAddress, CamelMimeMessage, CamelTransport,
    CamelTransportClass, camel_service_error_quark,
};
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, GTRUE, gboolean};
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{fail_bool, set_raw_gerror};
use jmap_backend_core::trampoline::guard_bool;

use crate::connect::StoreError;
use crate::envelope::{EnvelopeError, read_envelope};
use crate::transport::JmapTransport;
use jmap_backend_core::mime::{Unwritable, write_message};

/// Installs the send vfunc on a class whose first member is a
/// `CamelTransportClass`.
///
/// # Safety
///
/// `class` must point at an initialised class struct that leads with a
/// `CamelTransportClass` — which is every descendant of `CamelTransport`.
pub unsafe fn install_vfuncs(class: *mut CamelTransportClass) {
    // SAFETY: the contract above.
    let vfuncs = unsafe { &mut *class };
    vfuncs.send_to_sync = Some(send_to_sync);
}

/// Sends one message through the account this transport belongs to.
///
/// `TRUE` on success with `out_sent_message_saved` filled in, `FALSE` with the
/// error set otherwise — Camel's convention, and what
/// `e_mail_session_send_to` tests before it takes the message out of the
/// outbox.
unsafe extern "C" fn send_to_sync(
    transport: *mut CamelTransport,
    message: *mut CamelMimeMessage,
    from: *mut CamelAddress,
    recipients: *mut CamelAddress,
    out_sent_message_saved: *mut gboolean,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Camel's contract for the vfunc: a valid instance of ours, a live
    // message, two live addresses, and out-parameters that are NULL or
    // writable.
    unsafe {
        guard_bool("send_to_sync", error, || {
            // SAFETY: Camel keeps its cancellable alive for the length of the
            // call, so it outlives this observation — which is what makes the
            // upload and both requests stop when the user presses Stop.
            let _cancel = observe(cancellable);

            // Before anything that can fail, for the module docs' reason: a
            // caller dispatching the slot itself has not necessarily cleared
            // it.
            //
            // SAFETY: NULL or writable, by this block's contract.
            report_saved(out_sent_message_saved, false);

            // SAFETY: two live addresses, by this block's contract.
            let envelope = match read_envelope(from, recipients) {
                Ok(envelope) => envelope,
                Err(refusal) => return unsendable(error, &refusal),
            };

            // SAFETY: the message is live, by this block's contract.
            let source = match write_message(message) {
                Ok(source) => source,
                Err(failure) => return unwritable(error, failure),
            };

            // SAFETY: as above; a NULL is the only thing `borrow` rejects, and
            // Camel dispatches this slot on instances of our type.
            let Some(transport) = JmapTransport::borrow(transport) else {
                return fail_bool(error, &StoreError::Disconnected, StoreError::to_gerror);
            };

            match transport.send_message(source, envelope) {
                Ok(sent) => {
                    // SAFETY: NULL or writable, as above.
                    report_saved(out_sent_message_saved, sent.saved);
                    GTRUE
                }
                Err(problem) => fail_bool(error, &problem, StoreError::to_gerror),
            }
        })
    }
}

/// Says whether the account already holds the sent copy where sent mail
/// belongs.
///
/// # Safety
///
/// `out` must be NULL or writable.
unsafe fn report_saved(out: *mut gboolean, saved: bool) {
    if out.is_null() {
        return;
    }
    // SAFETY: the contract above.
    unsafe { *out = if saved { GTRUE } else { GFALSE } };
}

/// Reports a pair of addresses that does not describe a deliverable message.
///
/// [`EnvelopeError`] names its own domain and code — the service's
/// `CAMEL_SERVICE_ERROR_INVALID`, deliberately not `UNAVAILABLE`, which is what
/// Evolution reads to put an account offline. Nothing is wrong with the account
/// and this send would fail identically against a working server.
///
/// # Safety
///
/// `error` must meet [`set_raw_gerror`]'s contract.
unsafe fn unsendable(error: *mut *mut GError, refusal: &EnvelopeError) -> gboolean {
    // SAFETY: `to_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe { set_raw_gerror(error, refusal.to_gerror()) };
    GFALSE
}

/// Reports a message Camel's own emitter would not write out.
///
/// [`crate::append`] does the same thing in the folder's domain; here it is the
/// service's, because a `CamelTransport` is a `CamelService` and there is no
/// folder in this call to blame. Camel's own account of the failure is passed
/// through untouched either way — [`Unwritable::into_gerror`] only names what
/// an *unexplained* refusal is called.
///
/// # Safety
///
/// `error` must meet [`set_raw_gerror`]'s contract.
unsafe fn unwritable(error: *mut *mut GError, failure: Unwritable) -> gboolean {
    // SAFETY: no arguments, and the quark registers itself.
    let quark = unsafe { camel_service_error_quark() };
    // SAFETY: `into_gerror` hands over an owned GError, and `error` meets
    // `set_raw_gerror`'s contract by this function's.
    unsafe {
        set_raw_gerror(
            error,
            failure.into_gerror(quark, CAMEL_SERVICE_ERROR_INVALID as c_int),
        )
    };
    GFALSE
}
