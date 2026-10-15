// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Substituting values into the session's URL templates.
//!
//! RFC 8620 §6.1 and §6.2 describe `uploadUrl` and `downloadUrl` as templates
//! with named variables, and require that "the client MUST URI-encode" each
//! value it puts in one. That obligation is not decoration. Every value
//! substituted here arrives from somewhere else: `accountId` and `blobId` come
//! off the wire, from the session document and from `Email/get`, and
//! [`jmap_proto::Id`] is a newtype over `String` with no grammar check, so a
//! `#` truncates the URL at a fragment, a `?` opens a query string and a `/`
//! invents a path segment. `name` is a caller's label, which for this crate is
//! a message uid. In each case the unencoded string does not address the blob
//! it was read from; it addresses a different URL, chosen by whoever wrote the
//! value.
//!
//! The escaping is the strictest of the RFC 3986 sets on purpose. A template
//! may place a variable in a path segment (`/download/{accountId}/{blobId}`)
//! or in a query value (`?accept={type}`), and this crate does not parse the
//! template to find out which; percent-encoding everything outside
//! §2.3's unreserved set is the one answer that is correct in both.

use std::borrow::Cow;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// RFC 3986 §2.3's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`),
/// expressed as what to escape rather than what to keep.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encode a value for substitution into a URL template.
///
/// Unreserved characters pass through; every other byte becomes `%XX`, in
/// uppercase hex per RFC 3986 §2.1. `percent-encoding` encodes by octet, so a
/// multi-byte UTF-8 character becomes one escape per byte, which is what
/// makes non-ASCII right.
pub(crate) fn encode_template_value(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// Replace `url`'s scheme and authority with `origin`'s, keeping its path and
/// query untouched.
///
/// Backs [`crate::ClientBuilder::rebase_urls_to_origin`]: a real deployment's
/// session document is authoritative about the *path* its endpoints live at,
/// but a client that reached the session through a different scheme/host
/// than the document names (a reverse proxy, NAT boundary, or a configured
/// public hostname the client cannot route to) needs the option to keep
/// talking to the origin that worked rather than the one the server states.
pub(crate) fn rebase_origin(url: &str, origin: &str) -> String {
    let path_and_beyond = match url.find("://") {
        Some(scheme_end) => {
            let after_scheme = scheme_end + 3;
            match url[after_scheme..].find('/') {
                Some(slash) => &url[after_scheme + slash..],
                None => "",
            }
        }
        None => url,
    };
    format!("{}{path_and_beyond}", origin.trim_end_matches('/'))
}

/// The scheme+authority prefix of `url`: everything before its first `/`
/// after `://`, or the whole string if it has none.
///
/// A host and the same host with one trailing dot are treated as the same
/// origin (e.g. `https://example.com.` and `https://example.com`), so DNS
/// root labels do not cause cross-origin credential dropping or redirect
/// mismatch errors.
pub(crate) fn origin_of(url: &str) -> Cow<'_, str> {
    match url.find("://") {
        Some(scheme_end) => {
            let after_scheme = scheme_end + 3;
            let authority_end = match url[after_scheme..].find('/') {
                Some(slash) => after_scheme + slash,
                None => url.len(),
            };
            let authority = &url[after_scheme..authority_end];
            let host_port = match authority.find('@') {
                Some(at) => &authority[at + 1..],
                None => authority,
            };
            let userinfo = match authority.find('@') {
                Some(at) => &authority[..=at],
                None => "",
            };

            let (host, port) = if host_port.starts_with('[') {
                match host_port.find(']') {
                    Some(end) => {
                        let host = &host_port[..=end];
                        let rest = &host_port[end + 1..];
                        let port = rest.strip_prefix(':');
                        (host, port)
                    }
                    None => (host_port, None),
                }
            } else {
                match host_port.rfind(':') {
                    Some(colon) => (&host_port[..colon], Some(&host_port[colon + 1..])),
                    None => (host_port, None),
                }
            };

            let normalized_host = if host.len() > 1 && host.ends_with('.') && !host.ends_with("..")
            {
                &host[..host.len() - 1]
            } else {
                host
            };

            if normalized_host == host {
                Cow::Borrowed(&url[..authority_end])
            } else if port.is_none() && userinfo.is_empty() {
                Cow::Borrowed(&url[..after_scheme + normalized_host.len()])
            } else {
                let scheme = &url[..scheme_end];
                match port {
                    Some(p) => Cow::Owned(format!("{scheme}://{userinfo}{normalized_host}:{p}")),
                    None => Cow::Owned(format!("{scheme}://{userinfo}{normalized_host}")),
                }
            }
        }
        None => Cow::Borrowed(url),
    }
}

