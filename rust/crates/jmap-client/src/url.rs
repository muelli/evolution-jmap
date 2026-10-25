// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Substitutes values into session URL templates.
//!
//! RFC 8620 section 6.1 and section 6.2 describe `uploadUrl` and `downloadUrl` as templates.
//! The client must percent-encode each substituted variable value.
//! Values like `accountId` and `blobId` arrive from the network without validation.
//! Unencoded delimiters like `#`, `?`, or `/` would change URL structure unexpectedly.
//!
//! The encoder uses the strict RFC 3986 unreserved character set.
//! A template variable can appear in either a path segment or a query parameter.
//! Percent-encoding all characters outside the unreserved set works correctly in both positions.

use std::borrow::Cow;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// RFC 3986 section 2.3 unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`).
///
/// Defines bytes to escape instead of bytes to keep.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encodes a value for substitution into a URL template.
///
/// Unreserved characters pass through unchanged.
/// Other bytes become uppercase hex escapes per RFC 3986 section 2.1.
/// The encoder encodes multi-byte UTF-8 octets individually.
pub(crate) fn encode_template_value(value: &str) -> String {
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// Replaces `url`'s scheme and authority with `origin`'s, keeping path and query untouched.
///
/// Backs [`crate::ClientBuilder::rebase_urls_to_origin`].
/// A session document defines authoritative paths for endpoints.
/// Reverse proxies or NAT boundaries may require connecting through an alternate origin.
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

/// The scheme, host and port of `url`, in lower case, with no user part, no
/// default port (80 for http, 443 for https) and no trailing root dot on the host.
/// A value that is not an absolute URL comes back unchanged.
pub fn origin(url: &str) -> Cow<'_, str> {
    let Some(scheme_end) = url.find("://") else {
        return Cow::Borrowed(url);
    };
    let scheme = &url[..scheme_end];
    let after_scheme = scheme_end + 3;
    let authority_end = match url[after_scheme..].find(['/', '?', '#']) {
        Some(pos) => after_scheme + pos,
        None => url.len(),
    };
    let authority = &url[after_scheme..authority_end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);

    let (host, port) = if host_port.starts_with('[') {
        match host_port.find(']') {
            Some(end) => {
                let host = &host_port[..=end];
                let rest = &host_port[end + 1..];
                (host, rest.strip_prefix(':'))
            }
            None => return Cow::Borrowed(url),
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };

    if host.is_empty() {
        return Cow::Borrowed(url);
    }

    let normalized_host = if host.len() > 1 && host.ends_with('.') && !host.ends_with("..") {
        &host[..host.len() - 1]
    } else {
        host
    };

    let default_port = if scheme.eq_ignore_ascii_case("http") {
        Some("80")
    } else if scheme.eq_ignore_ascii_case("https") {
        Some("443")
    } else {
        None
    };

    let normalized_port = port.filter(|p| !p.is_empty() && default_port != Some(*p));

    let scheme_needs_lower = scheme.bytes().any(|b| b.is_ascii_uppercase());
    let host_needs_lower = normalized_host.bytes().any(|b| b.is_ascii_uppercase());
    let host_changed = normalized_host != host;
    let port_changed = port != normalized_port;
    let had_userinfo = host_port.len() != authority.len();

    if !scheme_needs_lower && !host_needs_lower && !host_changed && !port_changed && !had_userinfo {
        Cow::Borrowed(&url[..authority_end])
    } else {
        let scheme = scheme.to_ascii_lowercase();
        let host = normalized_host.to_ascii_lowercase();
        match normalized_port {
            Some(p) => Cow::Owned(format!("{scheme}://{host}:{p}")),
            None => Cow::Owned(format!("{scheme}://{host}")),
        }
    }
}

/// Scheme and authority prefix of `url`.
///
/// Delegates to [`origin`].
pub fn origin_of(url: &str) -> Cow<'_, str> {
    origin(url)
}

