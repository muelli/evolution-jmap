// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The headless answer to the operator's 2026-08-29 observation —
//! `evolution-source-registry` logging
//!
//! ```text
//! server_side_source_credentials_lookup_cb: Failed to lookup password for
//! source d06b56726065b3531de05498dd3519c2bd6efcb3
//! ```
//!
//! twice, followed in the same second by an interactive `authorization_code`
//! consent, for a UID absent from `~/.config/evolution/sources/` whose only
//! trace anywhere on the machine was inside `~/.config/dconf/user`.
//!
//! The item asks two things. This file answers the first with one test and the
//! second with a pair, because the second answer is only worth anything if the
//! measurement can also come out the other way.
//!
//! # 1. Can dconf debris drive it?
//!
//! No, and the reason is structural rather than incidental. Read from EDS
//! 3.52.3: `server_side_source_credentials_lookup_cb` (`e-server-side-source.c:
//! 418`) runs on a live `EServerSideSource` GObject that the callback holds a
//! reference to, and a live one exists only where a `.source` keyfile was
//! loaded. dconf holds no source data at all — the only dconf-linked
//! source-UID mechanism EDS ships is the `org.gnome.Evolution.DefaultSources`
//! schema (`e-source-registry.c:68`), six keys that hold nothing but a UID
//! *reference*, and whose getters resolve it with `e_source_registry_ref_source`
//! and fall back when that is NULL (`e_source_registry_ref_default_mail_account`,
//! line 3300). So the first test plants exactly the operator's debris — a UID
//! written into all six keys, verified present in the session's own
//! `dconf/user` file and nowhere else — and pins that the registry neither
//! gains a source for it nor answers it from any default-source getter.
//!
//! # 2. Then what produced that log line?
//!
//! A perfectly ordinary child source. `collection_backend_new_user_file`
//! (`e-collection-backend.c:176-200`) writes a collection backend's children
//! to `$XDG_CACHE_HOME/evolution/sources/<collection-uid>/`, never to the
//! config directory: **being absent from `~/.config/evolution/sources/` is the
//! normal condition of every address book and calendar Evolution has ever
//! fanned an account out into**, and is no evidence of staleness whatsoever.
//! The second test runs this project's own collection backend against the mock
//! server, takes a child it produced, and drives the operator's exact path on
//! it.
//!
//! # Why the escalation is EDS working as designed
//!
//! `server_side_source_invoke_credentials_required_cb` (`e-server-side-source.c:
//! 252`) does not forward a `required` request to clients. It sets
//! `skip_emit = TRUE` (line 318) and starts a silent credentials lookup;
//! only the callback decides what the user sees. Failure re-emits
//! `credentials-required` — the prompter, and for an OAuth2-method source the
//! consent window — while success emits `authenticate` and nobody is asked.
//! The operator's two lines are therefore ONE code path on ONE live source:
//! not a lookup failure and then a separate spurious consent, but a lookup
//! failure whose designed consequence is the consent.
//!
//! That is what makes the third test a control rather than a nicety: the same
//! source, the same request, differing only in whether a password is in the
//! store, must produce the opposite pair of signals. Without it a run could
//! not tell a real escalation from the registry echoing back the request this
//! harness itself made.

use jmap_functional::{Session, observations, required_path};

/// A UID of the operator's shape — 40 lowercase hex characters, what
/// `e_util_generate_uid()` produces — that names nothing on disk.
const DANGLING_UID: &str = "d06b56726065b3531de05498dd3519c2bd6efcb3";

/// The collection account both halves of the second question fan out.
const ACCOUNT_UID: &str = "jmap-functional-stale-source-uid";

