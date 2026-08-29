// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `docs/ROADMAP.md` item 22, Do(1): the headless reproduction of the stale
//! `Source.OAuth2Support` interface proxy.
//!
//! What the item-20 tracing captured live on 2026-08-28 was a calendar factory
//! asking for an access token and getting `"The name :1.4 was not provided by
//! any .service files"`, which then escalated into a consent window. Sessions
//! N+85 and N+86 established the mechanism from GLib/EDS source and N+86 fixed
//! the classification (`jmap_backend_core::oauth2::is_service_gone`), but
//! nothing had ever *run* the failure: what existed was a hand `dbus-send` to
//! a made-up unique name, which proves D-Bus's semantics rather than EDS's
//! behaviour. This test runs it against real daemons.
//!
//! The whole reproduction is one client program (`tests/functional/
//! oauth2-stale-proxy-client.c`, whose header carries the source citations)
//! standing in for a long-lived backend factory: hold an `ESource`, kill the
//! registry, ask the same `ESource` for a token.
//!
//! **The load-bearing assertion is `oauth2-support-exported`.** Session N+87
//! concluded the opposite — that for our accounts the entire token path is
//! D-Bus-free, so any harness of this shape would pass vacuously — on the
//! premise that `e_server_side_source_set_oauth2_support` is called only by
//! `module-google-backend.c` and `module-gnome-online-accounts.c`. There is a
//! third caller: `module-oauth2-services.c:139`, an EDS module inside
//! `evolution-source-registry`, which calls it for *every* server-side source
//! whose `[Authentication] Method` names a registered `EOAuth2Service` — ours
//! being `jmap_config::oauth2_service::NAME`, `"JMAP"`. This test is what
//! keeps that correction honest: if the export ever stops happening, the rest
//! of the reproduction would still "pass" while measuring nothing, so the
//! export is asserted first and by itself.

use jmap_functional::{Session, observations, required_path};

/// A collection account whose `[Authentication] Method` is our own
/// `EOAuth2Service`'s name.
///
/// `Method=JMAP` is the entire point of this keyfile and the one line that
/// must not drift: it is what `e_oauth2_services_is_oauth2_alias` matches
/// against the service `module-jmap-backend.so` registers, and so what makes
/// `module-oauth2-services.so` export the D-Bus interface this test is about.
/// A `Method=` naming anything else produces a source with no OAuth2Support
/// interface, no proxy, and nothing to go stale.
///
/// No `[Security] Method=none` and no reachable server: this test never
/// completes a token fetch or talks to a JMAP server at all — it only needs
/// the registry to have *built* the source and exported the interface.
fn keyfile() -> String {
    "[Data Source]\n\
     DisplayName=JMAP stale-proxy reproduction account\n\
     Enabled=true\n\
     \n\
     [Collection]\n\
     BackendName=jmap\n\
     ContactsEnabled=false\n\
     CalendarEnabled=false\n\
     MailEnabled=false\n\
     \n\
     [Authentication]\n\
     Host=127.0.0.1\n\
     Port=1\n\
     User=jmap-stale-proxy\n\
     Method=JMAP\n"
        .to_owned()
}

