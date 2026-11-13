// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Item 171b's error underline needs a real `GtkTextIter`, the one struct
// this crate gives a real layout instead of a zero-sized handle (see
// build.rs). A wrong field count or a wrong field type here would not fail
// to compile: GTK would write past the end of a Rust-allocated struct that
// is smaller than the real one, corrupting whatever the stack holds next.
// This file drives every iter call this crate offers against a real
// `GtkTextBuffer`, a plain GObject rather than a widget, so it needs no
// display connection, unlike tests/gtk.rs's `GtkWidget` classes. A wrong
// layout would make at least one assertion below fail, or crash the test
// process outright, rather than pass silently.

use std::ffi::{CString, c_char};
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use evo_sys::*;
use gobject_sys::{g_object_set, g_object_unref};

/// GTK 3's hand-written `get_type` once-guards are not thread-safe, the
/// same reason tests/gtk.rs and tests/layout.rs serialize their own first
/// touch of the type system. Each test below is the first in its thread to
/// register `GtkTextBuffer` or `GtkTextTag`; running two at once corrupted
/// the heap here (`malloc(): unaligned tcache chunk detected`), not a
/// `GtkTextIter` layout bug. Every test takes this lock first.
static TYPE_SYSTEM: Mutex<()> = Mutex::new(());

fn type_lock() -> MutexGuard<'static, ()> {
    TYPE_SYSTEM
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A fresh buffer holding `text`, with its own new tag table.
fn buffer_with_text(text: &str) -> *mut GtkTextBuffer {
    // SAFETY: `NULL` asks for a new, private tag table. The "text" property
    // copies the string; no reference to `text` outlives this call.
    unsafe {
        let buffer = gtk_text_buffer_new(ptr::null_mut());
        let text = CString::new(text).expect("no interior NUL");
        g_object_set(
            buffer.cast(),
            c"text".as_ptr(),
            text.as_ptr(),
            ptr::null::<c_char>(),
        );
        buffer
    }
}

#[test]
fn the_start_and_end_iters_bound_the_text() {
    let _lock = type_lock();
    const TEXT: &str = "one\ntwo\nthree\n";
    // SAFETY: `buffer` is a fresh, live `GtkTextBuffer`; both iters are
    // local and filled before being read.
    unsafe {
        let buffer = buffer_with_text(TEXT);
        let mut start = GtkTextIter::default();
        let mut end = GtkTextIter::default();
        gtk_text_buffer_get_start_iter(buffer, &mut start);
        gtk_text_buffer_get_end_iter(buffer, &mut end);
        assert_eq!(
            gtk_text_iter_get_offset(&start),
            0,
            "the start iter is not at offset 0"
        );
        assert_eq!(
            gtk_text_iter_get_offset(&end),
            TEXT.chars().count() as i32,
            "the end iter is not after the last character"
        );
        g_object_unref(buffer.cast());
    }
}

#[test]
fn an_iter_at_a_line_reports_that_line_back() {
    let _lock = type_lock();
    // SAFETY: as above.
    unsafe {
        let buffer = buffer_with_text("one\ntwo\nthree\n");
        let mut second = GtkTextIter::default();
        gtk_text_buffer_get_iter_at_line(buffer, &mut second, 1);
        assert_eq!(
            gtk_text_iter_get_line(&second),
            1,
            "an iter at line 1 does not report line 1 back"
        );
        let mut end_of_line = second;
        gtk_text_iter_forward_to_line_end(&mut end_of_line);
        assert_eq!(
            gtk_text_iter_get_line(&end_of_line),
            1,
            "forward_to_line_end moved to a different line"
        );
        assert!(
            gtk_text_iter_get_offset(&end_of_line) > gtk_text_iter_get_offset(&second),
            "forward_to_line_end did not move forward"
        );
        g_object_unref(buffer.cast());
    }
}

#[test]
fn a_tag_applies_over_a_line_and_is_removed_again() {
    let _lock = type_lock();
    // SAFETY: `tag` is new and this call adds the table's only reference;
    // the buffer and both iters are live for the whole call.
    unsafe {
        let buffer = buffer_with_text("one\ntwo\nthree\n");
        let tag = gtk_text_tag_new(ptr::null());
        let table = gtk_text_buffer_get_tag_table(buffer);
        assert!(!table.is_null(), "a fresh buffer has no tag table");
        assert_ne!(
            gtk_text_tag_table_add(table, tag),
            GFALSE,
            "the tag was not added to the table"
        );

        let mut line_start = GtkTextIter::default();
        gtk_text_buffer_get_iter_at_line(buffer, &mut line_start, 1);
        let mut line_end = line_start;
        gtk_text_iter_forward_to_line_end(&mut line_end);

        gtk_text_buffer_apply_tag(buffer, tag, &line_start, &line_end);
        gtk_text_buffer_remove_tag(buffer, tag, &line_start, &line_end);

        g_object_unref(buffer.cast());
    }
}
