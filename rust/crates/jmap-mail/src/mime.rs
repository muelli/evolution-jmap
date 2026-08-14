// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelMimeMessage` as the octets a JMAP request uploads.
//!
//! Everything this account puts on the server arrives as an object and goes up
//! as a blob, and this is the step between. Two callers need it and they are
//! not the same kind of thing: [`crate::append`] is a folder taking in a
//! message from outside the account, and [`crate::transport`]'s `send_to_sync`
//! is a service with no folder in the call at all sending one out. One function
//! rather than one each, because a second emitter is a second place a
//! difference could appear between the message the user filed and the message
//! they sent.
//!
//! ## Camel's writer, for [`crate::message`]'s reason turned around
//!
//! The bytes come from `camel_data_wrapper_write_to_output_stream_sync`, which
//! is Camel's own RFC 5322 emitter reached through the message's
//! `CamelDataWrapper` face. That is the mirror of the decision
//! [`crate::message`] takes about the parse: the object has to agree with every
//! other part of Camel about what the message says, and a provider that wrote
//! headers itself would be a second, disagreeing MIME implementation inside the
//! same process — one whose disagreement is *stored*, because what goes up is
//! what the account holds from then on.
//!
//! A `GMemoryOutputStream` rather than a `CamelStreamMem`: the destination is a
//! buffer either way, and the GIO one is the object Camel's own stream class is
//! a wrapper around. The whole message is in memory twice for the length of the
//! upload — the stream's copy and the request's — which is the same cost
//! [`crate::message`] pays in the other direction.
//!
//! ## The one thing added to what Camel wrote: the line endings
//!
//! Camel's emitter writes the message in Camel's *internal* form, whose lines
//! end with a bare LF; converting to CRLF is done by a
//! `CamelMimeFilterCrlf` its own transports put between the message and the
//! socket. This provider is a transport too — and an importer besides — and
//! both of its callers put the bytes somewhere that outlives the call, so the
//! conversion happens here or nowhere.
//!
//! Nowhere is not an option. RFC 5322 §2.1 defines a line as CRLF-terminated,
//! RFC 8621 §4.8 imports "an RFC 5322 message", and RFC 5321 §2.3.8 forbids a
//! bare LF in what an SMTP server is handed — which is what an
//! `EmailSubmission` eventually hands one. A message stored with bare LFs is
//! one whose DKIM signature is computed over different bytes than the recipient
//! verifies, and whose body a strict relay may cut short.
//!
//! `crlf` is therefore the same rule Camel's own filter applies in
//! `CAMEL_MIME_FILTER_CRLF_ENCODE`: a CR is inserted before an LF that has none
//! already, and nothing else is touched — not a lone CR, and deliberately not
//! the leading dots `CRLF_MODE_CRLF_DOTS` would stuff, which are an SMTP
//! wire-level escape and would end up *in* an imported message. Written here
//! rather than reached for through `CamelStream`: the filter is a
//! `CamelStream`-era API this crate otherwise has no use for, and the rule is
//! four lines whose edge cases can be tested directly.
//!
//! The exposure it carries is Camel's own, unchanged: a part whose content is
//! raw 8-bit or binary rather than transfer-encoded has its LFs rewritten too,
//! exactly as it would going out through `camel-smtp-transport`.
//!
//! ## The failure has no domain of its own
//!
//! A message its own writer will not write out is not something this provider
//! has a better account of than the writer does, so Camel's `GError` is passed
//! through untouched. What cannot be passed through is the failure Camel
//! reports *without* setting one: something has to be reported — a vfunc
//! answering FALSE with no error set earns a GLib critical — and the domain
//! that answer belongs in is the caller's, not this module's. An append is a
//! `CAMEL_FOLDER_ERROR`, because a folder is what Camel asked; a send is a
//! `CAMEL_SERVICE_ERROR`, because a transport has no folder to name. Hence
//! [`Unwritable`], which is the failure without a domain, and
//! [`Unwritable::into_gerror`], where the caller supplies one.

use std::ffi::c_int;
use std::mem::ManuallyDrop;
use std::ptr;