/// The account the collection tests fan out, with the mock's ephemeral port
/// filled in. The same literal `collection.rs` uses, for the same reason: a
/// change to the documented recipe should fail loudly rather than quietly
/// retarget the test.
fn keyfile(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP item-26 reproduction account\n\
         Enabled=true\n\
         \n\
         [Collection]\n\
         BackendName=jmap\n\
         ContactsEnabled=true\n\
         CalendarEnabled=true\n\
         MailEnabled=false\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

#[test]
fn a_uid_reachable_only_from_dconf_never_becomes_a_source_and_is_never_looked_up() {
    let client = required_path("JMAP_FUNCTIONAL_STALE_SOURCE_UID_CLIENT");

    let mut session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/stale-source-uid-dconf"
    ));
    // EDS's own diagnostics, so that the operator's line would be visible here
    // if anything produced it. Its absence is an assertion below.
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

    // (1) The precondition, and the thing that makes the rest non-vacuous: the
    // debris really is debris of the operator's kind — a UID that now lives in
    // this session's own `~/.config/dconf/user` and nowhere else. A GSettings
    // that fell back to the memory backend (no dconf GIO module, or
    // `ca.desrt.dconf` not activatable on the private bus) would leave the
    // rest of this test asserting things about a value no daemon could ever
    // have seen.
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
    // And nowhere else: exactly the check the operator ran by hand.
    let sources = session.sources_directory();
    assert!(
        !sources.join(format!("{DANGLING_UID}.source")).exists(),
        "the dangling UID has a keyfile in {sources:?}, which would make it an \
         ordinary configured source and not debris at all\n{report}"
    );

    // (2) The answer. `e_source_registry_ref_source` is a lookup over the
    // sources the registry actually built from files; nothing in EDS turns a
    // dconf UID into one.
    assert_eq!(
        seen.get("dangling-uid-in-registry"),
        Some(&"0"),
        "the registry has a source for a UID that exists only in dconf. That \
         would be the mechanism item 26 asks about, and it would mean EDS \
         fabricates sources from GSettings\n{report}"
    );
    let default_mail_account = seen
        .get("default-mail-account-uid")
        .unwrap_or_else(|| panic!("the client reported no default mail account\n{report}"));
    assert_ne!(
        *default_mail_account, DANGLING_UID,
        "e_source_registry_ref_default_mail_account answered the dangling UID \
         instead of falling back to the built-in account\n{report}"
    );

    // (3) And the registry's own source list never grows to include it, over
    // the same settling window the escalation test gives it. Checked from the
    // list rather than only from `ref_source` so that a source appearing late
    // — after some deferred read of the setting — would be caught too.
    for key in ["source-uids", "source-uids-after-settling"] {
        let uids = seen
            .get(key)
            .unwrap_or_else(|| panic!("the client reported no {key}\n{report}"));
        assert!(
            !uids.split(',').any(|uid| uid == DANGLING_UID),
            "{key} contains the dangling UID\n{report}"
        );
    }

    // (4) The operator's log line itself never appears. This is the assertion
    // the item's "if it turns out to be harmless noise, prove that" asks for,
    // and it is only meaningful because ESR_DEBUG=1 is set above — the same
    // switch that made the line visible on the operator's machine.
    assert!(
        !stdout.contains("Failed to lookup password for source"),
        "a credentials lookup ran, and failed, in a session whose only source \
         UID debris is in dconf\n{report}"
    );
}

/// Run the collection account, wait for a child, and hand back the child's
/// UID plus everything the run said — shared by the two halves of the second
/// question, which differ only in whether a password is in the store.
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

    // The half of the answer that is about *files*, and the reason this
    // helper takes the session apart rather than trusting the client: the
    // child the registry just handed out has no keyfile where the operator
    // looked, and does have one where EDS actually puts children.
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
         configured source' really would be evidence of debris and item 26's \
         premise needs revisiting\n{report}"
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

    // The escalation. Exactly one, and no `authenticate`: the registry tried
    // the store silently, found nothing, and put the prompter in front of the
    // user. `skip_emit = TRUE` is what makes this count the escalation rather
    // than an echo of the request the client made — see this file's header.
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

    // The second reason the operator's UID looked unfamiliar, and the sharper
    // one: the message names the source the request was ABOUT, not the source
    // the secret store was searched under. For a collection child those are
    // never the same — `source_credential_provider_ref_impl_for_source`
    // (`e-source-credentials-provider.c:216-237`) resolves the credentials
    // source first, and it is the collection. So a `Failed to lookup password
    // for source <uid>` line is not even a statement about a keyring entry
    // under `<uid>`.
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

    // And the operator's own line, for the operator's own UID shape, naming a
    // source that has no keyfile in the config directory — which is the whole
    // of item 26's puzzle, reproduced from an account set up thirty seconds
    // earlier rather than from debris.
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
    // The control. Everything is identical to the test above except that the
    // client stores a password for the child before asking — so if the counts
    // did not move, the pair above would be measuring the harness rather than
    // the registry's decision.
    let (child_uid, stdout, report) =
        run_collection_child("stale-source-uid-child-stored", Some("item-26-secret"));
    let seen = observations(&stdout);

    assert_eq!(
        seen.get("password-stored"),
        Some(&"1"),
        "the client did not take the stored-password path\n{report}"
    );
    // Where the password had to go, which is not where the log line points.
    // See `child-uid` vs `credentials-source-uid` in the test above.
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

    // The corollary an operator can act on: no failure line either. The log
    // message item 26 starts from is emitted only on the failing branch, so
    // seeing it always means the store had nothing for that source — never
    // that the source is unknown or stale.
    assert!(
        !stdout.contains(&format!("Failed to lookup password for source {child_uid}")),
        "the registry logged a lookup failure for a source whose password it \
         had just been given\n{report}"
    );
}
