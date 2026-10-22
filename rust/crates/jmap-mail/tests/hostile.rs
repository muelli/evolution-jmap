// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Claims about this crate's instance structs that the compiler never checks.
//!
//! `jmap-backend-book` and `jmap-backend-cal` each carry one of these structs.
//! Each one pins the threading claim behind `Slot` as a compile error, not a
//! comment. The mail provider has more instance structs than either backend.
//!
//! Camel is a harder case than EDS, not an easier one. A `CamelStore` runs on
//! more threads than a meta backend. Unlike the two backends, this crate also
//! carries a hand-written `unsafe impl Send`/`Sync`. Its justification holds
//! only while nothing else can reach the pointer it guards.

use std::sync::{Arc, Mutex, RwLock};

use jmap_backend_core::instance::Slot;
use jmap_backend_core::push::PushRefresh;
use jmap_mail::cache::MessageCache;
use jmap_mail::push::FolderRefresh;
use jmap_mail::summary::Recorded;
use jmap_mail_sync::{FolderTree, Keywords, MailSync};
use jmap_proto::{Id, State};

/// Everything a `Slot` in this crate holds, and everything inside it.
///
/// A vfunc receives the instance as a raw pointer and converts it to a `&` by
/// hand, so the compiler never sees the sharing. Nothing would stop
/// `MailSync`, the `Client` inside it, or the boxed `Transport` inside that,
/// from gaining an `Rc` or a `RefCell`. The result would be a data race in the
/// process serving every other mail account of the user.
///
/// One test covers every field, instead of one test per struct, because all
/// fields make one claim. The test must name all of them to stay that claim.
/// A `Slot` added to the crate without a line here is a struct the compiler
/// never checks.
#[test]
fn everything_an_instance_holds_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<MailSync>();
    assert_send_sync::<jmap_client::Client>();

    // `crate::transport`: the connection a send goes out over.
    assert_send_sync::<Slot<RwLock<Option<MailSync>>>>();
    // `crate::store`: the connection (behind an `Arc`, unlike the
    // transport's), the listing a refresh renews, the push subscription, and
    // the coalescing folder-refresh worker.
    // `Listing` is private to the crate, so its two fields stand in for it
    // here.
    assert_send_sync::<Slot<RwLock<Option<Arc<MailSync>>>>>();
    assert_send_sync::<State>();
    assert_send_sync::<Arc<FolderTree>>();
    assert_send_sync::<Slot<Mutex<Option<PushRefresh>>>>();
    assert_send_sync::<Slot<Mutex<Option<FolderRefresh>>>>();
    // `crate::folder`: the mailbox this folder represents, and the message
    // cache that this folder owns for the account.
    assert_send_sync::<Slot<Id>>();
    assert_send_sync::<Slot<MessageCache>>();
    // `crate::summary`: the `Email` state and the origin of the rows.
    assert_send_sync::<Slot<Mutex<Recorded>>>();
    // `crate::message_info`: the keywords the last listing found, and the
    // thread id the server assigned the row.
    assert_send_sync::<Slot<Mutex<Keywords>>>();
    assert_send_sync::<Slot<Mutex<Option<Id>>>>();
}

/// The one hand-written `unsafe impl` in this crate, tested where it is
/// claimed.
///
/// [`MessageCache`] wraps a `*mut CamelDataCache`, which is not `Send` or
/// `Sync` on its own, behind a `Mutex`. It claims both traits in an `unsafe
/// impl`. This claim holds only while the pointer never leaves the lock, a
/// property of the module, not of the type. Only this test would notice if
/// an accessor began handing out the pointer.
#[test]
fn the_message_cache_still_claims_to_be_shareable() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<MessageCache>();
}
