// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The headless answer to an operator observation.
//! `evolution-source-registry` logged
//!
//! ```text
//! server_side_source_credentials_lookup_cb: Failed to lookup password for
//! source d06b56726065b3531de05498dd3519c2bd6efcb3
//! ```
//!
//! two times, and in the same second an interactive `authorization_code`
//! consent followed. The UID was not in `~/.config/evolution/sources/`. Its
//! only occurrence on the machine was in `~/.config/dconf/user`.
//!
//! This file answers two questions. One test answers the first. A pair of
//! tests answers the second, because that measurement must also be able to
//! give the opposite result.
//!
//! # 1. Can dconf debris cause it?
//!
//! No, for a structural reason. In EDS 3.52.3,
//! `server_side_source_credentials_lookup_cb` (`e-server-side-source.c:418`)
//! holds a reference to a live `EServerSideSource` GObject. A live source
//! exists only if EDS loaded a `.source` keyfile. dconf holds no source data.
//! The only dconf source-UID mechanism in EDS is the
//! `org.gnome.Evolution.DefaultSources` schema (`e-source-registry.c:68`).
//! Its six keys hold only a UID *reference*. The getters resolve it with
//! `e_source_registry_ref_source` and use a fallback for NULL
//! (`e_source_registry_ref_default_mail_account`, line 3300). The first test
//! writes the UID into all six keys, and checks that it is only in the
//! `dconf/user` file of the session. It then makes sure that the registry
//! makes no source for the UID and that no default-source getter returns it.
//!
//! # 2. Then what caused that log line?
//!
//! A normal child source. `collection_backend_new_user_file`
//! (`e-collection-backend.c:176-200`) writes the children of a collection
//! backend to `$XDG_CACHE_HOME/evolution/sources/<collection-uid>/`, not to the
//! config directory. **Each address book and calendar of an account is absent
//! from `~/.config/evolution/sources/`**, so absence does not show a stale
//! source. The second test runs the collection backend of this project
//! against the mock server, takes a child, and does the operator path on it.
//!
//! # Why the escalation is the designed EDS behaviour
//!
//! `server_side_source_invoke_credentials_required_cb`
//! (`e-server-side-source.c:252`) does not send a `required` request to
//! clients. It sets `skip_emit = TRUE` (line 318) and starts a silent
//! credentials lookup. The callback decides what the user sees. A failure
//! emits `credentials-required` again (the prompter, or for an OAuth2-method
//! source the consent window). A success emits `authenticate`, and the user
//! sees nothing. Thus the two log lines are ONE code path on ONE live source.
//! The consent is the designed result of the lookup failure.
//!
//! Thus the third test is a necessary control. It uses the same source and
//! the same request, but with a password in the store, and it must give the
//! opposite pair of signals. Without it, a run cannot tell a real escalation
//! from an echo of the request of this harness.

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};

/// A UID with the shape that `e_util_generate_uid()` makes (40 lowercase hex
/// characters). No file on disk has this UID.
const DANGLING_UID: &str = "d06b56726065b3531de05498dd3519c2bd6efcb3";

/// The collection account of the two tests for the second question.
const ACCOUNT_UID: &str = "jmap-functional-stale-source-uid";

/// The keyfile of `collection.rs` with a different `DisplayName`, and with
/// the ephemeral port of the mock.
fn keyfile(port: u16) -> String {
    Keyfile::new(
        SourceKind::Collection {
            contacts: true,
            calendars: true,
            mail: false,
        },
        port,
    )
    .display_name("JMAP item-26 reproduction account")
    .to_string()
}

