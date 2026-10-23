// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared helpers for the live-server tests. Use this crate as a dev-dependency only.
//!
//! The `JMAP_LIVE_SERVER_*` variables name the test accounts. See
//! `docs/manual-test-live-server.md` for the list.

use std::time::{SystemTime, UNIX_EPOCH};

use jmap_client::{Client, ClientBuilder, Credentials, Error as ClientError};

/// Returns a value unique to this run, so that an object from a different run
/// cannot match.
pub fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// One test account on the live server, as the environment names it.
pub struct LiveAccount {
    pub origin: String,
    pub user: String,
    password: String,
    rebase: bool,
    label: &'static str,
}

impl LiveAccount {
    /// Returns the write-test account (`JMAP_LIVE_SERVER_WRITE_*`).
    ///
    /// Returns `None` when `JMAP_LIVE_SERVER_WRITE_USER` is unset. Panics when
    /// that variable is set but the password or `JMAP_LIVE_SERVER_URL` is not.
    pub fn write() -> Option<Self> {
        Self::read("WRITE", "write-test account", env_value)
    }

    /// Returns the second account in the same domain (`JMAP_LIVE_SERVER_RECIPIENT_*`).
    ///
    /// Returns `None` and panics as [`LiveAccount::write`] does.
    pub fn recipient() -> Option<Self> {
        Self::read("RECIPIENT", "recipient account", env_value)
    }

    fn read(
        role: &str,
        label: &'static str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        let user_var = format!("JMAP_LIVE_SERVER_{role}_USER");
        let password_var = format!("JMAP_LIVE_SERVER_{role}_PASSWORD");
        let user = lookup(&user_var)?;
        let password = lookup(&password_var)
            .unwrap_or_else(|| panic!("{user_var} is set but {password_var} is not"));
        let origin = lookup("JMAP_LIVE_SERVER_URL")
            .unwrap_or_else(|| panic!("set JMAP_LIVE_SERVER_URL alongside {user_var}"));
        let rebase = lookup("JMAP_LIVE_SERVER_REBASE_URLS").is_some_and(|value| value != "0");
        Some(Self {
            origin,
            user,
            password,
            rebase,
            label,
        })
    }

    /// Connects with the default client settings. Panics when the session fetch fails.
    pub fn connect(&self) -> Client {
        self.connect_with(Client::builder())
    }

    /// Connects with `builder`, for example one with a test transport.
    ///
    /// This sets URL rebasing from `JMAP_LIVE_SERVER_REBASE_URLS`. Panics when
    /// the session fetch fails.
    pub fn connect_with(&self, builder: ClientBuilder) -> Client {
        builder
            .rebase_urls_to_origin(self.rebase)
            .connect(
                &self.origin,
                Credentials::basic(self.user.clone(), self.password.clone()),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "could not fetch the session document for the {}: {error:?}",
                    self.label
                )
            })
    }
}

/// Connects the write-test account, or returns `None` when it is not configured.
pub fn connect_for_write() -> Option<Client> {
    LiveAccount::write().map(|account| account.connect())
}

/// Connects the recipient account, or returns `None` when it is not configured.
pub fn connect_recipient() -> Option<Client> {
    LiveAccount::recipient().map(|account| account.connect())
}

/// As [`connect_for_write`], and also returns the account's user name.
pub fn connect_owner() -> Option<(Client, String)> {
    LiveAccount::write().map(|account| (account.connect(), account.user))
}

/// As [`connect_recipient`], and also returns the account's user name.
pub fn connect_recipient_with_user() -> Option<(Client, String)> {
    LiveAccount::recipient().map(|account| (account.connect(), account.user))
}

/// Returns true if the server refused a method or a `/set` entry as `forbidden`.
pub fn is_forbidden(error: &ClientError) -> bool {
    match error {
        ClientError::Method(method_error) => method_error.error_type == "forbidden",
        ClientError::Set(set_error) => set_error.error_type == "forbidden",
        _ => false,
    }
}

/// Returns the number in the variable `name`, or `default` when it is unset or not a number.
pub fn env_count(name: &str, default: usize) -> usize {
    count_from(env_value(name), default)
}

