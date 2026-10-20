// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! [`jmap_backend_core::mime`]: the `CamelMimeMessage` as the octets a JMAP request
//! uploads.
//!
//! Each message that this account puts on the server goes through here. Two
//! different callers use it. The append of `jmap-mail` is a folder that takes
//! a message from outside the account. The `send_to_sync` of a
//! `CamelTransport` is a service with no folder that sends a message. Both get
//! an object and both need blob bytes, so one function does the writing. A
//! second function could make the filed message differ from the sent message.
//!
//! ## Camel's own writer, and why that is the whole point
//!
//! `camel_data_wrapper_write_to_output_stream_sync`, the RFC 5322 emitter of
//! Camel, makes the bytes through the `CamelDataWrapper` face of the message.
//! The message module of `jmap-mail` makes the same decision for the parse of
//! incoming data. A provider that wrote headers itself would be a second MIME
//! implementation in the process, and it could disagree with the first about
//! the message. Here the disagreement would be *stored*, because the uploaded
//! bytes are what the account holds after the upload.
//!
//! Thus these tests do not check the emitter, which belongs to Camel. They
//! check the code around it. The returned buffer must contain all that was
//! written ([`a_message_larger_than_one_buffer_is_written_out_whole`]: a
//! truncation is a corrupted message on the server, and only the length shows
//! it). The write must not consume the object that the caller still holds.
//! The failure that the caller reports must be the failure that occurred.
//!
//! ## The error is the caller's to name
//!
//! This is the one part taken from the append of `jmap-mail` with a change.
//! A writer that failed without a reason gave a `CAMEL_FOLDER_ERROR`. This is
//! correct for a folder but wrong for a transport, which has no folder in the
//! call. Thus [`Unwritable::into_gerror`] takes the domain and code from the
//! caller. Its two cases (Camel explained the failure, Camel did not) have
//! unit tests beside the implementation, where a failure can be constructed.
//! This file cannot construct a real `CamelMimeMessage` that its own writer
//! refuses.
//!
//! [`Unwritable::into_gerror`]: jmap_backend_core::mime::Unwritable::into_gerror

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CamelDataWrapper, CamelMimeMessage, camel_data_wrapper_construct_from_data_sync,
    camel_mime_message_get_subject,
};
use glib_sys::{GError, GFALSE, gssize};
use gobject_sys::g_object_unref;
use jmap_backend_core::mime::write_message;

/// The RFC 5322 bytes of an ordinary message: CRLF line endings, a header
/// block, a blank line, a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// A `CamelMimeMessage` parsed from bytes. Both callers get a message in this
/// way: from `get_message_sync` on a folder of another account for an append,
/// and as the object of the composer for a send.
struct Message(*mut CamelMimeMessage);

impl Message {
    fn parsed(source: &[u8]) -> Self {
        // SAFETY: a fresh message is a valid `CamelDataWrapper`, `source` is a
        // live buffer of the length given, and the error out-parameter is a
        // local that starts NULL.
        unsafe {
            let message = eds_sys::camel_mime_message_new();
            let mut error: *mut GError = ptr::null_mut();
            let parsed = camel_data_wrapper_construct_from_data_sync(
                message.cast::<CamelDataWrapper>(),
                source.as_ptr().cast(),
                source.len() as gssize,
                ptr::null_mut(),
                &mut error,
            );
            assert_ne!(parsed, GFALSE, "the fixture message would not parse");
            Self(message)
        }
    }

    /// The bytes that the account uploads for the message.
    fn written(&self) -> Vec<u8> {
        // SAFETY: a live message this value owns.
        match unsafe { write_message(self.0) } {
            Ok(source) => source,
            Err(_) => panic!("Camel would not write the message out"),
        }
    }

    /// The current subject header, as Camel reads it from the object. This
    /// lets a test compare a message parsed again with the original message.
    fn subject(&self) -> String {
        // SAFETY: a live message. The accessor returns a string that the
        // message owns and that outlives this call.
        unsafe {
            let subject = camel_mime_message_get_subject(self.0);
            assert!(!subject.is_null(), "the message has no subject");
            CStr::from_ptr(subject).to_string_lossy().into_owned()
        }
    }
}