#[test]
fn a_uid_reachable_only_from_dconf_never_becomes_a_source_and_is_never_looked_up() {
    let client = required_path("JMAP_FUNCTIONAL_STALE_SOURCE_UID_CLIENT");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/stale-source-uid-dconf"
    ));
    // Enable the EDS diagnostics, so that the operator log line is visible
    // here if it occurs. An assertion below makes sure that it does not occur.
    session.set_variable("ESR_DEBUG", "1");

    let output = session.run(&client, &["dconf-only", DANGLING_UID]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let seen = observations(&stdout);

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );
    assert_eq!(
        seen.get("done"),
        Some(&"1"),
        "the client did not run to the end\n{report}"
    );

    // (1) The precondition: the UID is in `~/.config/dconf/user` of this
    // session and in no other location. If GSettings uses the memory backend
    // (no dconf GIO module, or no `ca.desrt.dconf` activation on the private
    // bus), no daemon can see the value, and the test measures nothing.
    assert_eq!(
        seen.get("dconf-file-present"),
        Some(&"1"),
        "the session has no dconf/user file, so GSettings did not use the dconf \
         backend and this test would be measuring an in-process value rather \
         than the operator's on-disk debris\n{report}"
    );
    assert_eq!(
        seen.get("dconf-file-names-uid"),
        Some(&"1"),
        "the dangling UID was not written into the session's dconf/user, so \
         nothing here reproduces the operator's finding\n{report}"
    );
    // The UID has no keyfile. The operator did this check manually.
    let sources = session.sources_directory();
    assert!(
        !sources.join(format!("{DANGLING_UID}.source")).exists(),
        "the dangling UID has a keyfile in {sources:?}, which would make it an \
         ordinary configured source and not debris at all\n{report}"
    );

    // (2) The answer. `e_source_registry_ref_source` searches only the
    // sources that the registry built from files. EDS makes no source from a
    // dconf UID.
    assert_eq!(
        seen.get("dangling-uid-in-registry"),
        Some(&"0"),
        "the registry has a source for a UID that exists only in dconf, which \
         would mean EDS fabricates sources from GSettings\n{report}"
    );
    let default_mail_account = seen
        .get("default-mail-account-uid")
        .unwrap_or_else(|| panic!("the client reported no default mail account\n{report}"));
    assert_ne!(
        *default_mail_account, DANGLING_UID,
        "e_source_registry_ref_default_mail_account answered the dangling UID \
         instead of falling back to the built-in account\n{report}"
    );

    // (3) The source list of the registry does not get the UID during the
    // settling time. The test examines the list, not only `ref_source`, to
    // find a source that appears late after a deferred read of the setting.
    for key in ["source-uids", "source-uids-after-settling"] {
        let uids = seen
            .get(key)
            .unwrap_or_else(|| panic!("the client reported no {key}\n{report}"));
        assert!(
            !uids.split(',').any(|uid| uid == DANGLING_UID),
            "{key} contains the dangling UID\n{report}"
        );
    }

    // (4) The operator log line does not appear. This proves that the debris
    // is harmless. The assertion has a meaning only because of ESR_DEBUG=1
    // above, which also made the line visible on the operator machine.
    assert!(
        !stdout.contains("Failed to lookup password for source"),
        "a credentials lookup ran, and failed, in a session whose only source \
         UID debris is in dconf\n{report}"
    );
}

/// Runs the collection account, waits for a child, and returns the child UID,
/// the stdout and the report. The two tests for the second question use it,
/// with and without a password in the store.
fn run_collection_child(name: &str, password: Option<&str>) -> (String, String, String) {
    let client = required_path("JMAP_FUNCTIONAL_STALE_SOURCE_UID_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");

    let server = jmap_mock::MockServer::builder().start();
    let account_id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_address_book("Personal", true);
        account.seed_calendar("Personal", true);
    }
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(format!("{}/{name}", env!("CARGO_TARGET_TMPDIR")));
    session.write_source(ACCOUNT_UID, &keyfile(port));
    session.stage_collection_backend(&module);
    session.set_variable("ESR_DEBUG", "1");

    let mut arguments = vec!["collection-child", ACCOUNT_UID];
    arguments.extend(password);
    let output = session.run(&client, &arguments);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let seen = observations(&stdout);
    let child_uid = (*seen
        .get("child-uid")
        .unwrap_or_else(|| panic!("the client reported no child UID\n{report}")))
    .to_owned();

    // The *file* part of the answer, examined in the session directories.
    // The child has no keyfile in the config directory, but it has one in the
    // cache directory where EDS puts children.
    let configured = session
        .sources_directory()
        .join(format!("{child_uid}.source"));
    let cached = session
        .cache_sources_directory(ACCOUNT_UID)
        .join(format!("{child_uid}.source"));
    assert!(
        !configured.exists(),
        "a collection child has a keyfile at {configured:?}. If EDS has started \
         writing children to the config directory, then 'this UID matches no \
         configured source' really would be evidence of debris, overturning \
         the premise this test rests on\n{report}"
    );
    assert!(
        cached.exists(),
        "the child's keyfile is not at {cached:?} either, so this test has not \
         found where EDS put it and proves nothing about the config \
         directory's silence\n{report}"
    );

    (child_uid, stdout, report)
}

