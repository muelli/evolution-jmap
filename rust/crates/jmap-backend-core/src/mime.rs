// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `CamelMimeMessage` as the octets that a JMAP request uploads.
//!
//! An account puts everything on the server as an object, and sends it up as
//! a blob. This module is the step between. Three different kinds of caller
//! need it:
//!
//! - The append of the Camel provider (a folder that takes in a message from
//!   outside the account).
//! - The `send_to_sync` of its transport (a service that sends a message out,
//!   with no folder in the call).
//! - The scheduled send of jmap-ui (a composer that gives the server a
//!   message to hold).
//!
//! One function serves all three. A second emitter is a second place where
//! the message that the user filed and the message that they sent can
//! differ. For the same reason, this function is here and not in
//! `jmap-mail`. The UI module cannot link the rlib of the provider (its
//! `camel_provider_module_init` would go into a shell module with it), and a
//! copy would be that second emitter.
//!
//! ## Camel's writer, for the opposite of the reason `jmap-mail::message` gives
//!
//! The bytes come from `camel_data_wrapper_write_to_output_stream_sync`. That
//! is the RFC 5322 emitter of Camel, through the `CamelDataWrapper` interface
//! of the message. This decision mirrors the decision that the `message`
//! module of the provider makes about the parse. The object must agree with
//! all other parts of Camel about the content of the message. A provider that
//! wrote headers itself would be a second MIME implementation in the same
//! process that does not agree. That disagreement is *stored*, because the
//! uploaded bytes are what the account holds from then on.
//!
//! The module uses a `GMemoryOutputStream`, not a `CamelStreamMem`. The
//! destination is a buffer in both cases, and the Camel stream class is a
//! wrapper around the GIO object. During the upload, the full message is in
//! memory two times (the copy of the stream and the copy of the request).
//! The `message` module of the provider pays the same cost in the other
//! direction.
//!
//! ## The one change to what Camel wrote: the line endings
//!
//! The Camel emitter writes the message in the *internal* form of Camel,
//! where lines end with a bare LF. In Camel, the conversion to CRLF is done
//! by a `CamelMimeFilterCrlf` that its transports put between the message and
//! the socket. This provider is also a transport, and also an importer. Both
//! of its callers put the bytes in a location that exists after the call
//! ends. Thus the conversion occurs here, or it does not occur.
//!
//! No conversion is not an option. RFC 5322 §2.1 defines a line as
//! CRLF-terminated, and RFC 8621 §4.8 imports "an RFC 5322 message". RFC 5321
//! §2.3.8 forbids a bare LF in the data that an SMTP server gets, and an
//! `EmailSubmission` eventually gives the data to an SMTP server. For a
//! message stored with bare LFs, the DKIM signature covers different bytes
//! than the recipient verifies, and a strict relay can truncate the body.
//!
//! Thus `crlf` applies the same rule as the Camel filter in
//! `CAMEL_MIME_FILTER_CRLF_ENCODE`. It inserts a CR before an LF that has no
//! CR, and changes nothing else. It does not change a lone CR. Intentionally,
//! it does not add the leading dots that `CRLF_MODE_CRLF_DOTS` adds: they
//! are an SMTP wire-level escape, and they would go *into* an imported
//! message. The rule is written here, not used through `CamelStream`. The
//! filter is a `CamelStream`-era API that this crate does not otherwise use,
//! and the rule is four lines whose edge cases tests can examine directly.
//!
//! The rule has the same exposure as in Camel: in a part whose content is raw
//! 8-bit or binary (not transfer-encoded), it also changes the LFs. The same
//! occurs when the part goes out through `camel-smtp-transport`.
//!
//! ## The failure has no domain of its own
//!
//! When the writer refuses to write a message, this provider cannot explain
//! the failure better than the writer. Thus the module passes the `GError`
//! of Camel through without a change. But Camel can also report a failure
//! *without* a `GError`, and the module cannot pass that through. It must
//! report something: a vfunc that answers FALSE with no error set causes a
//! GLib critical. The domain of that error belongs to the caller, not to
//! this module. An append is a `CAMEL_FOLDER_ERROR`, because Camel asked a
//! folder. A send is a `CAMEL_SERVICE_ERROR`, because a transport has no
//! folder to name. Thus [`Unwritable`] is the failure without a domain, and
//! the caller supplies the domain in [`Unwritable::into_gerror`].

