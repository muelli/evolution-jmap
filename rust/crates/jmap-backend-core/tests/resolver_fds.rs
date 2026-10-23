// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The fd count of `/proc/self/fd` is for the whole process. Thus this test has
// its own binary: a lookup in a parallel test would change the count.

use jmap_backend_core::resolver::SystemResolver;
use jmap_client::resolver::Resolver;

/// This prevents GWakeup fd exhaustion from contexts that never iterate.
#[test]
fn open_file_descriptors_do_not_grow_across_repeated_lookups() {
    fn count_open_fds() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .expect("reading /proc/self/fd succeeds")
            .count()
    }

    // Do one lookup first to start the resolver thread and do the one-time
    // process allocations.
    assert_eq!(
        SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
        None
    );

    let fds_before = count_open_fds();

    for _ in 0..256 {
        assert_eq!(
            SystemResolver.lookup_srv("no-jmap-here.evolution-jmap-test.invalid"),
            None
        );
    }

    let fds_after = count_open_fds();

    assert!(
        fds_after <= fds_before,
        "fd count grew from {fds_before} to {fds_after} across 256 lookups"
    );
}