/// Whether two URLs name the same scheme, host, and port, treating a host and
/// the same host with one trailing dot as the same origin.
pub(crate) fn same_origin(a: &str, b: &str) -> bool {
    origin_of(a) == origin_of(b)
}

/// Resolves a redirect location against the current URL.
pub(crate) fn resolve_redirect(current_url: &str, location: &str) -> Result<String, String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_string());
    }
    let scheme_end = current_url
        .find("://")
        .ok_or_else(|| "current URL lacks scheme".to_string())?;
    let scheme = &current_url[..scheme_end];

    if let Some(network_ref) = location.strip_prefix("//") {
        return Ok(format!("{scheme}://{network_ref}"));
    }

    if location.starts_with('/') {
        let origin = origin_of(current_url);
        return Ok(format!("{origin}{location}"));
    }

    let path_start = scheme_end + 3;
    let url_without_query = match current_url.find('?') {
        Some(q) => &current_url[..q],
        None => current_url,
    };
    // `url_without_query` can be shorter than `path_start` when the query
    // marker precedes the scheme (a hostile or prior-hop `current_url`), so
    // `get` rather than index: no path region there reads the same as none
    // found.
    let last_slash = url_without_query
        .get(path_start..)
        .and_then(|rest| rest.rfind('/'))
        .map(|idx| path_start + idx);
    match last_slash {
        Some(idx) => Ok(format!("{}{location}", &current_url[..=idx])),
        None => Ok(format!("{current_url}/{location}")),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{encode_template_value, origin_of, rebase_origin, resolve_redirect, same_origin};

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(
            encode_template_value("Abc-123_x.y~z"),
            "Abc-123_x.y~z",
            "an ordinary id must not be made unreadable"
        );
    }

    #[test]
    fn the_characters_that_reshape_a_url_are_escaped() {
        assert_eq!(encode_template_value("a#b"), "a%23b");
        assert_eq!(encode_template_value("a?b"), "a%3Fb");
        assert_eq!(encode_template_value("a/b"), "a%2Fb");
        assert_eq!(encode_template_value("a b"), "a%20b");
        assert_eq!(encode_template_value("a%b"), "a%25b");
        assert_eq!(encode_template_value(".."), "..");
    }

    #[test]
    fn a_reserved_character_in_a_query_position_is_escaped_too() {
        // `&` and `=` are legal in a path segment and would split a query
        // string; the strict set means one encoder serves both positions.
        assert_eq!(encode_template_value("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_template_value("application/octet-stream"), {
            "application%2Foctet-stream"
        });
    }

    #[test]
    fn a_multibyte_character_becomes_one_escape_per_octet() {
        assert_eq!(encode_template_value("ä"), "%C3%A4");
        assert_eq!(encode_template_value("日"), "%E6%97%A5");
    }

    #[test]
    fn rebase_origin_keeps_the_path_and_swaps_the_scheme_and_authority() {
        assert_eq!(
            rebase_origin("https://example.com/jmap", "http://10.0.0.5:8080"),
            "http://10.0.0.5:8080/jmap"
        );
    }

    #[test]
    fn rebase_origin_keeps_a_template_with_query_and_braces_intact() {
        assert_eq!(
            rebase_origin(
                "https://example.com/download/{accountId}/{blobId}/{name}",
                "http://127.0.0.1:9"
            ),
            "http://127.0.0.1:9/download/{accountId}/{blobId}/{name}"
        );
    }

    #[test]
    fn rebase_origin_strips_a_trailing_slash_from_the_new_origin() {
        assert_eq!(
            rebase_origin("https://example.com/jmap", "http://10.0.0.5:8080/"),
            "http://10.0.0.5:8080/jmap"
        );
    }

    #[test]
    fn origin_of_drops_the_path() {
        assert_eq!(
            origin_of("https://example.com/download/A1/B2/name"),
            "https://example.com"
        );
    }

    #[test]
    fn origin_of_a_bare_origin_is_itself() {
        assert_eq!(origin_of("https://example.com"), "https://example.com");
    }

    #[test]
    fn origin_of_distinguishes_host_and_port() {
        assert_ne!(
            origin_of("http://127.0.0.1:1234/download/x"),
            origin_of("http://127.0.0.1:5678/download/x")
        );
        assert_ne!(
            origin_of("http://a.example.com/x"),
            origin_of("http://b.example.com/x")
        );
    }

    #[test]
    fn origin_of_a_schemeless_string_is_itself() {
        assert_eq!(origin_of("not a url"), "not a url");
    }

    #[test]
    fn origin_of_normalizes_trailing_dot_in_both_directions() {
        assert_eq!(origin_of("https://example.com."), "https://example.com");
        assert_eq!(
            origin_of("https://example.com"),
            origin_of("https://example.com.")
        );
        assert_eq!(
            origin_of("https://example.com."),
            origin_of("https://example.com")
        );
        assert_eq!(
            origin_of("https://example.com.:8080"),
            origin_of("https://example.com:8080")
        );
        assert_eq!(
            origin_of("https://example.com:8080"),
            origin_of("https://example.com.:8080")
        );
    }

    #[test]
    fn same_origin_treats_trailing_dot_as_same_origin_in_both_directions() {
        assert!(same_origin(
            "https://example.com./.well-known/jmap",
            "https://example.com/jmap/session"
        ));
        assert!(same_origin(
            "https://example.com/.well-known/jmap",
            "https://example.com./jmap/session"
        ));
        assert!(same_origin(
            "https://example.com.:8080/download/1",
            "https://example.com:8080/download/2"
        ));
        assert!(same_origin(
            "https://example.com:8080/download/1",
            "https://example.com.:8080/download/2"
        ));
        assert!(!same_origin(
            "https://example.com:8080/download/1",
            "https://example.com:9090/download/1"
        ));
        assert!(!same_origin(
            "http://example.com/download/1",
            "https://example.com/download/1"
        ));
        assert!(!same_origin(
            "https://example.com./download/1",
            "https://other.com/download/1"
        ));
    }

    #[test]
    fn resolve_redirect_resolves_absolute_network_root_and_relative_locations() {
        assert_eq!(
            resolve_redirect(
                "https://api.fastmail.com./.well-known/jmap",
                "https://api.fastmail.com/jmap/session"
            )
            .unwrap(),
            "https://api.fastmail.com/jmap/session"
        );
        assert_eq!(
            resolve_redirect(
                "https://api.fastmail.com./.well-known/jmap",
                "/jmap/session"
            )
            .unwrap(),
            "https://api.fastmail.com/jmap/session"
        );
        assert_eq!(
            resolve_redirect("http://example.com/api/v1/auth", "session").unwrap(),
            "http://example.com/api/v1/session"
        );
        assert_eq!(
            resolve_redirect("https://example.com/foo", "//cdn.example.com/bar").unwrap(),
            "https://cdn.example.com/bar"
        );
    }

    #[test]
    fn resolve_redirect_does_not_panic_when_the_query_marker_precedes_the_scheme() {
        // `current_url` can be a prior hop's own resolved redirect, so it is
        // not guaranteed to be a well-formed URL: a `?` before the `://` made
        // `url_without_query` shorter than `path_start`, indexing past its end.
        let _ = resolve_redirect("?://", "");
    }

    proptest! {
        // `url` and `origin` here stand in for `apiUrl`/`downloadUrl`/
        // `uploadUrl`/`eventSourceUrl` straight off an untrusted server's
        // session document (see `Client::connect`'s `rebase_origin` calls):
        // a malicious or merely buggy server controls both arguments
        // entirely. Arbitrary Unicode, not just ASCII attack strings: a
        // multi-byte character straddling where `rebase_origin` slices for
        // `://` or the first `/` is exactly the kind of input manual
        // inspection is least trustworthy about.
        #[test]
        fn rebase_origin_never_panics_on_hostile_input(url in ".*", origin in ".*") {
            let _ = rebase_origin(&url, &origin);
        }

        // `url` stands in for `HttpResponse::final_url`, which a transport
        // builds from whatever a server's `Location` header names across
        // however many redirects it followed, as untrusted as `rebase_origin`'s
        // own arguments above.
        #[test]
        fn origin_of_never_panics_on_hostile_input(url in ".*") {
            let _ = origin_of(&url);
        }

        #[test]
        fn same_origin_never_panics_on_hostile_input(a in ".*", b in ".*") {
            let _ = same_origin(&a, &b);
        }

        #[test]
        fn resolve_redirect_never_panics_on_hostile_input(base in ".*", loc in ".*") {
            let _ = resolve_redirect(&base, &loc);
        }

        // `value` stands in for a server-supplied `blobId`/`accountId`, or
        // this crate's own `name` (a message uid), substituted into a
        // `downloadUrl`/`uploadUrl` template.
        #[test]
        fn encode_template_value_never_panics_on_hostile_input(value in ".*") {
            let _ = encode_template_value(&value);
        }
    }
}