#[test]
fn a_registry_restart_leaves_a_held_source_fetching_tokens_from_a_dead_peer() {
    let client = required_path("JMAP_FUNCTIONAL_OAUTH2_STALE_PROXY_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_COLLECTION_MODULE");
    let oauth2_services = required_path("JMAP_FUNCTIONAL_EDS_OAUTH2_SERVICES_MODULE");

    const ACCOUNT_UID: &str = "jmap-functional-oauth2-stale-proxy";
    let mut session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/oauth2-stale-proxy"));
    session.write_source(ACCOUNT_UID, &keyfile());
    session.stage_collection_backend(&module);
    // Must come after the line above: both write EDS_REGISTRY_MODULES, and
    // this one adds to the directory that one created.
    session.stage_installed_registry_module(&oauth2_services);

    let output = session.run(&client, &[ACCOUNT_UID]);
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
        seen.get("source-found"),
        Some(&"1"),
        "the registry never saw the account keyfile at all\n{report}"
    );

    // (1) The precondition, and the fact N+87 doubted: our account really is
    // one the registry exports Source.OAuth2Support for, so a client's token
    // fetch goes over the bus instead of through e-source.c's in-process
    // EOAuth2Services fallback. Everything below is vacuous without this.
    assert_eq!(
        seen.get("oauth2-support-exported"),
        Some(&"1"),
        "the registry did not export Source.OAuth2Support for an account whose \
         [Authentication] Method names our own EOAuth2Service. Either \
         module-oauth2-services.so was not loaded beside module-jmap-backend.so, \
         or our service is no longer registered under the name the keyfile uses \
         — in both cases a client's token fetch takes e-source.c's in-process \
         fallback and item 22's failure cannot arise at all\n{report}"
    );
    assert_eq!(
        seen.get("oauth2-support-peer-is-unique-name"),
        Some(&"1"),
        "the interface proxy is not addressed to a unique name, so it would \
         survive a restart by re-activation and item 22's mechanism (a proxy \
         pinned to a name that can never come back) would not apply\n{report}"
    );

    // (2) The token path was live before the kill. Reported as "not a bus
    // error" rather than "succeeded": this scratch session holds no refresh
    // token, so the fetch legitimately fails — but it must fail *inside* our
    // EOAuth2Service, having reached a living registry, not in transport.
    assert_eq!(
        seen.get("token-before-kill-was-bus-error"),
        Some(&"0"),
        "the token fetch was already failing at the D-Bus layer before the \
         registry was killed, so what this test measures afterwards is not the \
         restart's doing\n{report}"
    );

    assert_eq!(
        seen.get("registry-name-gone"),
        Some(&"1"),
        "the registry's unique name still has an owner after SIGKILL\n{report}"
    );

    // (3) The reproduction. The ESource is the same object the client held
    // before the kill, and it still carries the proxy — EDS's client-side
    // `source_registry_object_removed_no_owner` deliberately does not strip
    // it (only the *by_owner* branch calls
    // `__e_source_private_replace_dbus_object(source, NULL)`), which is why
    // this is deterministic rather than a race.
    assert_eq!(
        seen.get("oauth2-support-still-exported"),
        Some(&"1"),
        "the client dropped the interface when the registry died, which would \
         mean EDS recovers on its own and item 22 has no client-side bug\n{report}"
    );
    assert_eq!(
        seen.get("token-after-kill-succeeded"),
        Some(&"0"),
        "the token fetch SUCCEEDED after the registry was killed — item 22's \
         failure no longer reproduces, and this test has become a museum \
         piece. Check whether EDS grew a recovery path before deleting it\n{report}"
    );

    // (4) And it fails as exactly the error the live capture recorded, in
    // exactly the shape jmap_backend_core::oauth2 classifies. `is_service_gone`
    // keys on domain `g-dbus-error-quark` with G_DBUS_ERROR_SERVICE_UNKNOWN or
    // G_DBUS_ERROR_NAME_HAS_NO_OWNER; its own unit tests build that error by
    // hand, and this is the run that proves real EDS produces it.
    assert_eq!(
        seen.get("token-after-kill-is-service-unknown"),
        Some(&"1"),
        "the failure was not G_DBUS_ERROR_SERVICE_UNKNOWN, so it is not the \
         one jmap_backend_core::oauth2::is_service_gone carves out of item \
         17's blanket secret-store classification\n{report}"
    );
    assert_eq!(
        seen.get("token-after-kill-error-domain"),
        Some(&"g-dbus-error-quark"),
        "the error is not in the domain is_service_gone matches on\n{report}"
    );
    // The domain matters more than the code, and that is the point: 2 is an
    // entirely ordinary code in other domains too, which is why
    // `is_service_gone` matches on both and why
    // `a_dead_peer_is_recognised_by_domain_not_by_code_alone` (jmap-backend-
    // core/src/oauth2.rs) pins the same pair from the other side. This
    // assertion is what would catch the numeric value drifting under it.
    assert_eq!(
        seen.get("token-after-kill-error-code"),
        Some(&"2"),
        "G_DBUS_ERROR_SERVICE_UNKNOWN is not 2 on this GLib, which is the \
         value jmap_backend_core::oauth2's own tests document it as\n{report}"
    );

    // The message is the user-visible half of N+86's fix: it must name the
    // dead peer, because that is the thing an operator would go and restart.
    assert_eq!(
        seen.get("token-after-kill-names-dead-peer"),
        Some(&"1"),
        "the error message does not name the peer that vanished\n{report}"
    );
    let message = seen
        .get("token-after-kill-error-message")
        .unwrap_or_else(|| panic!("the client reported no error message\n{report}"));
    assert!(
        message.contains("was not provided by any .service files"),
        "the error message is not the one the live capture recorded; it was \
         {message:?}\n{report}"
    );
}
