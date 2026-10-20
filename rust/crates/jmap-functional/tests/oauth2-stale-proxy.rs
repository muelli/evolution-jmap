// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The headless reproduction of the stale `Source.OAuth2Support` interface
//! proxy.
//!
//! A calendar factory that asked for an access token got `"The name :1.4 was
//! not provided by any .service files"`. This error then caused a consent
//! window. The fix is the classification in
//! `jmap_backend_core::oauth2::is_service_gone`. This test runs the failure
//! against real daemons, not a manual `dbus-send` to an invented unique name.
//!
//! One client program (`tests/functional/oauth2-stale-proxy-client.c`, with
//! the source citations in its header) acts as a long-lived backend factory.
//! It holds an `ESource`, kills the registry, then asks the same `ESource`
//! for a token.
//!
//! **The necessary assertion is `oauth2-support-exported`.** An earlier
//! analysis said that the token path of these accounts uses no D-Bus. It
//! assumed that only `module-google-backend.c` and
//! `module-gnome-online-accounts.c` call
//! `e_server_side_source_set_oauth2_support`. There is a third caller:
//! `module-oauth2-services.c:139` in `evolution-source-registry`. It calls the
//! function for each server-side source whose `[Authentication] Method` names
//! a registered `EOAuth2Service`. The name of the service of this project is
//! `jmap_config::oauth2_service::NAME`, `"JMAP"`. If the export stops, the
//! other assertions pass but measure nothing. Thus the test asserts the
//! export first, as a separate assertion.

use jmap_functional::{Session, observations, required_path};

/// A collection account whose `[Authentication] Method` is the name of the
/// `EOAuth2Service` of this project.
///
/// Do not change `Method=JMAP`. `e_oauth2_services_is_oauth2_alias` compares
/// it with the service that `module-jmap-backend.so` registers. A match makes
/// `module-oauth2-services.so` export the D-Bus interface under test. With a
/// different `Method=`, the source has no OAuth2Support interface and no proxy.
///
/// The keyfile has no `[Security] Method=none` and no reachable server. The
/// test does not complete a token fetch or connect to a JMAP server. It only
/// needs the registry to *build* the source and export the interface.
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
    // Keep this after the line above. Both write EDS_REGISTRY_MODULES, and
    // this call adds to the directory that the previous call created.
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

    // (1) The precondition: the registry exports Source.OAuth2Support for
    // this account. Thus the token fetch goes over the bus, not through the
    // in-process EOAuth2Services fallback of e-source.c. Without this, the
    // assertions below measure nothing.
    assert_eq!(
        seen.get("oauth2-support-exported"),
        Some(&"1"),
        "the registry did not export Source.OAuth2Support for an account whose \
         [Authentication] Method names our own EOAuth2Service. Either \
         module-oauth2-services.so was not loaded beside module-jmap-backend.so, \
         or our service is no longer registered under the name the keyfile uses: \
         in both cases a client's token fetch takes e-source.c's in-process \
         fallback and this reproduction's failure cannot arise at all\n{report}"
    );
    assert_eq!(
        seen.get("oauth2-support-peer-is-unique-name"),
        Some(&"1"),
        "the interface proxy is not addressed to a unique name, so it would \
         survive a restart by re-activation and the mechanism under test (a \
         proxy pinned to a name that can never come back) would not apply\n{report}"
    );

    // (2) The token path operates before the kill. The session has no
    // refresh token, so the fetch fails. It must fail *inside* the
    // EOAuth2Service of this project after it reaches a live registry, not in
    // transport. Thus the client reports "not a bus error", not "succeeded".
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

    // (3) The reproduction. The client holds the same ESource as before the
    // kill, and it keeps the proxy. The EDS client-side
    // `source_registry_object_removed_no_owner` does not remove it. Only the
    // *by_owner* branch calls
    // `__e_source_private_replace_dbus_object(source, NULL)`. Thus the result
    // is deterministic, not a race.
    assert_eq!(
        seen.get("oauth2-support-still-exported"),
        Some(&"1"),
        "the client dropped the interface when the registry died, which would \
         mean EDS recovers on its own and this reproduction has no \
         client-side bug\n{report}"
    );
    assert_eq!(
        seen.get("token-after-kill-succeeded"),
        Some(&"0"),
        "the token fetch SUCCEEDED after the registry was killed: the failure \
         no longer reproduces, and this test has become a museum piece. \
         Check whether EDS grew a recovery path before deleting it\n{report}"
    );

    // (4) The error is the same as in the live capture, and has the shape
    // that jmap_backend_core::oauth2 classifies. `is_service_gone` matches
    // domain `g-dbus-error-quark` with G_DBUS_ERROR_SERVICE_UNKNOWN or
    // G_DBUS_ERROR_NAME_HAS_NO_OWNER. Its unit tests make that error manually.
    // This run proves that real EDS makes it.
    assert_eq!(
        seen.get("token-after-kill-is-service-unknown"),
        Some(&"1"),
        "the failure was not G_DBUS_ERROR_SERVICE_UNKNOWN, so it is not the \
         one jmap_backend_core::oauth2::is_service_gone carves out of the \
         blanket secret-store classification\n{report}"
    );
    assert_eq!(
        seen.get("token-after-kill-error-domain"),
        Some(&"g-dbus-error-quark"),
        "the error is not in the domain is_service_gone matches on\n{report}"
    );
    // The domain is more important than the code: other domains also use
    // code 2. Thus `is_service_gone` matches on both, and
    // `a_dead_peer_is_recognised_by_domain_not_by_code_alone`
    // (jmap-backend-core/src/oauth2.rs) tests the same pair. This assertion
    // finds a change of the numeric value.
    assert_eq!(
        seen.get("token-after-kill-error-code"),
        Some(&"2"),
        "G_DBUS_ERROR_SERVICE_UNKNOWN is not 2 on this GLib, which is the \
         value jmap_backend_core::oauth2's own tests document it as\n{report}"
    );

    // The message is visible to the user. It must name the dead peer,
    // because the operator must restart that peer.
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