/// Checks whether two URLs have the same scheme, host, and port.
///
/// Normalizes scheme case, host case, default ports, and trailing root dots before comparing.
pub fn same_origin(a: &str, b: &str) -> bool {
    origin(a) == origin(b)
}

/// Whether a redirect from `current` to `next` leaves TLS behind.
pub(crate) fn downgrades(current: &str, next: &str) -> bool {
    on_tls(current) && !on_tls(next)
}

/// Splits an `http://` or `https://` URL into a TLS flag and the rest of the string.
///
/// Returns `Some((true, rest))` for `https://` and `Some((false, rest))` for `http://`.
/// Matches the scheme without case sensitivity.
/// Returns `None` for any other scheme or for a relative path.
pub fn split_http_scheme(url: &str) -> Option<(bool, &str)> {
    if url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        Some((true, &url[8..]))
    } else if url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        Some((false, &url[7..]))
    } else {
        None
    }
}

/// Whether `url` uses TLS.
fn on_tls(url: &str) -> bool {
    split_http_scheme(url).is_some_and(|(tls, _)| tls)
}

/// Whether the host of `url` is a loopback name or address.
///
/// The authority ends at the first `/`, `?`, `#` or `\`, and the host follows the last `@`, as in the WHATWG URL parser.
/// A host that this function cannot read is not loopback.
pub(crate) fn is_loopback(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    let rest = &url[scheme_end + 3..];
    let authority = &rest[..rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len())];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.strip_prefix('[') {
        Some(bracketed) => match bracketed.find(']') {
            Some(end) => &bracketed[..end],
            None => return false,
        },
        None => &host_port[..host_port.rfind(':').unwrap_or(host_port.len())],
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("localhost.localdomain")
    {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

/// Whether a session endpoint may receive the credentials of `session_url`.
///
/// The endpoint must use TLS or a loopback host.
/// A plain-http endpoint is also accepted on the origin of a plain-http `session_url`: the account settings already accepted that origin.
/// An empty endpoint names no endpoint.
pub(crate) fn may_carry_credentials(session_url: &str, endpoint: &str) -> bool {
    endpoint.is_empty()
        || on_tls(endpoint)
        || is_loopback(endpoint)
        || (!on_tls(session_url) && same_origin(session_url, endpoint))
}

/// Resolves a redirect location against the current URL.
pub(crate) fn resolve_redirect(current_url: &str, location: &str) -> Result<String, String> {
    if split_http_scheme(location).is_some() {
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
    // A query marker before the scheme can make `url_without_query` shorter than `path_start`.
    // Safe slicing with `get` avoids indexing past the end of the string.
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

    use super::{
        downgrades, encode_template_value, is_loopback, may_carry_credentials, origin, origin_of,
        rebase_origin, resolve_redirect, same_origin, split_http_scheme,
    };

    #[test]
    fn only_a_loopback_host_is_loopback() {
        for url in [
            "http://localhost/jmap",
            "http://LOCALHOST:8080/jmap",
            "http://localhost./jmap",
            "http://127.0.0.1:8080",
            "http://127.9.9.9/jmap",
            "http://[::1]:8080/jmap",
            "http://user@127.0.0.1/jmap",
        ] {
            assert!(is_loopback(url), "{url} should be loopback");
        }
        for url in [
            "http://evil.example#@127.0.0.1/",
            "http://evil.example?@localhost/",
            "http://evil.example\\@localhost/",
            "http://localhost.example.net/",
            "http://127.0.0.1.example.net/",
            "http://localhost@evil.example/",
            "http://10.0.0.5/jmap",
            "http://[::1/jmap",
            "localhost",
        ] {
            assert!(!is_loopback(url), "{url} should not be loopback");
        }
    }

    #[test]
    fn credentials_stay_on_tls_loopback_or_the_configured_plain_origin() {
        let tls = "https://jmap.example.com/.well-known/jmap";
        let plain = "http://jmap.example.com/.well-known/jmap";
        assert!(may_carry_credentials(tls, "https://other.example.net/jmap"));
        assert!(may_carry_credentials(tls, "http://127.0.0.1:9/jmap"));
        assert!(may_carry_credentials(tls, ""));
        assert!(!may_carry_credentials(tls, "http://jmap.example.com/jmap"));
        assert!(may_carry_credentials(plain, "http://jmap.example.com/jmap"));
        assert!(!may_carry_credentials(
            plain,
            "http://other.example.net/jmap"
        ));
        assert!(!may_carry_credentials(plain, "ftp://jmap.example.com/jmap"));
    }

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
        // Strict encoding escapes query delimiters so one encoder serves both path and query.
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
        // A malformed URL with a `?` before `://` must not index out of bounds.
        let _ = resolve_redirect("?://", "");
    }

    #[test]
    fn rebase_origin_with_http_scheme_and_short_authority() {
        assert_eq!(
            rebase_origin("http://example.com/path", "https://target.org"),
            "https://target.org/path"
        );
        assert_eq!(
            rebase_origin("http://a/b", "https://dest"),
            "https://dest/b"
        );
        assert_eq!(rebase_origin("https://a/b", "http://c"), "http://c/b");
    }

    #[test]
    fn origin_of_strips_userinfo_in_authority() {
        assert_eq!(
            origin_of("https://user:pass@example.com/path"),
            "https://example.com"
        );
        assert_eq!(
            origin_of("https://user:pass@example.com:8080/path"),
            "https://example.com:8080"
        );
        assert_eq!(
            origin_of("https://user:pass@example.com.:8080/path"),
            "https://example.com:8080"
        );
        assert_eq!(
            origin_of("https://user@example.com/path"),
            "https://example.com"
        );
    }

    #[test]
    fn origin_drops_default_ports_and_lowercases_scheme_and_host() {
        assert_eq!(
            origin("https://mail.example:443/jmap"),
            "https://mail.example"
        );
        assert_eq!(origin("http://mail.example:80/jmap"), "http://mail.example");
        assert_eq!(
            origin("HTTPS://Mail.Example:443/jmap"),
            "https://mail.example"
        );
        assert_eq!(origin("HTTP://Mail.Example:80/jmap"), "http://mail.example");
        assert_eq!(
            origin("https://mail.example.:443/jmap"),
            "https://mail.example"
        );
        assert_eq!(
            origin("https://mail.example:8443/jmap"),
            "https://mail.example:8443"
        );
        assert_eq!(
            origin("http://mail.example:8080/jmap"),
            "http://mail.example:8080"
        );
        assert_eq!(origin("http://[::1]:8080/jmap"), "http://[::1]:8080");
        assert_eq!(origin("https://[::1]:443/jmap"), "https://[::1]");
    }

    #[test]
    fn origin_of_handles_ipv6_host_with_and_without_port() {
        assert_eq!(
            origin_of("http://[2001:db8::1]:8080/path"),
            "http://[2001:db8::1]:8080"
        );
        assert_eq!(
            origin_of("http://[2001:db8::1]/path"),
            "http://[2001:db8::1]"
        );
        assert_eq!(
            origin_of("https://[::1]:9999/download"),
            "https://[::1]:9999"
        );
        assert_eq!(origin_of("https://[::1]/download"), "https://[::1]");
    }

    #[test]
    fn origin_of_preserves_single_dot_host() {
        assert_eq!(origin_of("http://./foo"), "http://.");
        assert_eq!(origin_of("http://../foo"), "http://..");
    }

    #[test]
    fn a_redirect_from_https_to_plain_http_is_a_downgrade() {
        // RFC 8620 §8.1: every request uses TLS, so a session that started on
        // TLS must not continue without it.
        assert!(downgrades("https://a/s", "http://a/s"));
        assert!(downgrades("HTTPS://a/s", "Http://b/s"));
        assert!(downgrades("https://a/s", "ftp://a/s"));
        assert!(!downgrades("https://a/s", "https://b/s"));
        assert!(!downgrades("https://a/s", "HTTPS://b/s"));
        // A plain HTTP start (a local test server) has nothing to lose.
        assert!(!downgrades("http://127.0.0.1/s", "http://127.0.0.1/t"));
    }

    #[test]
    fn resolve_redirect_handles_short_urls() {
        assert_eq!(resolve_redirect("https://a/b", "c").unwrap(), "https://a/c");
        assert_eq!(resolve_redirect("http://x/y", "z").unwrap(), "http://x/z");
        assert_eq!(
            resolve_redirect("https://a/b/", "c").unwrap(),
            "https://a/b/c"
        );
    }

    #[test]
    fn resolve_redirect_handles_uppercase_and_mixed_case_schemes() {
        assert_eq!(
            resolve_redirect(
                "http://example.com/api/v1/auth",
                "HTTPS://other.example.com/session"
            )
            .unwrap(),
            "HTTPS://other.example.com/session"
        );
        assert_eq!(
            resolve_redirect(
                "http://example.com/api/v1/auth",
                "Https://other.example.com/session"
            )
            .unwrap(),
            "Https://other.example.com/session"
        );
        assert_eq!(
            resolve_redirect(
                "http://example.com/api/v1/auth",
                "HTTP://other.example.com/session"
            )
            .unwrap(),
            "HTTP://other.example.com/session"
        );
        assert_eq!(
            resolve_redirect(
                "http://example.com/api/v1/auth",
                "Http://other.example.com/session"
            )
            .unwrap(),
            "Http://other.example.com/session"
        );
    }

    #[test]
    fn split_http_scheme_detects_tls_and_plain_http_case_insensitively() {
        assert_eq!(
            split_http_scheme("https://example.com/path"),
            Some((true, "example.com/path"))
        );
        assert_eq!(
            split_http_scheme("HTTPS://example.com/path"),
            Some((true, "example.com/path"))
        );
        assert_eq!(
            split_http_scheme("Https://example.com/path"),
            Some((true, "example.com/path"))
        );
        assert_eq!(
            split_http_scheme("http://example.com/path"),
            Some((false, "example.com/path"))
        );
        assert_eq!(
            split_http_scheme("HTTP://example.com/path"),
            Some((false, "example.com/path"))
        );
        assert_eq!(
            split_http_scheme("Http://example.com/path"),
            Some((false, "example.com/path"))
        );
        assert_eq!(split_http_scheme("ftp://example.com/path"), None);
        assert_eq!(split_http_scheme("/relative/path"), None);
        assert_eq!(split_http_scheme(""), None);
    }

    proptest! {
        // Verifies that arbitrary Unicode inputs do not panic `rebase_origin`.
        // A server controls both template URLs and origin strings.
        #[test]
        fn rebase_origin_never_panics_on_hostile_input(url in ".*", origin in ".*") {
            let _ = rebase_origin(&url, &origin);
        }

        // Verifies that arbitrary server redirect locations do not panic `origin_of`.
        #[test]
        fn origin_of_never_panics_on_hostile_input(url in ".*") {
            let _ = origin_of(&url);
        }

        // Verifies that arbitrary input strings do not panic `split_http_scheme`.
        #[test]
        fn split_http_scheme_never_panics_on_hostile_input(url in ".*") {
            let _ = split_http_scheme(&url);
        }

        #[test]
        fn same_origin_never_panics_on_hostile_input(a in ".*", b in ".*") {
            let _ = same_origin(&a, &b);
        }

        #[test]
        fn is_loopback_never_panics_on_hostile_input(url in ".*") {
            let _ = is_loopback(&url);
        }

        #[test]
        fn resolve_redirect_never_panics_on_hostile_input(base in ".*", loc in ".*") {
            let _ = resolve_redirect(&base, &loc);
        }

        // Verifies that arbitrary values do not panic `encode_template_value`.
        #[test]
        fn encode_template_value_never_panics_on_hostile_input(value in ".*") {
            let _ = encode_template_value(&value);
        }
    }
}