fn count_from(value: Option<String>, default: usize) -> usize {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jmap_client::proto::error::{MethodError, SetError};
    use jmap_mock::MockServer;

    use super::*;

    fn read_from(role: &str, pairs: &[(&str, &str)]) -> Option<LiveAccount> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        LiveAccount::read(role, "write-test account", |name| values.get(name).cloned())
    }

    const COMPLETE: &[(&str, &str)] = &[
        ("JMAP_LIVE_SERVER_WRITE_USER", "agent-a@example.test"),
        ("JMAP_LIVE_SERVER_WRITE_PASSWORD", "secret"),
        ("JMAP_LIVE_SERVER_URL", "http://127.0.0.1:1"),
    ];

    #[test]
    fn unique_suffix_differs_between_two_calls() {
        assert_ne!(unique_suffix(), unique_suffix());
    }

    #[test]
    fn an_unset_user_skips_the_test() {
        assert!(read_from("WRITE", &[("JMAP_LIVE_SERVER_URL", "http://x")]).is_none());
    }

    #[test]
    fn the_role_selects_the_variables() {
        assert!(read_from("RECIPIENT", COMPLETE).is_none());
        let account = read_from("WRITE", COMPLETE).unwrap();
        assert_eq!(account.user, "agent-a@example.test");
        assert_eq!(account.password, "secret");
        assert_eq!(account.origin, "http://127.0.0.1:1");
    }

    #[test]
    #[should_panic(
        expected = "JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not"
    )]
    fn a_user_without_a_password_panics() {
        read_from("WRITE", &[COMPLETE[0], COMPLETE[2]]);
    }

    #[test]
    #[should_panic(expected = "set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER")]
    fn a_user_without_a_url_panics() {
        read_from("WRITE", &COMPLETE[..2]);
    }

    #[test]
    fn rebasing_is_on_for_any_value_but_0() {
        let rebase = |value: Option<&str>| {
            let mut pairs = COMPLETE.to_vec();
            pairs.extend(value.map(|value| ("JMAP_LIVE_SERVER_REBASE_URLS", value)));
            read_from("WRITE", &pairs).unwrap().rebase
        };
        assert!(!rebase(None));
        assert!(!rebase(Some("0")));
        assert!(rebase(Some("1")));
        assert!(rebase(Some("yes")));
    }

    #[test]
    fn connect_sends_the_account_credentials() {
        let server = MockServer::builder()
            .basic_auth("agent-a@example.test", "secret")
            .start();
        let origin = server.origin().to_string();
        let account = read_from(
            "WRITE",
            &[COMPLETE[0], COMPLETE[1], ("JMAP_LIVE_SERVER_URL", &origin)],
        )
        .unwrap();
        account.connect();
    }

    #[test]
    #[should_panic(expected = "could not fetch the session document for the write-test account")]
    fn connect_panics_with_the_label_on_a_refused_login() {
        let server = MockServer::builder()
            .basic_auth("agent-a@example.test", "other")
            .start();
        let origin = server.origin().to_string();
        read_from(
            "WRITE",
            &[COMPLETE[0], COMPLETE[1], ("JMAP_LIVE_SERVER_URL", &origin)],
        )
        .unwrap()
        .connect();
    }

    #[test]
    fn forbidden_is_a_method_error_or_a_set_error() {
        let method = |error_type: &str| ClientError::Method(MethodError::new(error_type));
        let set = |error_type: &str| ClientError::Set(SetError::new(error_type));
        assert!(is_forbidden(&method("forbidden")));
        assert!(is_forbidden(&set("forbidden")));
        assert!(!is_forbidden(&method("notFound")));
        assert!(!is_forbidden(&set("notFound")));
        assert!(!is_forbidden(&ClientError::Http {
            status: 403,
            problem: None
        }));
    }

    #[test]
    fn a_count_falls_back_to_the_default() {
        assert_eq!(count_from(None, 500), 500);
        assert_eq!(count_from(Some("12".into()), 500), 12);
        assert_eq!(count_from(Some("many".into()), 500), 500);
    }
}
