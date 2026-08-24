// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// That asking "is this method OAuth 2.0?" from several threads at once does not
// take the process down.
//
// `EOAuth2Services` is a process-wide singleton whose *last* reference destroys
// it (`e-oauth2-services.c`, EDS 3.52.3), and
// `e_oauth2_services_is_oauth2_alias_static()` creates one, queries it and drops
// that reference on every call. Two threads asking at once is then a race
// against EDS's own machinery: one is inside `oauth2_services_dispose`, which
// does `g_slist_free_full (priv->services, g_object_unref)` *without clearing
// the field*, while the other is inside `oauth2_services_constructor`, which
// still sees the not-yet-cleared `services_singleton` and takes a reference to
// it — a legal GObject resurrection, of an object whose service list has just
// been freed. The second thread then walks that freed list and dereferences a
// dangling `EOAuth2Service`.
//
// EDS's own documentation for `e_oauth2_services_is_oauth2_alias_static()` names
// the precondition that avoids this: the singleton "won't be much trouble, as
// long as there is something else having created one instance." Every real
// Evolution process satisfies it by accident — `e_source_registry_init()` holds
// an `EOAuth2Services` for the registry's lifetime — so the crash only shows up
// where nothing does, which is this project's own test binaries, and would show
// up in any future tool that queries before opening a registry.
// `jmap_backend_core::oauth2` therefore satisfies it on purpose; this test is
// what pins that.
//
// Written as its own binary rather than a case in `tests/oauth2.rs` so that the
// threads below are the only thing in the process touching the singleton, which
// is what makes a red run red rather than occasionally red.

use std::sync::{Arc, Barrier};
use std::thread;

use jmap_backend_core::oauth2::method_is_oauth2;

/// A name no `EOAuth2Service` can answer to, so the lookup walks the whole
/// service list every time instead of stopping at the first entry — the same
/// reason `tests/oauth2.rs` uses a deliberately unregistrable name, here also
/// because a full walk is the widest window onto a list being freed underneath
/// it.
const NO_SUCH_SERVICE: &str = "not-a-registered-oauth2-service";

const THREADS: usize = 8;
const QUERIES_PER_THREAD: usize = 400;

#[test]
fn the_alias_lookup_survives_being_asked_from_several_threads_at_once() {
    let start = Arc::new(Barrier::new(THREADS));

    let askers: Vec<_> = (0..THREADS)
        .map(|_| {
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                for _ in 0..QUERIES_PER_THREAD {
                    assert!(
                        !method_is_oauth2(Some(NO_SUCH_SERVICE)),
                        "a name no service is registered under was read as OAuth 2.0"
                    );
                }
            })
        })
        .collect();

    for asker in askers {
        asker.join().expect("an asking thread panicked");
    }
}

/// The literal spelling, asked the same way. `"OAuth2"` returns before the
/// service list is consulted at all, so this is not a second copy of the test
/// above: it pins that the fix did not make the fast path pay for the slow
/// one's safety, and it is the answer every account written by this project's
/// own setup UI depends on.
#[test]
fn the_literal_method_is_still_recognised_under_the_same_contention() {
    let start = Arc::new(Barrier::new(THREADS));

    let askers: Vec<_> = (0..THREADS)
        .map(|_| {
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                for _ in 0..QUERIES_PER_THREAD {
                    assert!(method_is_oauth2(Some("OAuth2")));
                }
            })
        })
        .collect();

    for asker in askers {
        asker.join().expect("an asking thread panicked");
    }
}
