// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the info bar after a collection delete: when it shows, its text and its
// time, and what Undo changes on a real `ESource`. No test makes a widget (see
// `evo-sys/tests/gtk.rs` for the reason).

use std::ffi::CString;
use std::ptr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, e_source_get_enabled, e_source_get_extension, e_source_new_with_uid,
    e_source_resource_set_identity, e_source_set_display_name, e_source_set_enabled,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::g_object_unref;
use jmap_collection_sync::ChildKind;
use jmap_collection_sync::pending::Due;
use jmap_ui::undo_delete::notice::{Deleted, undo, view_kind};

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

const NOW: u64 = 1_800_000_000;

fn deleted(kind: ChildKind, identity: &str) -> Deleted {
    Deleted {
        kind,
        identity: identity.to_owned(),
        display_name: "Team".to_owned(),
        enabled: false,
        due: Due::from_stamp(NOW as i64 + 30),
    }
}

#[test]
fn a_pending_delete_of_an_own_calendar_shows_the_time_left() {
    let notice = deleted(ChildKind::Calendar, "cal1")
        .notice(at(NOW))
        .expect("a pending delete shows an info bar");
    assert_eq!(notice.kind, ChildKind::Calendar);
    assert_eq!(notice.seconds, 30);
    assert_eq!(notice.primary, "Calendar \"Team\" is deleted.");
    assert_eq!(
        notice.secondary,
        "The calendar is deleted on the server. To keep it, click Undo before this message closes."
    );
}

#[test]
fn a_shared_calendar_is_deleted_for_all_users_who_share_it() {
    let notice = deleted(ChildKind::Calendar, "owner-account/cal1")
        .notice(at(NOW))
        .expect("a pending delete shows an info bar");
    assert_eq!(
        notice.secondary,
        "The calendar is deleted for all users who share it. To keep it, click Undo before this message closes."
    );
}

#[test]
fn an_address_book_has_its_own_text() {
    let own = deleted(ChildKind::AddressBook, "ab1")
        .notice(at(NOW))
        .expect("a pending delete shows an info bar");
    assert_eq!(own.primary, "Address book \"Team\" is deleted.");
    assert_eq!(
        own.secondary,
        "The address book is deleted on the server. To keep it, click Undo before this message closes."
    );
    let shared = deleted(ChildKind::AddressBook, "owner-account/ab1")
        .notice(at(NOW))
        .expect("a pending delete shows an info bar");
    assert_eq!(
        shared.secondary,
        "The address book is deleted for all users who share it. To keep it, click Undo before this message closes."
    );
}

#[test]
fn a_part_second_rounds_up_so_the_bar_never_closes_before_the_due_time() {
    let notice = deleted(ChildKind::Calendar, "cal1")
        .notice(at(NOW) + Duration::from_millis(100))
        .expect("a pending delete shows an info bar");
    assert_eq!(notice.seconds, 30);
}

#[test]
fn no_info_bar_shows_for_an_undone_a_due_or_a_plain_source() {
    let mut undone = deleted(ChildKind::Calendar, "cal1");
    undone.enabled = true;
    assert_eq!(undone.notice(at(NOW)), None, "an enabled source is undone");

    let due = deleted(ChildKind::Calendar, "cal1");
    assert_eq!(
        due.notice(at(NOW + 30)),
        None,
        "at the due time the destroy goes out"
    );

    let mut plain = deleted(ChildKind::Calendar, "cal1");
    plain.due = None;
    assert_eq!(
        plain.notice(at(NOW)),
        None,
        "a disabled source with no due time"
    );
}

#[test]
fn only_the_calendar_and_the_contacts_views_show_an_info_bar() {
    assert_eq!(view_kind("calendar"), Some(ChildKind::Calendar));
    assert_eq!(view_kind("addressbook"), Some(ChildKind::AddressBook));
    for other in ["mail", "tasks", "memos", ""] {
        assert_eq!(view_kind(other), None, "{other:?}");
    }
}

struct TestSource(*mut ESource);

impl TestSource {
    fn new(extension: Option<&std::ffi::CStr>) -> Self {
        let uid = CString::new("jmap-child").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        if let Some(extension) = extension {
            // SAFETY: a live source and a registered extension name.
            unsafe { e_source_get_extension(source, extension.as_ptr()) };
        }
        Self(source)
    }

    fn pending(self, identity: &str, name: &str, due: Due) -> Self {
        let identity = CString::new(identity).expect("no NUL");
        let name = CString::new(name).expect("no NUL");
        // SAFETY: a live source, NUL-terminated strings, and the resource
        // extension that `e_source_get_extension` returns.
        unsafe {
            let resource = e_source_get_extension(self.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr());
            e_source_resource_set_identity(resource.cast(), identity.as_ptr());
            e_source_set_display_name(self.0, name.as_ptr());
            jmap_config::pending_delete::write(self.0, Some(due));
            e_source_set_enabled(self.0, GFALSE);
        }
        self
    }

    fn read(&self) -> Option<Deleted> {
        // SAFETY: a live source.
        unsafe { Deleted::read(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: the reference that `new` took.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

#[test]
fn a_source_is_read_with_its_kind_identity_name_and_due_time() {
    let due = Due::from_stamp(NOW as i64 + 30).expect("a positive stamp");
    let source =
        TestSource::new(Some(E_SOURCE_EXTENSION_CALENDAR)).pending("acct/cal1", "Team", due);
    assert_eq!(
        source.read(),
        Some(Deleted {
            kind: ChildKind::Calendar,
            identity: "acct/cal1".to_owned(),
            display_name: "Team".to_owned(),
            enabled: false,
            due: Some(due),
        })
    );

    let book = TestSource::new(Some(E_SOURCE_EXTENSION_ADDRESS_BOOK)).pending("ab1", "Team", due);
    assert_eq!(
        book.read().map(|deleted| deleted.kind),
        Some(ChildKind::AddressBook)
    );
}

#[test]
fn a_source_that_is_not_a_collection_is_not_read() {
    assert_eq!(TestSource::new(None).read(), None);
}

#[test]
fn undo_enables_the_source_and_clears_the_due_time() {
    let due = Due::from_stamp(NOW as i64 + 30).expect("a positive stamp");
    let source = TestSource::new(Some(E_SOURCE_EXTENSION_CALENDAR)).pending("cal1", "Team", due);
    // SAFETY: a live source.
    unsafe { undo(source.0) };
    // SAFETY: a live source.
    assert_eq!(unsafe { e_source_get_enabled(source.0) }, GTRUE);
    let read = source.read().expect("still a calendar");
    assert_eq!(read.due, None);
    assert_eq!(read.notice(at(NOW)), None);
}