use std::ffi::c_int;
use std::mem::ManuallyDrop;
use std::ptr;

use crate::owned::Owned;
use eds_sys::{CamelDataWrapper, CamelMimeMessage, camel_data_wrapper_write_to_output_stream_sync};
use gio_sys::{
    GMemoryOutputStream, g_memory_output_stream_get_data, g_memory_output_stream_get_data_size,
    g_memory_output_stream_new_resizable, g_output_stream_flush,
};
use glib_sys::{GError, GFALSE, GQuark, g_error_free, g_error_new_literal};

/// The message for a failure where Camel refused to write a message and did
/// not tell why.
///
/// Both callers use this one sentence, because it is the same failure. Only
/// the domain is different, and that domain is the argument of
/// [`Unwritable::into_gerror`].
const UNEXPLAINED: &std::ffi::CStr = c"the message could not be written out";

/// The message as the octets that an `Email/import` upload takes.
///
/// The function flushes the stream before it reads the buffer.
/// `GMemoryOutputStream` does not buffer, but the writer above it can wrap
/// it. Without the flush, the message loses the bytes that a filter still
/// holds, and the server gets a corrupted message with no error.
///
/// The function copies the buffer and does not steal it. Thus the free
/// function of the stream continues to release it. The upload needs an owned
/// `Vec` in both cases.
///
/// # Safety
///
/// `message` must be NULL or point at a live `CamelMimeMessage`.
pub unsafe fn write_message(message: *mut CamelMimeMessage) -> Result<Vec<u8>, Unwritable> {
    // SAFETY: a new resizable memory stream, the message by the contract of
    // this function, and an error out-parameter that is a local with the
    // initial value NULL. `stream` releases its reference when this scope
    // ends, on every path below.
    unsafe {
        let Some(stream) = Owned::from_raw(g_memory_output_stream_new_resizable()) else {
            return Err(Unwritable(ptr::null_mut()));
        };
        let mut failure: *mut GError = ptr::null_mut();
        let written = camel_data_wrapper_write_to_output_stream_sync(
            message.cast::<CamelDataWrapper>(),
            stream.as_ptr(),
            ptr::null_mut(),
            &mut failure,
        );
        let flushed = written >= 0
            && g_output_stream_flush(stream.as_ptr(), ptr::null_mut(), &mut failure) != GFALSE;

        if !flushed {
            return Err(Unwritable(failure));
        }

        let memory_stream = stream.as_ptr().cast::<GMemoryOutputStream>();
        let data = g_memory_output_stream_get_data(memory_stream).cast::<u8>();
        let len = g_memory_output_stream_get_data_size(memory_stream);
        let source = match data.is_null() {
            true => Vec::new(),
            false => std::slice::from_raw_parts(data, len as usize).to_vec(),
        };
        Ok(crlf(source))
    }
}

/// The bytes with every line ending CRLF, as RFC 5322 §2.1 defines a line.
///
/// The function inserts a CR before an LF that has no CR, and changes
/// nothing else. A CRLF stays a CRLF (not CR CRLF). A lone CR is not a line
/// ending, and stays where it is. This is the encode rule of
/// `CamelMimeFilterCrlf`, without the dot-stuffing. Dot-stuffing belongs to
/// the SMTP conversation, not to a message.
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

/// A `CamelMimeMessage` that the Camel emitter did not write, and the
/// explanation from the emitter, if any.
///
/// Owns the `GError` if there is one. Thus a caller that drops the failure
/// and does not report it frees the error. It does not leak a message that
/// nobody saw.
pub struct Unwritable(*mut GError);