impl Drop for Message {
    fn drop(&mut self) {
        // SAFETY: the one reference, taken at construction.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The bytes are a message (headers, a blank line, the body), and the parser
/// of Camel makes the same message from them again.
///
/// The test uses a round trip, not a byte comparison with the input. Camel
/// can emit a parsed message with canonicalised or reordered headers. A check
/// of the exact bytes would test the current format of the emitter, not this
/// function. The content must stay the same.
#[test]
fn the_bytes_a_message_is_written_out_as_parse_back_to_the_same_message() {
    let message = Message::parsed(MESSAGE);

    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote text");
    assert!(text.contains("Subject: Lunch?"), "{text}");
    assert!(text.contains("From: Bob <bob@example.com>"), "{text}");
    assert!(text.contains("One o'clock at the usual place."), "{text}");

    let again = Message::parsed(&written);
    assert_eq!(again.subject(), message.subject());
}

/// A message larger than one buffer returns complete.
///
/// This tests the flush before the read in the implementation. If the code
/// read the buffer while a filter still held part of the message, the result
/// would be a truncation: a valid, shorter message, uploaded and stored, with
/// no sign of a missing paragraph. The test checks the body at both ends and
/// by length, because a message without its middle still starts and stops
/// correctly.
#[test]
fn a_message_larger_than_one_buffer_is_written_out_whole() {
    let body = "0123456789abcdef".repeat(16 * 1024);
    let source = format!(
        "From: Bob <bob@example.com>\r\n\
Subject: A long one\r\n\
\r\n\
{body}"
    );
    let message = Message::parsed(source.as_bytes());

    let written = message.written();

    let text = String::from_utf8(written).expect("the emitter wrote text");
    let (_, emitted) = text.split_once("\r\n\r\n").expect("a header/body boundary");
    assert_eq!(
        emitted.trim_end_matches("\r\n").len(),
        body.len(),
        "the body came back {} bytes for {} written",
        emitted.len(),
        body.len()
    );
    assert!(emitted.starts_with("0123456789abcdef"));
    assert!(emitted.trim_end_matches("\r\n").ends_with("abcdef"));
}

/// A write of a message does not change the message.
///
/// This is important for the caller that the tests above do not cover. A
/// transport gets the `CamelMimeMessage` that the composer still owns and
/// Evolution still shows. A send could empty it, or work only once because the
/// first write consumed the content stream. That bug would show only when a
/// failed submission is tried again.
#[test]
fn writing_a_message_out_does_not_consume_it() {
    let message = Message::parsed(MESSAGE);

    let first = message.written();
    let second = message.written();

    assert_eq!(first, second);
    assert_eq!(message.subject(), "Lunch?");
}

/// Each line ends with CRLF, because that is what a message *is*.
///
/// The emitter of Camel writes the message in the internal form of Camel,
/// which ends lines with a bare LF. The transports of Camel put a
/// `CamelMimeFilterCrlf` between the message and the socket. This provider is
/// one of those transports and also an importer. Both put the bytes in a
/// location that outlives the call, so the conversion must occur here or not
/// at all.
///
/// It is not cosmetic. RFC 5322 §2.1 defines a line as CRLF-terminated, RFC
/// 8621 §4.8 imports "an RFC 5322 message", and RFC 5321 §2.3.8 forbids a bare
/// LF in the data that an SMTP server gets. An `EmailSubmission` eventually
/// gives the message to an SMTP server. Thus, for a message stored with bare
/// LFs, the DKIM signature covers different bytes than the recipient
/// verifies. Also, a strict relay can truncate its body at the first bare LF.
#[test]
fn the_bytes_a_message_goes_up_as_end_their_lines_the_way_rfc_5322_says() {
    let message = Message::parsed(MESSAGE);

    let written = message.written();

    assert!(
        written.windows(2).any(|pair| pair == b"\r\n"),
        "no line endings at all"
    );
    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(
        bare,
        0,
        "{bare} bare LFs in {:?}",
        String::from_utf8_lossy(&written)
    );
}

/// A line that already ended with CRLF still ends with CRLF, not CR CRLF.
///
/// This is the other half of the same rule, which a careless conversion
/// breaks. Camel can emit parts that already contain the line endings, for
/// example a `text/plain` part with CRLF in its content. Such a message would
/// get a stray CR on each line, and the recipient would see blank lines in the
/// body.
#[test]
fn a_line_that_already_ended_crlf_does_not_gain_a_second_cr() {
    let message = Message::parsed(MESSAGE);

    let written = message.written();

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "{:?}",
        String::from_utf8_lossy(&written)
    );
    // The message is still the same message: the write added nothing to the
    // body except the line endings.
    let again = Message::parsed(&written);
    assert_eq!(again.subject(), "Lunch?");
}

/// A multipart/mixed message with text and an attachment serializes with CRLF
/// at all boundaries and headers, with no bare LFs, and roundtrips correctly.
#[test]
fn multipart_mixed_message_with_pdf_attachment_serializes_with_crlf_and_parses_back_intact() {
    let multipart_source = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Monthly Report with Attachment\r\n\
Message-ID: <multipart-report-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 10:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"====_report_boundary_123_====\"\r\n\
\r\n\
This is a multi-part message in MIME format.\r\n\
\r\n\
--====_report_boundary_123_====\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
Please find attached the financial report.\r\n\
\r\n\
--====_report_boundary_123_====\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjUKJcfsj6IKNCAwIG9iago8PAovVHlwZSAvUGFnZQovUGFyZW50IDMgMCBS\r\n\
\r\n\
--====_report_boundary_123_====--\r\n";

    let message = Message::parsed(multipart_source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote text");
    assert!(
        text.contains("Subject: Monthly Report with Attachment"),
        "{text}"
    );
    assert!(text.contains("report.pdf"), "{text}");
    assert!(text.contains("====_report_boundary_123_===="), "{text}");

    // All line endings must be CRLF.
    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in multipart output");

    // No double-CR (\r\r\n)
    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "Monthly Report with Attachment");
}

/// A multipart/alternative message with HTML and plain text bodies keeps its
/// MIME parts and roundtrips without loss of line format.
#[test]
fn multipart_alternative_message_with_html_and_plain_text_preserves_structure() {
    let alt_source = b"From: Team <team@example.com>\r\n\
To: Member <member@example.com>\r\n\
Subject: Alternative format announcement\r\n\
Message-ID: <alt-announcement-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 11:30:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"alt_boundary_456\"\r\n\
\r\n\
--alt_boundary_456\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
New feature release announcement in plain text.\r\n\
\r\n\
--alt_boundary_456\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
\r\n\
<html><body><h1>New Feature Release</h1><p>Announcement in rich HTML.</p></body></html>\r\n\
\r\n\
--alt_boundary_456--\r\n";

    let message = Message::parsed(alt_source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("Subject: Alternative format announcement"));
    assert!(text.contains("<h1>New Feature Release</h1>"));
    assert!(text.contains("New feature release announcement in plain text."));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(
        bare, 0,
        "found {bare} bare LFs in multipart/alternative output"
    );

    let parsed_again = Message::parsed(&written);
    assert_eq!(parsed_again.subject(), "Alternative format announcement");
}

/// A message with a Quoted-Printable body and custom X-headers serializes with
/// canonical CRLF line endings and roundtrips without bare LFs.
#[test]
fn message_with_quoted_printable_body_and_custom_headers_serializes_with_crlf() {
    let qp_source = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Quoted Printable Encoding Test\r\n\
Message-ID: <qp-test-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 16:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
X-Custom-Delivery: priority-high\r\n\
X-Originating-IP: [192.0.2.1]\r\n\
\r\n\
This is a test with soft line breaks=\r\n\
 and accented characters like caf=C3=A9 and na=C3=AFve.\r\n";

    let message = Message::parsed(qp_source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("Subject: Quoted Printable Encoding Test"));
    assert!(text.contains("X-Custom-Delivery: priority-high"));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in QP output");

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "Quoted Printable Encoding Test");
}

