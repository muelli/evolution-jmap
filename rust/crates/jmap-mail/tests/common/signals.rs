// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listens to a store and to a folder, the way Evolution listens. Defines the
//! main context that makes listening work.
//!
//! Camel does not emit its notifications at the point where they are reported.
//! A folder's `changed` signal and all six of a store's folder signals are
//! *queued* onto the context that the relevant `CamelSession` captured at
//! construction. `camel_folder_changed` and the store's signals
//! both schedule through `camel_session_idle_add`. That call always attaches
//! to the captured context, never to the thread-default context at the time
//! the signal fires. Delivery happens from a main loop. Nothing arrives on a
//! thread that never iterates a main loop, and a Rust test thread does not. A
//! test that connects a handler and reads what it recorded observes silence
//! every time. Such a test could mistake that silence for a pass.
//!
//! Every test here holds a [`Context`] and reads through [`emissions`] or
//! [`events`], which pump before they read. This pattern started in
//! `tests/refresh.rs` for the folder's `changed` signal alone. It lives here
//! because the store's six signals have the same problem and the same
//! ordering rule: see [`Context`].

use std::cell::RefCell;
use std::ffi::CStr;
use std::ptr;

use eds_sys::{
    CamelFolder, CamelFolderChangeInfo, CamelFolderInfo, CamelFolderInfoFlags, CamelStore,
    camel_folder_change_info_get_added_uids, camel_folder_change_info_get_changed_uids,
    camel_folder_change_info_get_recent_uids, camel_folder_change_info_get_removed_uids,
};
use glib_sys::{
    GFALSE, GMainContext, GPtrArray, g_main_context_iteration, g_main_context_new,
    g_main_context_pop_thread_default, g_main_context_push_thread_default, g_main_context_unref,
    gchar, gpointer,
};
use gobject_sys::g_signal_connect_data;
use jmap_backend_core::callback::{CHandler, gcallback};

/// A main context owned by this test, pushed as the thread default for as long
/// as it is held.
///
/// This is not a convenience, it is required. `g_main_context_iteration` on
/// the *global* default acquires it first. When another thread already owns
/// it, the call returns immediately and dispatches nothing. A Rust test binary
/// runs all its tests as threads of one process, so tests that pumped the same
/// context would steal each other's turn. A queued emission then arrives one
/// pump too late, or not within the test at all. This caused the intermittent
/// failures in `tests/refresh.rs`. A context per test gives each test its own
/// queue.
///
/// **Push this context before the test does anything else.** What matters is
/// which context is current when Camel *queues* the signal, not when the
/// test reads it. The queuing moment comes earlier than it looks. Both
/// groups of signals queue this way: `camel_folder_changed`, and `camel_store_folder_created`
/// with its four sibling signals, all queue onto the context that the
/// relevant `CamelSession` captured. `camel_session_ref_main_context` takes
/// that context once, at construction. A test that opens its account first
/// and pushes the context afterwards puts its session's queue on the context
/// that every other test also uses.
pub struct Context(*mut GMainContext);

impl Context {
    pub fn push() -> Self {
        // SAFETY: a fresh context, pushed on this thread and popped in `drop`.
        // This follows the stack discipline that
        // `g_main_context_pop_thread_default` requires.
        unsafe {
            let context = g_main_context_new();
            g_main_context_push_thread_default(context);
            Self(context)
        }
    }

