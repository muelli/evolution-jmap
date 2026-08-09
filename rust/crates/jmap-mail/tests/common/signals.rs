// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Listening to a store and to a folder the way Evolution listens, and the main
//! context that is what makes listening work at all.
//!
//! Camel does not emit its notifications where they are reported. A folder's
//! `changed` and all five of a store's folder signals are *queued* — the folder
//! onto the context that was thread-default when `camel_folder_changed` was
//! called, the store onto the one its `CamelSession` captured when the session
//! was constructed — and delivered from a main loop. Nothing arrives on a thread
//! that never iterates one, which a Rust test thread does not. A test that
//! connected a handler and read what it recorded would therefore observe
//! silence, every time, and call it a pass.
//!
//! So every test here holds a [`Context`] and reads through [`emissions`] or
//! [`events`], which pump before they read. This started in `tests/refresh.rs`
//! for the folder's `changed` signal alone and lives here because the store's
//! five have exactly the same problem — and, in the store's case, a sharper
//! version of [`Context`]'s ordering rule: see that type.

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

/// A main context of this test's own, pushed as the thread default for as long
/// as it is held.
///
/// Not a convenience. `g_main_context_iteration` on the *global* default
/// acquires it first and returns immediately, dispatching nothing, when another
/// thread already owns it — and a Rust test binary runs its tests on threads of
/// one process, so tests that pumped the same context would steal each other's
/// turn. A queued emission then arrives one pump too late, or not within the
/// test at all, which is where `tests/refresh.rs`'s intermittent failures came
/// from. A context per test is a queue per test.
///
/// **Push it before anything else the test does.** What matters is which context
/// is current when Camel *queues*, not when the test reads, and for the store's
/// signals that moment is earlier than it looks: `camel_store_folder_created`
/// and its four siblings queue onto the context the store's `CamelSession`
/// captured — `camel_session_ref_main_context`, taken once at construction. A
/// test that opened its account first and pushed afterwards would put its
/// session's queue on the context every other test is also using.
pub struct Context(*mut GMainContext);

impl Context {
    pub fn push() -> Self {
        // SAFETY: a fresh context, pushed on this thread and popped in `drop`
        // — the stack discipline `g_main_context_pop_thread_default` requires.
        unsafe {
            let context = g_main_context_new();
            g_main_context_push_thread_default(context);
            Self(context)
        }
    }

    /// Delivers everything queued, without blocking on anything that is not.
    pub fn pump(&self) {
        // SAFETY: a live context this thread is the only user of, and FALSE is
        // what asks it not to wait for a source to become ready.
        unsafe { while g_main_context_iteration(self.0, GFALSE) != GFALSE {} }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the context pushed in `push`, and the reference taken
        // there is the one released here.
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
    /// The list only a delta may fill, and the one with a side effect: Camel
    /// hands a folder's recent uids to the session's filter driver, so a
    /// message on it is one the user's incoming rules will act on.
    pub recent: Vec<String>,
}

thread_local! {
    /// Every emission the folder made, in order. A thread local rather than
    /// user data threaded through the handler, which is sound because
    /// [`Context`] makes the pumping thread the only one that can deliver.
    static EMISSIONS: RefCell<Vec<Emission>> = const { RefCell::new(Vec::new()) };
}

/// Listens to a folder the way Evolution's message list listens.
pub fn watch(folder: *mut CamelFolder) {
    EMISSIONS.with(|seen| seen.borrow_mut().clear());
    // SAFETY: `folder` is a live GObject, the signal name is one `CamelFolder`
    // declares, and the handler has the signature that signal's marshaller
    // calls with.
    unsafe { connect(folder.cast(), c"changed", on_changed as *const ()) };
}

unsafe extern "C" fn on_changed(
    _folder: *mut CamelFolder,
    changes: *mut CamelFolderChangeInfo,
    _data: gpointer,
) {
    // SAFETY: the signal hands over a live change info for the duration of the
    // emission, and the four accessors borrow its arrays.
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

/// Everything the folder has announced since [`watch`], after giving the test's
/// main context the chance to deliver it. See this module's header: reading
/// without pumping reads nothing, always.
pub fn emissions(context: &Context) -> Vec<Emission> {
    context.pump();
    EMISSIONS.with(|seen| seen.take())
}

// ---------------------------------------------------------------------------
// a store's five folder signals

/// One folder as an announcement described it.
///
/// The flags are here because for two of the five signals they are half of what
/// is being announced: an info that says a folder was subscribed and does not
/// carry `CAMEL_FOLDER_SUBSCRIBED` is one Evolution would draw without a tick.
#[derive(Debug, PartialEq, Eq)]
pub struct Announced {
    pub path: String,
    pub display_name: String,
    pub flags: CamelFolderInfoFlags,
}

/// What one emission of a store's folder signals carried.
#[derive(Debug, PartialEq, Eq)]
pub struct FolderEvent {
    /// Which of the five it was, by the name it is connected under.
    pub signal: String,
    /// The path the folder had before, which only `folder-renamed` carries.
    pub old_name: Option<String>,
    /// The `CamelFolderInfo` forest, pre-order: a folder, then everything under
    /// it, then its next sibling. A rename announces the folder's descendants
    /// too, and their new paths are the point of it.
    pub folders: Vec<Announced>,
}

impl FolderEvent {
    /// The paths alone, which is what most assertions are about.
    pub fn paths(&self) -> Vec<&str> {
        self.folders
            .iter()
            .map(|folder| folder.path.as_str())
            .collect()
    }
}

thread_local! {
    /// Every folder signal the store made, in order — all five in one list,
    /// because a test that asserts a subscribe was announced is also asserting
    /// nothing else was.
    static EVENTS: RefCell<Vec<FolderEvent>> = const { RefCell::new(Vec::new()) };
}

/// The four signals whose handler takes an info and nothing else. The two on
/// `CamelSubscribable` are connected on the store as well: an interface's
/// signals belong to the instance, and this store implements it.
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
        // SAFETY: `store` is a live GObject; each name is a signal it or an
        // interface it implements declares, and `on_folder_info` has the
        // signature their shared marshaller calls with. The user data is the
        // static name itself — see `on_folder_info`.
        unsafe { connect_with(store.cast(), signal, on_folder_info as *const ()) };
    }
    // SAFETY: as above, with the one signal whose marshaller passes the old
    // path before the info.
    unsafe {
        connect_with(
            store.cast(),
            c"folder-renamed",
            on_folder_renamed as *const (),
        )
    };
}