/// A nested multipart message with a multipart/alternative part and an
/// application/octet-stream attachment serializes with clean CRLF boundaries
/// and keeps the full boundary tree.
#[test]
fn message_with_nested_multipart_and_mixed_encodings_preserves_tree_structure() {
    let nested_source = b"From: Reports <reports@example.com>\r\n\
To: Team <team@example.com>\r\n\
Subject: Comprehensive Weekly Audit\r\n\
Message-ID: <audit-weekly-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 17:30:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"_outer_boundary_888_\"\r\n\
\r\n\
--_outer_boundary_888_\r\n\
Content-Type: multipart/alternative; boundary=\"_inner_alt_888_\"\r\n\
\r\n\
--_inner_alt_888_\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
Plain audit summary line.\r\n\
\r\n\
--_inner_alt_888_\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
<strong>Rich audit summary</strong>\r\n\
\r\n\
--_inner_alt_888_--\r\n\
\r\n\
--_outer_boundary_888_\r\n\
Content-Type: application/octet-stream; name=\"data.bin\"\r\n\
Content-Disposition: attachment; filename=\"data.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=\r\n\
\r\n\
--_outer_boundary_888_--\r\n";

    let message = Message::parsed(nested_source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("Subject: Comprehensive Weekly Audit"));
    assert!(text.contains("_outer_boundary_888_"));
    assert!(text.contains("_inner_alt_888_"));
    assert!(text.contains("data.bin"));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in nested multipart output");

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "Comprehensive Weekly Audit");
}