impl Unwritable {
    /// Gives the failure as an owned `GError`. If Camel did not report an
    /// error, the error has `domain` and `code`.
    ///
    /// Ownership goes to the caller. The caller must `g_error_free` the error
    /// or give it to a C caller that frees it. [`set_raw_gerror`] does this
    /// for the out-parameter of a vfunc.
    ///
    /// [`set_raw_gerror`]: crate::error::set_raw_gerror
    pub fn into_gerror(self, domain: GQuark, code: c_int) -> *mut GError {
        // The error goes to the caller and is not freed, so this value must
        // not run its `Drop`.
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
        // SAFETY: the error that this value owns, freed one time.
        // `into_gerror` takes `self` by value and forgets it. Thus a reported
        // failure never gets here.
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
        // SAFETY: the contract above. The message belongs to the error, and
        // the code copies it before it frees the error.
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
    /// This test calls the implementation directly, not through the Camel
    /// emitter. Camel decides the content of a message that Camel writes. A
    /// fixture that has a lone CR today can lose it when the emitter changes.
    /// Then the test would stop testing the rule, without a warning.
    #[test]
    fn a_line_ending_is_crlf_and_only_the_missing_cr_is_added() {
        assert_eq!(crlf(b"a\nb".to_vec()), b"a\r\nb".to_vec());
        assert_eq!(crlf(b"a\r\nb".to_vec()), b"a\r\nb".to_vec());
        // A lone CR is not a line ending, and this function ignores it.
        assert_eq!(crlf(b"a\rb".to_vec()), b"a\rb".to_vec());
        // This includes a CR at the end, which has no LF after it.
        assert_eq!(crlf(b"a\r".to_vec()), b"a\r".to_vec());
    }

    /// An LF with nothing before it is still an LF that needs a CR.
    ///
    /// The loop prevents this off-by-one error. A message whose first byte is
    /// an LF has an empty header block. Camel can produce such a message, and
    /// a server can receive it.
    #[test]
    fn an_lf_at_the_start_of_the_message_gets_its_cr_too() {
        assert_eq!(crlf(b"\nbody".to_vec()), b"\r\nbody".to_vec());
        assert_eq!(crlf(b"\n".to_vec()), b"\r\n".to_vec());
    }

    /// Bytes with no LF in them come back as they were, byte for byte.
    ///
    /// This is the cheap path. It also shows what the function does *not* do:
    /// it does not normalise, escape or re-encode.
    #[test]
    fn bytes_with_no_line_ending_in_them_are_left_alone() {
        assert_eq!(crlf(Vec::new()), Vec::<u8>::new());
        assert_eq!(crlf(b"\x00\xff\x7f".to_vec()), b"\x00\xff\x7f".to_vec());
    }

    /// A failure that Camel described keeps the description from Camel,
    /// whatever the caller would have said.
    ///
    /// The writer knows why it did not write the message, and this layer does
    /// not. To replace its sentence (or its domain) with text from the context
    /// of the caller would lose the only real description of the failure.
    #[test]
    fn a_failure_the_writer_explained_is_the_error_the_caller_gets() {
        // SAFETY: a quark that registers itself, and a NUL-terminated message
        // that the constructor copies.
        let explained = unsafe {
            g_error_new_literal(
                gio_sys::g_io_error_quark(),
                gio_sys::G_IO_ERROR_NO_SPACE,
                c"the disk is full".as_ptr(),
            )
        };

        // SAFETY: an owned GError, given to the value.
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

    /// A writer that failed without a reason gets an error in the domain that
    /// the caller names (the folder domain, for an append).
    ///
    /// The code must report something. Camel logs a critical for a vfunc that
    /// answers FALSE with no error set. A silent failure would tell the user
    /// nothing about the message, and the log warning would blame this
    /// provider for a bug that it does not have.
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

    /// For a transport, the error is in the service domain. This is why the
    /// domain is an argument.
    ///
    /// This is the same unexplained failure, from the other caller. The
    /// `send_to_sync` of a `CamelTransport` has no folder in the call. A
    /// `CAMEL_FOLDER_ERROR` from a service that Camel did not ask for a folder
    /// is in a domain that its caller does not test.
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

    /// Both callers get the same sentence, because it is the same failure.
    /// Only the domain is different.
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
