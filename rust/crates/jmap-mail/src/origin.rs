// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The origin of an account: the server and the account id that the summary
//! rows and the message cache belong to.
//!
//! The server part is the scheme, the host and the port of the `apiUrl`. The
//! path is not part of it. A server can give a different `apiUrl` path in a
//! new session document (RFC 8620 section 2), and the messages stay the same.

use std::borrow::Cow;

/// The origin of `account_id` on the server of `api_url`.
pub fn of(api_url: &str, account_id: &str) -> String {
    format!("{} {account_id}", server(api_url))
}

/// Whether two recorded origins name the same server and account.
///
/// A recorded origin can also be in the earlier form, the full `apiUrl` and
/// the account id. Both forms compare by server and account id.
pub fn same(known: &str, origin: &str) -> bool {
    key(known) == key(origin)
}

fn key(origin: &str) -> Cow<'_, str> {
    match origin.rsplit_once(' ') {
        Some((url, account_id)) => Cow::Owned(of(url, account_id)),
        None => Cow::Borrowed(origin),
    }
}

/// The scheme, host and port of `url`, in lower case, with no user part, no
/// default port and no trailing dot on the host. A value that is not an
/// absolute URL comes back unchanged.
fn server(url: &str) -> Cow<'_, str> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Cow::Borrowed(url);
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, rest)) => (&authority[..host.len() + 2], rest.strip_prefix(':')),
            None => return Cow::Borrowed(url),
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() {
        return Cow::Borrowed(url);
    }
    let scheme = scheme.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    };
    match port.filter(|port| !port.is_empty() && Some(*port) != default) {
        Some(port) => Cow::Owned(format!("{scheme}://{host}:{port}")),
        None => Cow::Owned(format!("{scheme}://{host}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{of, same};

    #[test]
    fn the_path_is_not_part_of_the_origin() {
        assert!(same(
            &of("https://mail.example/jmap", "A1"),
            &of("https://mail.example/jmap/", "A1")
        ));
        assert!(same(
            &of("https://mail.example/jmap", "A1"),
            &of("https://mail.example/api/v2/jmap?x=1", "A1")
        ));
    }

    #[test]
    fn spellings_of_one_server_are_one_origin() {
        let origin = of("https://mail.example/jmap", "A1");
        for url in [
            "HTTPS://Mail.Example/jmap",
            "https://mail.example:443/jmap",
            "https://mail.example./jmap",
            "https://user@mail.example/jmap",
        ] {
            assert!(same(&origin, &of(url, "A1")), "{url}");
        }
    }

    #[test]
    fn port_80_is_the_default_port_of_http() {
        assert!(same(
            &of("http://mail.example/jmap", "A1"),
            &of("http://mail.example:80/jmap", "A1")
        ));
    }

    #[test]
    fn another_server_or_account_is_another_origin() {
        let origin = of("https://mail.example/jmap", "A1");
        for (url, account_id) in [
            ("https://other.example/jmap", "A1"),
            ("http://mail.example/jmap", "A1"),
            ("https://mail.example:8443/jmap", "A1"),
            ("https://mail.example/jmap", "A2"),
        ] {
            assert!(!same(&origin, &of(url, account_id)), "{url} {account_id}");
        }
    }

    #[test]
    fn an_ipv6_host_keeps_its_port() {
        assert_eq!(of("http://[::1]:8080/jmap", "A1"), "http://[::1]:8080 A1");
        assert!(!same(
            &of("http://[::1]:8080/jmap", "A1"),
            &of("http://[::1]:8081/jmap", "A1")
        ));
    }

    #[test]
    fn an_origin_of_the_earlier_form_is_the_same_origin() {
        assert!(same(
            "https://mail.example/jmap A1",
            &of("https://mail.example/jmap/", "A1")
        ));
        assert!(!same(
            "https://mail.example/jmap A1",
            &of("https://other.example/jmap", "A1")
        ));
    }

    #[test]
    fn a_value_that_is_not_a_url_compares_as_it_is() {
        assert!(same("not a url", "not a url"));
        assert!(!same("jmap A1", "jmap/ A1"));
    }
}