use eds_sys::{CamelDataWrapper, CamelMimeMessage, camel_data_wrapper_write_to_output_stream_sync};
use gio_sys::{
    GMemoryOutputStream, GOutputStream, g_memory_output_stream_get_data,
    g_memory_output_stream_get_data_size, g_memory_output_stream_new_resizable,
    g_output_stream_flush,
};
use glib_sys::{GError, GFALSE, GQuark, g_error_free, g_error_new_literal};
use gobject_sys::g_object_unref;

/// What is reported when Camel refused to write a message out and said nothing
/// about why.
///
/// One sentence for both callers, because it is one thing that went wrong; only
/// the domain it is reported in differs, and that is
/// [`Unwritable::into_gerror`]'s argument.
const UNEXPLAINED: &std::ffi::CStr = c"the message could not be written out";

/// The message as the octets an `Email/import` upload takes.
///
/// The stream is flushed before its buffer is read. `GMemoryOutputStream` does
/// not buffer, but the writer above it is free to wrap it — and a message that
/// arrived truncated by however much a filter was still holding would be a
/// silently corrupted message on the server.
///
/// The buffer is copied out rather than stolen, so that the stream's own free
/// function stays the one that releases it; the upload needs an owned `Vec`
/// either way.
///
/// # Safety
///
/// `message` must be NULL or point at a live `CamelMimeMessage`.
pub unsafe fn write_message(message: *mut CamelMimeMessage) -> Result<Vec<u8>, Unwritable> {
    // SAFETY: a fresh resizable memory stream, the message by this function's
    // contract, and an error out-parameter that is a local starting NULL.
    unsafe {
        let stream: *mut GOutputStream = g_memory_output_stream_new_resizable();
        let mut failure: *mut GError = ptr::null_mut();
        let written = camel_data_wrapper_write_to_output_stream_sync(
            message.cast::<CamelDataWrapper>(),
            stream,
            ptr::null_mut(),
            &mut failure,
        );
        let flushed =
            written >= 0 && g_output_stream_flush(stream, ptr::null_mut(), &mut failure) != GFALSE;

        if !flushed {
            g_object_unref(stream.cast());
            return Err(Unwritable(failure));
        }

        let stream = stream.cast::<GMemoryOutputStream>();
        let data = g_memory_output_stream_get_data(stream).cast::<u8>();
        let len = g_memory_output_stream_get_data_size(stream);
        let source = match data.is_null() {
            true => Vec::new(),
            false => std::slice::from_raw_parts(data, len as usize).to_vec(),
        };
        g_object_unref(stream.cast());
        Ok(crlf(source))
    }
}

/// The bytes with every line ending CRLF, as RFC 5322 §2.1 defines a line.
///
/// A CR is inserted before an LF that does not already have one, and nothing
/// else is changed: a CRLF stays one rather than becoming CR CRLF, and a lone
/// CR — which is not a line ending — is left where it is. That is the rule
/// `CamelMimeFilterCrlf` applies when encoding, minus the dot-stuffing, which
/// belongs to the SMTP conversation and not to a message.
fn crlf(source: Vec<u8>) -> Vec<u8> {
    if !source.contains(&b'\n') {
        return source;
    }
    let mut converted = Vec::with_capacity(source.len() + 16);
    let mut previous = 0u8;
    for byte in source {
        if byte == b'\n' && previous != b'\r' {
            converted.push(b'\r');
        }
        converted.push(byte);
        previous = byte;
    }
    converted
}

/// A `CamelMimeMessage` Camel's own emitter would not write out, and whatever
/// it said about that.
///
/// Owns the `GError` when there is one — a caller that drops the failure
/// instead of reporting it frees it rather than leaking a message it never
/// showed anyone.
pub struct Unwritable(*mut GError);

