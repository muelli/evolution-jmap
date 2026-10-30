// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fetches an http(s) contact photo the user asked for, by clicking the
//! placeholder's button.
//!
//! The same rules as item 148 (c)'s `blobId` photo download: no cookies, no
//! credentials, a size cap, and a check that the server calls the body an
//! image before this project believes it. [`UreqTransport`] already gives
//! the first two for free (it carries no cookie jar, and this module never
//! sets an `Authorization` header), and already streams the response under
//! a byte limit instead of trusting `Content-Length`. The free function
//! shape, with no `Client`/session, follows `jmap_client::oauth::discover`:
//! the nearest existing call that fetches a URL this project has not
//! authenticated to.

use std::time::Duration;

use jmap_client::transport::{HttpMethod, HttpRequest, Transport, UreqTransport};

/// Same cap as item 148 (c)'s inline photo download
/// (`jmap-book-sync::MAX_INLINE_PHOTO_BYTES`): large enough for an ordinary
/// avatar, small enough that a server cannot use this button to make the
/// process allocate without bound.
const MAX_EXTERNAL_PHOTO_BYTES: u64 = 512 * 1024;

/// How long to wait for the whole exchange. A contact preview is not worth
/// hanging a worker thread over.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A fetched photo, ready to become a `data:` URI.
#[derive(Debug)]
pub struct Photo {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Fetches `url`, with no cookies and no credentials, and checks that the
/// server calls the body an image.
///
/// This project shows no detail from the error text to the user, so it need
/// not be translated.
///
/// # Errors
///
/// Returns a description of the failure: a non-2xx status, no declared
/// content type, or a declared content type that is not `image/...`.
pub fn fetch_external_photo(url: &str) -> Result<Photo, String> {
    let response = UreqTransport::new(TIMEOUT)
        .execute(HttpRequest {
            method: HttpMethod::Get,
            url,
            headers: &[],
            body: None,
            cancel: None,
            max_response_bytes: MAX_EXTERNAL_PHOTO_BYTES,
        })
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!("the server answered {}", response.status));
    }
    let media_type = response
        .content_type
        .as_deref()
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .ok_or_else(|| "the server named no content type".to_string())?;
    if !media_type.starts_with("image/") {
        return Err(format!(
            "the server called the body {media_type:?}, not an image"
        ));
    }
    Ok(Photo {
        media_type,
        bytes: response.body,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// Runs a one-shot HTTP server on loopback that answers `response` to
    /// its one connection, and returns the URL to fetch it at.
    ///
    /// `response` is the raw bytes after the request line: status line,
    /// headers, blank line, body, exactly as this test wants to send them.
    fn serve_once(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let port = listener.local_addr().expect("local address").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept the one connection");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            stream.write_all(response).expect("write the response");
        });
        format!("http://127.0.0.1:{port}/photo")
    }

    #[test]
    fn an_image_response_is_accepted() {
        let url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\n\r\nABCD",
        );
        let photo = fetch_external_photo(&url).expect("an image/png body is a photo");
        assert_eq!(photo.media_type, "image/png");
        assert_eq!(photo.bytes, b"ABCD");
    }

    #[test]
    fn a_content_type_with_parameters_is_still_recognised() {
        let url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png; charset=binary\r\nContent-Length: 2\r\n\r\nAB",
        );
        let photo = fetch_external_photo(&url).expect("a parameterised image type is a photo");
        assert_eq!(photo.media_type, "image/png");
    }

    #[test]
    fn a_non_image_content_type_is_rejected() {
        let url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 4\r\n\r\n<p>?",
        );
        let error = fetch_external_photo(&url).expect_err("text/html is not a photo");
        assert!(error.contains("text/html"), "unexpected error: {error}");
    }

    #[test]
    fn no_content_type_is_rejected() {
        let url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nABCD");
        let error = fetch_external_photo(&url).expect_err("no declared type is not a photo");
        assert!(
            error.contains("no content type"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_404_is_rejected() {
        let url = serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        let error = fetch_external_photo(&url).expect_err("a 404 is not a photo");
        assert!(error.contains("404"), "unexpected error: {error}");
    }

    #[test]
    fn an_oversized_body_is_rejected() {
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 600000\r\n\r\n"
                .to_vec();
        response.extend(std::iter::repeat_n(b'A', 600_000));
        let url = serve_once(Box::leak(response.into_boxed_slice()));
        let error = fetch_external_photo(&url).expect_err("an oversized body is not a photo");
        assert!(
            error.to_ascii_lowercase().contains("large")
                || error.to_ascii_lowercase().contains("limit"),
            "unexpected error: {error}"
        );
    }
}