/// The handler the four one-argument signals share.
///
/// Which of them fired is read back out of the user data, which is the static
/// name it was connected under: a `&'static CStr` outlives every store, and it
/// saves four handlers that would differ only in a string literal.
unsafe extern "C" fn on_folder_info(
    _store: *mut CamelStore,
    info: *mut CamelFolderInfo,
    data: gpointer,
) {
    // SAFETY: `data` is the pointer `watch_store` passed, one of the static
    // names above, and `info` is a live forest for the duration of the emission.
    unsafe { record(data.cast::<gchar>(), None, info) };
}

unsafe extern "C" fn on_folder_renamed(
    _store: *mut CamelStore,
    old_name: *const gchar,
    info: *mut CamelFolderInfo,
    data: gpointer,
) {
    // SAFETY: as above; `old_name` is the NUL-terminated path Camel was given.
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
/// Iteratively, for the reason `FolderInfoChain::from_forest` gives: the depth
/// of the chain comes from a `parentId` chain a server chose, so a recursive
/// reader would be a stack overflow a server could ask for.
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
        // SAFETY: the contract above — every pointer reached from a live
        // chain's `next` and `child` is NULL or another live info.
        unsafe {
            seen.push(Announced {
                path: read((*info).full_name).unwrap_or_default(),
                display_name: read((*info).display_name).unwrap_or_default(),
                flags: (*info).flags,
            });
            // `next` first and `child` second, so that the child is what comes
            // off the stack next: a folder is followed by everything under it,
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
unsafe fn connect(instance: *mut gobject_sys::GObject, signal: &CStr, handler: *const ()) {
    // SAFETY: the contract above.
    unsafe { connect_data(instance, signal, handler, ptr::null_mut()) };
}

/// The same, passing the signal's own name as the user data.
///
/// # Safety
///
/// As [`connect`], and `signal` must outlive the connection — which the
/// `&'static CStr`s above do.
unsafe fn connect_with(instance: *mut gobject_sys::GObject, signal: &CStr, handler: *const ()) {
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
    handler: *const (),
    data: gpointer,
) {
    // SAFETY: the contract above. The transmute to `GCallback` is what every
    // `g_signal_connect` in C is, spelled out — the marshaller casts it back to
    // the signature the signal declares.
    unsafe {
        let id = g_signal_connect_data(
            instance,
            signal.as_ptr(),
            Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                handler,
            )),
            data,
            None,
            0,
        );
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
    // SAFETY: the contract above; the strings live as long as the array.
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
