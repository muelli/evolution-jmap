// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stress-proves that concurrent
//! captures on different threads, racing to touch the same callsite for the
//! first time in this process, never lose or cross-deliver an event.

use std::thread;

use jmap_tracing_capture::capture;

fn emit(id: usize) {
    tracing::debug!(marker = "isolation-probe", id = id);
}

fn ids_field(captured: &[(tracing::Level, String, String)]) -> Vec<String> {
    captured
        .iter()
        .filter(|(_, name, _)| name == "id")
        .map(|(_, _, value)| value.clone())
        .collect()
}

#[test]
fn concurrent_captures_never_see_another_thread_s_event() {
    for _ in 0..2_000 {
        let handles: Vec<_> = (0..8)
            .map(|id| thread::spawn(move || capture(|| emit(id)).1))
            .collect();

        for (id, handle) in handles.into_iter().enumerate() {
            let captured = handle.join().unwrap();
            assert_eq!(
                ids_field(&captured),
                vec![id.to_string()],
                "thread {id} expected exactly its own event, got {captured:?}"
            );
        }
    }
}

#[test]
fn capturing_is_unaffected_by_concurrent_non_capturing_calls() {
    for round in 0..1_000 {
        let capturing = thread::spawn(move || capture(|| emit(round)).1);
        let noisy: Vec<_> = (0..4)
            .map(|n| thread::spawn(move || drop(capture(|| emit(1_000_000 + n)))))
            .collect();

        let captured = capturing.join().unwrap();
        for handle in noisy {
            handle.join().unwrap();
        }

        assert_eq!(
            ids_field(&captured),
            vec![round.to_string()],
            "got {captured:?}"
        );
    }
}

#[test]
fn a_nested_capture_hands_the_scope_back_to_the_outer_one() {
    let ((), outer) = capture(|| {
        emit(1);
        let ((), inner) = capture(|| emit(2));
        assert_eq!(ids_field(&inner), ["2"]);
        emit(3);
    });
    assert_eq!(ids_field(&outer), ["1", "3"]);
}
