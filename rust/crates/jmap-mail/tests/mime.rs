// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! [`jmap_mail::mime`]: the `CamelMimeMessage` as the octets a JMAP request
//! uploads.
//!
//! Every message this account puts on the server goes through here. Two callers
//! want it and they are not the same object: [`jmap_mail::append`] is a folder
//! taking a message from outside the account, and a `CamelTransport`'s
//! `send_to_sync` is a service with no folder at all sending one. Both are
//! handed an object and both need blob bytes, so the writing is one function
//! rather than two — the second of which would be the place a difference could
//! appear between the message the user filed and the message they sent.
//!
//! ## Camel's own writer, and why that is the whole point
//!
//! The bytes are produced by `camel_data_wrapper_write_to_output_stream_sync`,
//! Camel's RFC 5322 emitter, reached through the message's `CamelDataWrapper`
//! face. That is the mirror of the decision [`jmap_mail::message`] makes about
//! the parse on the way in: a provider that wrote headers itself would be a
//! second MIME implementation inside the process, disagreeing with the first
//! about what a message says — and here the disagreement would be *stored*,
//! because what goes up is what the account holds from then on.
//!
//! So what these tests check is not the emitter, which is Camel's, but the
//! plumbing around it: that the buffer that comes back is the whole of what was
//! written ([`a_message_larger_than_one_buffer_is_written_out_whole`] — a
//! truncation here is a corrupted message on the server, and the only sign of
//! it is a length), that writing does not consume the object the caller still
//! holds, and that the failure the caller reports is the failure that happened.
//!
//! ## The error is the caller's to name
//!
//! The one thing lifted out of [`jmap_mail::append`] rather than moved: a
//! writer that fails without saying why used to produce a `CAMEL_FOLDER_ERROR`,
//! which is the right answer for a folder and the wrong one for a transport
//! that has no folder in the call. [`Unwritable::into_gerror`] therefore takes
//! the domain and code from the caller. The two cases it distinguishes —
//! Camel explained the failure, Camel did not — are unit-tested beside the
//! implementation, where a failure can be constructed; what cannot be
//! constructed here is a real `CamelMimeMessage` its own writer refuses.
//!
//! [`Unwritable::into_gerror`]: jmap_mail::mime::Unwritable::into_gerror

use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CamelDataWrapper, CamelMimeMessage, camel_data_wrapper_construct_from_data_sync,
    camel_mime_message_get_subject,
};
use glib_sys::{GError, GFALSE, gssize};
use gobject_sys::g_object_unref;
use jmap_mail::mime::write_message;

/// The RFC 5322 bytes of an ordinary message — CRLF line endings, a header
/// block, a blank line, a body.
const MESSAGE: &[u8] = b"From: Bob <bob@example.com>\r\n\
To: Alice <alice@example.com>\r\n\
Subject: Lunch?\r\n\
Message-ID: <lunch@example.com>\r\n\
Date: Thu, 15 Jan 2026 09:30:00 +0000\r\n\
\r\n\
One o'clock at the usual place.\r\n";

