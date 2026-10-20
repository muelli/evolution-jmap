// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests that the question "is this method OAuth 2.0?" from several threads at
// the same time does not crash the process.
//
// `EOAuth2Services` is a process-wide singleton. Its *last* reference destroys
// it (`e-oauth2-services.c`, EDS 3.52.3).
// `e_oauth2_services_is_oauth2_alias_static()` creates one, queries it and
// drops that reference on each call. Thus two threads that ask at the same time
// cause a race in EDS's own code. One thread is in `oauth2_services_dispose`,
// which does `g_slist_free_full (priv->services, g_object_unref)` *without
// clearing the field*. The other thread is in `oauth2_services_constructor`. It
// still sees the `services_singleton` that is not yet cleared and takes a
// reference to it. This is a legal GObject resurrection of an object whose
// service list is already freed. The second thread then walks that freed list
// and dereferences a dangling `EOAuth2Service`.
//
// EDS's documentation for `e_oauth2_services_is_oauth2_alias_static()` names
// the precondition that prevents this: the singleton "won't be much trouble, as
// long as there is something else having created one instance." Every real
// Evolution process satisfies it by accident: `e_source_registry_init()` holds
// an `EOAuth2Services` for the registry's lifetime. Thus the crash occurs only
// where nothing holds one. This project's test binaries are such a place. Any
// future tool that queries before it opens a registry is another.
// `jmap_backend_core::oauth2` therefore satisfies the precondition
// intentionally, and this test makes sure that it stays so.
//
// This test is a separate binary, not a case in `tests/oauth2.rs`. Thus the
// threads below are the only code in the process that touch the singleton.
// This makes a failing run fail every time, not only sometimes.

use std::sync::{Arc, Barrier};
use std::thread;

use jmap_backend_core::oauth2::method_is_oauth2;

/// A name that no `EOAuth2Service` matches. Thus the lookup walks the full
/// service list each time and does not stop at the first entry.
/// `tests/oauth2.rs` uses an unregistrable name for the same reason. Here a
/// full walk also gives the widest window onto a list that another thread
/// frees.
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
/// lookup reads the service list, so this is not a copy of the test above. It
/// makes sure that the fix does not make the fast path slower for the safety
/// of the slow path. Every account that this project's setup UI writes needs
/// this answer.
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