impl Unwritable {
    /// Hands the failure over as an owned `GError`, in `domain` and `code` if
    /// Camel did not report one of its own.
    ///
    /// Ownership passes to the caller, who must `g_error_free` it or hand it to
    /// a C caller that will — which is what [`set_raw_gerror`] does for a
    /// vfunc's out-parameter.
    ///
    /// [`set_raw_gerror`]: jmap_backend_core::error::set_raw_gerror
    pub fn into_gerror(self, domain: GQuark, code: c_int) -> *mut GError {
        // The error is handed on rather than freed, so this value must not run
        // its `Drop`.
        let failure = ManuallyDrop::new(self);
        if !failure.0.is_null() {
            return failure.0;
        }
        // SAFETY: a quark and a code are plain values, and the message is
        // 'static, NUL-terminated and copied by the constructor.
        unsafe { g_error_new_literal(domain, code, UNEXPLAINED.as_ptr()) }
    }

    /// The failure Camel described.
    ///
    /// # Safety
    ///
    /// `error` must be an owned `GError` this value may consume.
    #[cfg(test)]
    unsafe fn explained(error: *mut GError) -> Self {
        Self(error)
    }

    /// The failure Camel reported without describing.
    #[cfg(test)]
    fn unexplained() -> Self {
        Self(ptr::null_mut())
    }
}