#[test]
fn a_live_collection_child_has_no_configured_keyfile_and_a_failed_lookup_for_it_escalates() {
    let (child_uid, stdout, report) = run_collection_child("stale-source-uid-child", None);
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("account-found"),
        Some(&"1"),
        "the registry never saw the account keyfile at all\n{report}"
    );
    assert_eq!(
        seen.get("done"),
        Some(&"1"),
        "the client did not run to the end\n{report}"
    );

    // The escalation: exactly one, and no `authenticate`. The registry did a
    // silent lookup in the store, found nothing, and showed the prompter.
    // Because of `skip_emit = TRUE`, this count is the escalation, not an echo
    // of the client request (see the file header).
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"1"),
        "the credentials request did not reach the client as an escalation. \
         If this is 0, the registry answered it silently and there is no \
         consent to explain; if it is 2 or more, `skip_emit` no longer \
         suppresses the immediate echo and this test is counting something \
         else\n{report}"
    );
    assert_eq!(
        seen.get("authenticate"),
        Some(&"0"),
        "the registry found credentials for a source nothing ever stored any \
         for\n{report}"
    );

    // The second reason why the operator did not know the UID. The message
    // names the source of the request, not the source of the secret-store
    // lookup. For a collection child, these are different.
    // `source_credential_provider_ref_impl_for_source`
    // (`e-source-credentials-provider.c:216-237`) first resolves the
    // credentials source, which is the collection. Thus `Failed to lookup
    // password for source <uid>` is not about a keyring entry for `<uid>`.
    assert_eq!(
        seen.get("credentials-source-uid"),
        Some(&ACCOUNT_UID),
        "the child's credentials source is not the collection account. If EDS \
         has stopped sharing a collection's credentials with its children, the \
         reading of the operator's log line below changes\n{report}"
    );
    assert_ne!(
        seen.get("credentials-source-uid"),
        seen.get("child-uid"),
        "the child is its own credentials source, so the log line does name \
         the UID whose secret was looked up after all\n{report}"
    );

    // The operator log line, for a UID of the same shape and a source with no
    // keyfile in the config directory. A new account, not debris, causes it.
    let expected = format!(
        "server_side_source_credentials_lookup_cb: Failed to lookup password for source {child_uid}"
    );
    assert!(
        stdout.contains(&expected),
        "the registry did not log {expected:?}. Either ESR_DEBUG=1 no longer \
         reaches the daemon, or its stdout no longer reaches this run's \
         capture\n{report}"
    );
}

#[test]
fn and_the_same_request_is_answered_silently_when_the_password_is_in_the_store() {
    // The control. It is the same as the test above, but first the client
    // stores a password under the credentials source of the child. If the
    // counts do not change, the test above measures the harness, not the
    // registry.
    let (child_uid, stdout, report) =
        run_collection_child("stale-source-uid-child-stored", Some("item-26-secret"));
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("password-stored"),
        Some(&"1"),
        "the client did not take the stored-password path\n{report}"
    );
    // The password goes to the credentials source, not to the source in the
    // log line. See `child-uid` and `credentials-source-uid` above.
    assert_eq!(
        seen.get("credentials-source-uid"),
        Some(&ACCOUNT_UID),
        "the child's credentials source is not the collection account, so the \
         password below was stored somewhere the registry would not have \
         looked and this control proves nothing\n{report}"
    );
    assert_eq!(
        seen.get("authenticate"),
        Some(&"1"),
        "the registry did not hand the stored password to the backend. Without \
         this the run below proves only that nothing happened\n{report}"
    );
    assert_eq!(
        seen.get("credentials-required"),
        Some(&"0"),
        "the user was still escalated to although the password was in the \
         store — which would mean the escalation is unconditional and the \
         lookup's outcome does not decide it\n{report}"
    );

    // No failure line. EDS logs the message only on the failure branch. Thus
    // the line always means that the store had no secret for the source, not
    // that the source is unknown or stale.
    assert!(
        !stdout.contains(&format!("Failed to lookup password for source {child_uid}")),
        "the registry logged a lookup failure for a source whose password it \
         had just been given\n{report}"
    );
}