/// A `CamelMimeMessage` parsed out of bytes, which is how one reaches both
/// callers: `get_message_sync` on another account's folder for an append, and
/// the composer's own object for a send.
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

    /// The bytes the account would upload for it.
    fn written(&self) -> Vec<u8> {
        // SAFETY: a live message this value owns.
        match unsafe { write_message(self.0) } {
            Ok(source) => source,
            Err(_) => panic!("Camel would not write the message out"),
        }
    }

    /// What Camel makes of the subject header now — asked of the object, so
    /// that a message read back through the parser can be compared with the one
    /// that went in.
    fn subject(&self) -> String {
        // SAFETY: a live message; the accessor returns a string the message
        // owns and which outlives this call.
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

/// The bytes are a message: headers, a blank line, the body — and Camel's own
/// parser makes the same message of them again.
///
/// The round trip rather than a byte comparison with the input, because Camel
/// is entitled to re-emit a message it parsed with its headers canonicalised or
/// reordered, and pinning the exact bytes would be a test of the emitter's
/// current formatting rather than of this function. What must survive is the
/// content.
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

/// A message bigger than any one buffer comes back whole.
///
/// The flush-then-read in the implementation is what this is about, and a
/// truncation is exactly what it would look like if the buffer were read while
/// a filter still held part of the message: a valid, shorter message, uploaded
/// and stored, with nothing to say a paragraph is missing. The body is checked
/// at both ends and by length, because a message that lost its middle still
/// starts and stops correctly.
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

/// Writing a message out leaves the message alone.
///
/// It matters for the caller neither test above is: a transport is handed the
/// `CamelMimeMessage` the composer still owns and Evolution still shows, and a
/// send that emptied it — or that could only be done once, because the first
/// write consumed the content stream — would be a bug visible only after a
/// failed submission was retried.
#[test]
fn writing_a_message_out_does_not_consume_it() {
    let message = Message::parsed(MESSAGE);

    let first = message.written();
    let second = message.written();

    assert_eq!(first, second);
    assert_eq!(message.subject(), "Lunch?");
}

/// Every line ends CRLF, because that is what a message *is*.
///
/// Camel's emitter writes the message in Camel's own internal form, which ends
/// lines with a bare LF; it is Camel's transports that put a
/// `CamelMimeFilterCrlf` between the message and the socket. This provider is
/// one of those transports and also an importer, and both put the bytes
/// somewhere that outlives the call — so the conversion has to happen here or
/// not at all.
///
/// It is not cosmetic. RFC 5322 §2.1 defines a line as CRLF-terminated, RFC
/// 8621 §4.8 imports "an RFC 5322 message", and RFC 5321 §2.3.8 forbids a bare
/// LF in what an SMTP server is handed — which is what an `EmailSubmission`
/// eventually hands one. Between those, a message stored with bare LFs is one
/// whose DKIM signature is computed over different bytes than the recipient
/// verifies, and whose body a strict relay may truncate at the first bare LF.
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

/// And a line that already ended CRLF still ends CRLF, rather than CR CRLF.
///
/// The other half of the same rule, and the one a careless conversion breaks: a
/// message whose parts Camel emitted with the endings already in them — one
/// that came back out of a `text/plain` part with CRLF in its content — would
/// grow a stray CR per line, which is a body the recipient sees blank lines
/// through.
#[test]
fn a_line_that_already_ended_crlf_does_not_gain_a_second_cr() {
    let message = Message::parsed(MESSAGE);

    let written = message.written();

    assert!(
        !written.windows(3).any(|run| run == b"\r\r\n"),
        "{:?}",
        String::from_utf8_lossy(&written)
    );
    // And the message is still the message: nothing was inserted into the body
    // beyond the line endings it was written with.
    let again = Message::parsed(&written);
    assert_eq!(again.subject(), "Lunch?");
}

/// A multipart/mixed message containing text and an attachment serializes with CRLF
/// across all boundaries and headers without introducing bare LFs, and roundtrips faithfully.
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

    // All line endings must strictly be CRLF
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

/// A multipart/alternative message with HTML and plain text bodies preserves MIME parts
/// and roundtrips without losing line formatting.
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

/// A message with Quoted-Printable encoded body and custom X-headers serializes with
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

/// A complex nested multipart message containing multipart/alternative and application/octet-stream
/// attachments serializes with clean CRLF boundaries and preserves all boundary trees.
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

/// A message with UTF-8 encoded-word headers and plain/HTML bodies serializes with
/// canonical CRLF and roundtrips without header corruption.
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

/// A multipart/related message containing an HTML body and inline image attachment
/// referenced via Content-ID serializes with clean CRLF endings and roundtrips intact.
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

/// A message with 8bit Content-Transfer-Encoding and custom charset serializes with
/// canonical CRLF endings and parses back without line corruption.
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