/// A message with UTF-8 encoded-word headers and plain/HTML bodies serializes
/// with canonical CRLF and roundtrips without header corruption.
#[test]
fn message_with_encoded_word_subject_and_html_alternative_serializes_cleanly() {
    let source = b"From: Sender <sender@example.com>\r\n\
To: Recipient <recipient@example.com>\r\n\
Subject: =?UTF-8?B?Sk1BUCDDnGJlcnByw7xmdW5nIPCfmYA=?=\r\n\
Message-ID: <encoded-subj-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 18:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"_part_alt_boundary_999_\"\r\n\
\r\n\
--_part_alt_boundary_999_\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
Text preview line with UTF-8: Gr\xc3\xbc\xc3\x9fe aus Berlin!\r\n\
\r\n\
--_part_alt_boundary_999_\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
<html><body><p>HTML body with UTF-8: Gr&uuml;&szlig;e aus Berlin!</p></body></html>\r\n\
\r\n\
--_part_alt_boundary_999_--\r\n";

    let message = Message::parsed(source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("_part_alt_boundary_999_"));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in encoded word message");

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "JMAP Überprüfung 🙀");
}

/// A multipart/related message with an HTML body and an inline image attachment
/// (referenced through Content-ID) serializes with clean CRLF endings and
/// roundtrips intact.
#[test]
fn multipart_related_message_with_html_and_inline_attachment_serializes_cleanly() {
    let related_source = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Rich Newsletter with Inline Image\r\n\
Message-ID: <newsletter-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 19:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"_rel_bound_777_\"; type=\"text/html\"\r\n\
\r\n\
--_rel_bound_777_\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 7bit\r\n\
\r\n\
<html><body><h1>Weekly Update</h1><p><img src=\"cid:logo-img@example.com\" alt=\"Logo\"/></p></body></html>\r\n\
\r\n\
--_rel_bound_777_\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo-img@example.com>\r\n\
Content-Disposition: inline; filename=\"logo.png\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==\r\n\
\r\n\
--_rel_bound_777_--\r\n";

    let message = Message::parsed(related_source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("Subject: Rich Newsletter with Inline Image"));
    assert!(text.contains("_rel_bound_777_"));
    assert!(text.contains("cid:logo-img@example.com"));
    assert!(text.contains("image/png"));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in multipart/related output");

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "Rich Newsletter with Inline Image");
}

/// A message with 8bit Content-Transfer-Encoding serializes with canonical
/// CRLF endings and parses back without line corruption.
#[test]
fn message_with_8bit_body_and_iso_charset_serializes_with_crlf_and_parses_back_intact() {
    let source = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: 8-Bit Body and Charset Test\r\n\
Message-ID: <8bit-charset-2026@example.com>\r\n\
Date: Fri, 16 Jan 2026 20:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
Direct 8bit utf-8 body: \xc3\x84pfel, \xc3\x96l, \xc3\x9cbung und Ma\xc3\x9fe.\r\n\
Second paragraph with international accents: caf\xc3\xa9, na\xc3\xafve, fa\xc3\xa7ade.\r\n";

    let message = Message::parsed(source);
    let written = message.written();

    let text = String::from_utf8(written.clone()).expect("the emitter wrote valid utf-8");
    assert!(text.contains("Subject: 8-Bit Body and Charset Test"));
    assert!(text.contains("Direct 8bit utf-8 body"));
    assert!(text.contains("Äpfel, Öl, Übung und Maße."));

    let bare = written
        .iter()
        .enumerate()
        .filter(|(at, byte)| **byte == b'\n' && (*at == 0 || written[at - 1] != b'\r'))
        .count();
    assert_eq!(bare, 0, "found {bare} bare LFs in 8bit message output");

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "found spurious double CR in output"
    );

    let roundtripped = Message::parsed(&written);
    assert_eq!(roundtripped.subject(), "8-Bit Body and Charset Test");
}