    /// Delivers everything queued, without blocking on anything not yet ready.
    pub fn pump(&self) {
        // SAFETY: a live context that only this thread uses. FALSE tells the
        // call not to wait for a source to become ready.
        unsafe { while g_main_context_iteration(self.0, GFALSE) != GFALSE {} }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the context pushed in `push`. The reference taken
        // there is released here.
        unsafe {
            g_main_context_pop_thread_default(self.0);
            g_main_context_unref(self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// a folder's `changed`

/// What one emission of the `changed` signal carried.
#[derive(Debug, PartialEq, Eq)]
pub struct Emission {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    /// Only a delta fills this list, and it has a side effect. Camel hands a
    /// folder's recent uids to the session's filter driver. A message on this
    /// list is one that the user's incoming rules will act on.
    pub recent: Vec<String>,
}

thread_local! {
    /// Every emission the folder made, in order. This is a thread local
    /// rather than user data passed through the handler. That design is sound
    /// because [`Context`] makes the pumping thread the only thread that can
    /// deliver.
    static EMISSIONS: RefCell<Vec<Emission>> = const { RefCell::new(Vec::new()) };
}

/// Listens to a folder the way Evolution's message list listens.
pub fn watch(folder: *mut CamelFolder) {
    EMISSIONS.with(|seen| seen.borrow_mut().clear());
    // SAFETY: `folder` is a live GObject. The signal name is one that
    // `CamelFolder` declares. The handler has the signature that the
    // signal's marshaller calls.
    unsafe {
        connect(
            folder.cast(),
            c"changed",
            on_changed
                as unsafe extern "C" fn(*mut CamelFolder, *mut CamelFolderChangeInfo, gpointer),
        )
    };
}

unsafe extern "C" fn on_changed(
    _folder: *mut CamelFolder,
    changes: *mut CamelFolderChangeInfo,
    _data: gpointer,
) {
    // SAFETY: the signal hands over a live change info for the duration of
    // the emission. The four accessors borrow its arrays.
    let emission = unsafe {
        Emission {
            added: uid_list(camel_folder_change_info_get_added_uids(changes)),
            removed: uid_list(camel_folder_change_info_get_removed_uids(changes)),
            changed: uid_list(camel_folder_change_info_get_changed_uids(changes)),
            recent: uid_list(camel_folder_change_info_get_recent_uids(changes)),
        }
    };
    EMISSIONS.with(|seen| seen.borrow_mut().push(emission));
}

/// Everything the folder has announced since [`watch`], after giving the
/// test's main context the chance to deliver it. See this module's header:
/// reading without pumping always reads nothing.
pub fn emissions(context: &Context) -> Vec<Emission> {
    context.pump();
    EMISSIONS.with(|seen| seen.take())
}

// ---------------------------------------------------------------------------
// a store's six folder signals

/// One folder as an announcement described it.
///
/// The flags field is here because for two of the six signals, the flags are
/// half of what the signal announces: an info that says a folder was
/// subscribed, but does not carry `CAMEL_FOLDER_SUBSCRIBED`, is one that
/// Evolution would draw without a tick.
#[derive(Debug, PartialEq, Eq)]
pub struct Announced {
    pub path: String,
    pub display_name: String,
    pub flags: CamelFolderInfoFlags,
}

/// What one emission of a store's folder signals carried.
#[derive(Debug, PartialEq, Eq)]
pub struct FolderEvent {
    /// Which of the six signals this was, named by the name it is connected
    /// under.
    pub signal: String,
    /// The path the folder had before. Only `folder-renamed` carries this.
    pub old_name: Option<String>,
    /// The `CamelFolderInfo` forest, pre-order: a folder, then everything
    /// under it, then its next sibling. A rename also announces the folder's
    /// descendants, and their new paths are the point of the announcement.
    pub folders: Vec<Announced>,
}

impl FolderEvent {
    /// The paths alone. Most assertions check only these.
    pub fn paths(&self) -> Vec<&str> {
        self.folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect()
    }
}

thread_local! {
    /// Every folder signal the store made, in order. All six signals share
    /// one list, because a test that asserts a subscribe was announced is
    /// also asserting that nothing else was.
    static EVENTS: RefCell<Vec<FolderEvent>> = const { RefCell::new(Vec::new()) };
}

/// The four signals whose handler takes only an info. The two signals on
/// `CamelSubscribable` are also connected on the store: an interface's
/// signals belong to the instance, and this store implements the interface.
const INFO_SIGNALS: [&CStr; 4] = [
    c"folder-created",
    c"folder-deleted",
    c"folder-subscribed",
    c"folder-unsubscribed",
];

/// Listens to a store the way Evolution's folder tree listens.
pub fn watch_store(store: *mut CamelStore) {
    EVENTS.with(|seen| seen.borrow_mut().clear());
    for signal in INFO_SIGNALS {
        // SAFETY: `store` is a live GObject. Each name is a signal that
        // the store or an interface it implements declares, and
        // `on_folder_info` has the signature that their shared marshaller
        // calls. The user data is the static name itself: see
        // `on_folder_info`.
        unsafe {
            connect_with(
                store.cast(),
                signal,
                on_folder_info
                    as unsafe extern "C" fn(*mut CamelStore, *mut CamelFolderInfo, gpointer),
            )
        };
    }
    // SAFETY: as above, for the signal that passes no info. The user data
    // is its static name.
    unsafe {
        connect_with(
            store.cast(),
            c"folder-info-stale",
            on_folder_info_stale as unsafe extern "C" fn(*mut CamelStore, gpointer),
        )
    };
    // SAFETY: as above, for the one signal whose marshaller passes the old
    // path before the info.
    unsafe {
        connect_with(
            store.cast(),
            c"folder-renamed",
            on_folder_renamed
                as unsafe extern "C" fn(
                    *mut CamelStore,
                    *const gchar,
                    *mut CamelFolderInfo,
                    gpointer,
                ),
        )
    };
}

/// The handler shared by the four one-argument signals.
///
/// The user data tells which signal fired: it is the static name the signal
/// was connected under. A `&'static CStr` outlives every store, and sharing it
/// avoids writing four handlers that would differ only in a string literal.
unsafe extern "C" fn on_folder_info(
    _store: *mut CamelStore,
    info: *mut CamelFolderInfo,
    data: gpointer,
) {
    // SAFETY: `data` is the pointer that `watch_store` passed: one of the
    // static names above. `info` is a live forest for the duration of the
    // emission.
    unsafe { record(data.cast::<gchar>(), None, info) };
}

/// Evolution rebuilds the full tree of the account for this signal.
unsafe extern "C" fn on_folder_info_stale(_store: *mut CamelStore, data: gpointer) {
    // SAFETY: as in `on_folder_info`. A NULL info is an empty forest.
    unsafe { record(data.cast::<gchar>(), None, ptr::null_mut()) };
}

unsafe extern "C" fn on_folder_renamed(
    _store: *mut CamelStore,
    old_name: *const gchar,
    info: *mut CamelFolderInfo,
    data: gpointer,
) {
    // SAFETY: as above. `old_name` is the NUL-terminated path that Camel was
    // given.
    unsafe { record(data.cast::<gchar>(), read(old_name), info) };
}

/// Writes one emission down.
///
/// # Safety
///
/// `signal` must be a live NUL-terminated string and `info` NULL or a live
/// `CamelFolderInfo` forest.
unsafe fn record(signal: *const gchar, old_name: Option<String>, info: *mut CamelFolderInfo) {
    // SAFETY: the contract above.
    let event = unsafe {
        FolderEvent {
            signal: read(signal).unwrap_or_default(),
            old_name,
            folders: forest(info),
        }
    };
    EVENTS.with(|seen| seen.borrow_mut().push(event));
}

/// A `CamelFolderInfo` forest, pre-order.
///
/// Iterative, for the reason `FolderInfoChain::from_forest` gives: a server
/// chooses the depth through a `parentId` chain, so a recursive reader would
/// let a server trigger a stack overflow.
///
/// # Safety
///
/// `head` must be NULL or the head of a live sibling chain.
unsafe fn forest(head: *mut CamelFolderInfo) -> Vec<Announced> {
    let mut seen = Vec::new();
    let mut pending = vec![head];
    while let Some(info) = pending.pop() {
        if info.is_null() {
            continue;
        }
        // SAFETY: the contract above. Every pointer reached from a live
        // chain's `next` and `child` is NULL or another live info.
        unsafe {
            seen.push(Announced {
                path: read((*info).full_name).unwrap_or_default(),
                display_name: read((*info).display_name).unwrap_or_default(),
                flags: (*info).flags,
            });
            // Push `next` first and `child` second, so the child comes off
            // the stack next. A folder is followed by everything under it,
            // and only then by its own next sibling.
            pending.push((*info).next);
            pending.push((*info).child);
        }
    }
    seen
}

/// Everything the store has announced since [`watch_store`], pumped first.
pub fn events(context: &Context) -> Vec<FolderEvent> {
    context.pump();
    EVENTS.with(|seen| seen.take())
}

// ---------------------------------------------------------------------------
// the plumbing under both

/// `g_signal_connect` with no user data, spelled out.
///
/// # Safety
///
/// `instance` must be a live GObject, `signal` a signal it declares, and
/// `handler` a function with the signature that signal's marshaller calls.
unsafe fn connect(instance: *mut gobject_sys::GObject, signal: &CStr, handler: impl CHandler) {
    // SAFETY: the contract above.
    unsafe { connect_data(instance, signal, handler, ptr::null_mut()) };
}

/// The same, passing the signal's own name as the user data.
///
/// # Safety
///
/// As [`connect`]. `signal` must outlive the connection, as the `&'static
/// CStr`s above do.
unsafe fn connect_with(instance: *mut gobject_sys::GObject, signal: &CStr, handler: impl CHandler) {
    // SAFETY: the contract above.
    unsafe {
        connect_data(
            instance,
            signal,
            handler,
            signal.as_ptr().cast_mut().cast::<std::ffi::c_void>(),
        )
    };
}

/// # Safety
///
/// As [`connect_with`].
unsafe fn connect_data(
    instance: *mut gobject_sys::GObject,
    signal: &CStr,
    handler: impl CHandler,
    data: gpointer,
) {
    // SAFETY: the contract above. The marshaller casts the erased handler
    // back to the signature that the signal declares.
    unsafe {
        let id =
            g_signal_connect_data(instance, signal.as_ptr(), gcallback(handler), data, None, 0);
        assert_ne!(id, 0, "nothing connected to {signal:?}");
    }
}

/// A borrowed `GPtrArray` of uids, as strings.
///
/// # Safety
///
/// `array` must be NULL or a live array of NUL-terminated strings.
pub unsafe fn uid_list(array: *mut GPtrArray) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: the contract above. The strings live as long as the array.
    unsafe {
        (0..(*array).len)
            .map(|index| {
                let uid = *(*array).pdata.add(index as usize);
                CStr::from_ptr(uid.cast()).to_string_lossy().into_owned()
            })
            .collect()
    }
}

/// A borrowed C string, copied.
///
/// # Safety
///
/// `text` must be NULL or NUL-terminated and live for the call.
unsafe fn read(text: *const gchar) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the contract above.
    Some(
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned(),
    )
}