impl Drop for Unwritable {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: the error this value owns, freed once — `into_gerror` takes
        // `self` by value and forgets it, so a failure that was reported never
        // reaches here.
        unsafe { g_error_free(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::CStr;

    use eds_sys::{
        CAMEL_FOLDER_ERROR_INVALID, CAMEL_SERVICE_ERROR_INVALID, camel_folder_error_quark,
        camel_service_error_quark,
    };
    use glib_sys::{g_error_free, g_error_new_literal};

    /// What a `GError` says, as the three things a caller branches on.
    ///
    /// # Safety
    ///
    /// `error` must be an owned `GError` this call may consume.
    unsafe fn taken(error: *mut GError) -> (GQuark, c_int, String) {
        // SAFETY: the contract above; the message is the error's own and is
        // copied out before the error is freed.
        unsafe {
            let said = (
                (*error).domain,
                (*error).code,
                CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned(),
            );
            g_error_free(error);
            said
        }
    }

    /// The rule, on the four shapes a byte before an LF can have.
    ///
    /// Beside the implementation rather than through Camel's emitter, because
    /// what a message Camel wrote contains is Camel's decision — a fixture that
    /// happened to hold a lone CR today would stop holding one the moment the
    /// emitter changed, and the rule would silently stop being tested.
    #[test]
    fn a_line_ending_is_crlf_and_only_the_missing_cr_is_added() {
        assert_eq!(crlf(b"a\nb".to_vec()), b"a\r\nb".to_vec());
        assert_eq!(crlf(b"a\r\nb".to_vec()), b"a\r\nb".to_vec());
        // A lone CR is not a line ending and is not this function's business.
        assert_eq!(crlf(b"a\rb".to_vec()), b"a\rb".to_vec());
        // Including the one at the very end, which has no LF to be part of.
        assert_eq!(crlf(b"a\r".to_vec()), b"a\r".to_vec());
    }

    /// An LF with nothing before it is still an LF that needs a CR.
    ///
    /// The off-by-one the loop is written around: a message whose first byte is
    /// an LF is an empty header block, which is a message Camel can produce and
    /// a server can be handed.
    #[test]
    fn an_lf_at_the_start_of_the_message_gets_its_cr_too() {
        assert_eq!(crlf(b"\nbody".to_vec()), b"\r\nbody".to_vec());
        assert_eq!(crlf(b"\n".to_vec()), b"\r\n".to_vec());
    }

    /// Bytes with no LF in them come back as they were, byte for byte.
    ///
    /// Both because it is the cheap path and because it says what the function
    /// does *not* do: nothing is normalised, escaped or re-encoded here.
    #[test]
    fn bytes_with_no_line_ending_in_them_are_left_alone() {
        assert_eq!(crlf(Vec::new()), Vec::<u8>::new());
        assert_eq!(crlf(b"\x00\xff\x7f".to_vec()), b"\x00\xff\x7f".to_vec());
    }

    /// A failure Camel described is reported as Camel described it, whatever
    /// the caller would have said.
    ///
    /// The writer knows why it would not write the message and this layer does
    /// not, so replacing its sentence — or its domain — would lose the only
    /// account of the failure anyone has, for one composed out of the caller's
    /// context.
    #[test]
    fn a_failure_the_writer_explained_is_the_error_the_caller_gets() {
        // SAFETY: a quark that registers itself and a NUL-terminated message
        // the constructor copies.
        let explained = unsafe {
            g_error_new_literal(
                gio_sys::g_io_error_quark(),
                gio_sys::G_IO_ERROR_NO_SPACE,
                c"the disk is full".as_ptr(),
            )
        };

        // SAFETY: an owned GError, handed over.
        let failure = unsafe { Unwritable::explained(explained) };
        // SAFETY: no arguments, and the quark registers itself.
        let quark = unsafe { camel_folder_error_quark() };
        let reported = failure.into_gerror(quark, CAMEL_FOLDER_ERROR_INVALID as c_int);

        // SAFETY: the error `into_gerror` handed over.
        let (domain, code, message) = unsafe { taken(reported) };
        // SAFETY: no arguments.
        assert_eq!(domain, unsafe { gio_sys::g_io_error_quark() });
        assert_eq!(code, gio_sys::G_IO_ERROR_NO_SPACE);
        assert_eq!(message, "the disk is full");
    }

    /// A writer that failed without saying why is reported in the domain the
    /// caller names — the folder's, for an append.
    ///
    /// Something has to be reported: Camel logs a critical of its own for a
    /// vfunc that answers FALSE with no error set, so a silent failure would be
    /// a message the user is told nothing about and a warning in the log
    /// blaming this provider for a bug it does not have.
    #[test]
    fn a_failure_the_writer_did_not_explain_is_reported_in_the_folders_domain() {
        // SAFETY: no arguments, and the quark registers itself.
        let quark = unsafe { camel_folder_error_quark() };

        let reported =
            Unwritable::unexplained().into_gerror(quark, CAMEL_FOLDER_ERROR_INVALID as c_int);

        // SAFETY: the error `into_gerror` handed over.
        let (domain, code, message) = unsafe { taken(reported) };
        assert_eq!(domain, quark);
        assert_eq!(code, CAMEL_FOLDER_ERROR_INVALID as c_int);
        assert!(!message.is_empty(), "a failure with nothing to say");
    }

    /// And in the service's domain for a transport, which is why the domain is
    /// an argument at all.
    ///
    /// The same unexplained failure, asked for by the other caller: a
    /// `CamelTransport`'s `send_to_sync` has no folder in the call, and a
    /// `CAMEL_FOLDER_ERROR` from a service Camel never asked for a folder is an
    /// error in a domain its caller does not test.
    #[test]
    fn the_same_failure_is_reported_in_the_transports_domain_for_a_transport() {
        // SAFETY: no arguments, and the quark registers itself.
        let quark = unsafe { camel_service_error_quark() };

        let reported =
            Unwritable::unexplained().into_gerror(quark, CAMEL_SERVICE_ERROR_INVALID as c_int);

        // SAFETY: the error `into_gerror` handed over.
        let (domain, code, _) = unsafe { taken(reported) };
        assert_eq!(domain, quark);
        assert_eq!(code, CAMEL_SERVICE_ERROR_INVALID as c_int);
    }

    /// Both callers get the same sentence for it, because it is the same thing
    /// that went wrong: only the domain differs.
    #[test]
    fn the_sentence_does_not_depend_on_who_is_reporting_it() {
        // SAFETY: no arguments, and the quarks register themselves.
        let (folder, service) =
            unsafe { (camel_folder_error_quark(), camel_service_error_quark()) };

        let appending = Unwritable::unexplained().into_gerror(folder, 0);
        let sending = Unwritable::unexplained().into_gerror(service, 0);

        // SAFETY: the two errors `into_gerror` handed over.
        let (appending, sending) = unsafe { (taken(appending), taken(sending)) };
        assert_eq!(appending.2, sending.2);
    }
}
