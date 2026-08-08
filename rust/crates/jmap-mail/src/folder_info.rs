// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A [`FolderTree`] as the `CamelFolderInfo` forest `get_folder_info_sync`
//! returns.
//!
//! `jmap-mail-sync` already decided what the folders *are*: which mailbox is
//! which folder, what its Camel path is, where it sits in the tree. This module
//! is only the crossing into C, and it exists as its own file because that
//! crossing has an ownership rule that nothing in the type system enforces.
//!
//! `CamelFolderInfo` is not an object. It is a plain struct with three pointers
//! (`next`, `parent`, `child`) and two `gchar *`s, and the whole forest is freed
//! by handing the head of the root chain to `camel_folder_info_free`, which
//! walks `next` and `child` and `g_free`s both names. Three consequences shape
//! everything below:
//!
//! * The names must come from `g_malloc`. A `CString::into_raw` in their place
//!   frees Rust's allocation with GLib's allocator — heap corruption that
//!   usually surfaces somewhere else entirely.
//! * Ownership is all-or-nothing and lives at the head. [`FolderInfoChain`] is
//!   therefore the only owner while the chain is ours, and [`into_raw`] is the
//!   single point where it stops being ours — a vfunc that returned the pointer
//!   while a `Drop` still held it would be a double free.
//! * A half-built forest has no owner, so building must not be able to fail
//!   part-way. Nothing in [`FolderInfoChain::from_forest`] can: `g_malloc` aborts
//!   rather than returning NULL, and the one fallible conversion — a name with
//!   a NUL in it — is resolved by rewriting the name, not by returning an
//!   error.
//!
//! [`into_raw`]: FolderInfoChain::into_raw

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    CAMEL_FOLDER_CHILDREN, CAMEL_FOLDER_NOCHILDREN, CAMEL_FOLDER_SUBSCRIBED, CAMEL_FOLDER_SYSTEM,
    CAMEL_FOLDER_TYPE_ARCHIVE, CAMEL_FOLDER_TYPE_DRAFTS, CAMEL_FOLDER_TYPE_INBOX,
    CAMEL_FOLDER_TYPE_JUNK, CAMEL_FOLDER_TYPE_NORMAL, CAMEL_FOLDER_TYPE_SENT,
    CAMEL_FOLDER_TYPE_TRASH, CamelFolderInfo, CamelFolderInfoFlags, camel_folder_info_free,
    camel_folder_info_new,
};
use jmap_mail_sync::{FolderInfo, FolderRole, FolderTree};

/// A `CamelFolderInfo` forest, owned.
///
/// The pointer is the head of the *root* sibling chain, which is what
/// `camel_folder_info_free` wants and what `get_folder_info_sync` returns. NULL
/// is a legitimate value and means an account with no folders — Camel reads a
/// NULL return with no error set exactly that way, so there is no need for an
/// `Option` around it.
pub struct FolderInfoChain {
    head: *mut CamelFolderInfo,
}

impl FolderInfoChain {
    /// Translate a whole tree: every folder of an account, at every depth.
    pub fn from_tree(tree: &FolderTree) -> Self {
        Self::from_forest(tree.roots(), None)
    }

    /// Translate one sibling chain and `depth` levels of descendants below it,
    /// where `None` means all of them.
    ///
    /// The two arguments are what `get_folder_info_sync`'s `top` and
    /// `CAMEL_STORE_FOLDER_INFO_RECURSIVE` come down to once
    /// [`Request`](crate::folders::Request) has read them; this end only
    /// allocates.
    ///
    /// The depth is applied while the forest is built rather than to a finished
    /// one, because a cut that happened afterwards would have allocated — and
    /// freed — every folder of a deep account to keep its first level. What it
    /// deliberately does *not* touch is the flags: a folder whose children were
    /// left out still says `CAMEL_FOLDER_CHILDREN`, since that flag is what
    /// makes the folder tree draw the expander the cut level is fetched
    /// through.
    ///
    /// Iteratively, with an explicit stack of sibling groups still to link.
    /// The depth of the tree comes from a `parentId` chain the server chose, so
    /// recursing over it would be a stack overflow a server could ask for — the
    /// same reasoning that made the walk in `jmap-mail-sync` iterative.
    /// `camel_folder_info_free` recursing over the result is Camel's own
    /// bound and not one this side can lift.
    pub fn from_forest(siblings: &[FolderInfo], depth: Option<usize>) -> Self {
        let mut head: *mut CamelFolderInfo = ptr::null_mut();
        // Each entry is a sibling list, the info it hangs off — NULL for the
        // roots, which is also how the head is recognised below — and how many
        // levels below it are still wanted.
        let mut pending: Vec<(&[FolderInfo], *mut CamelFolderInfo, Option<usize>)> =
            vec![(siblings, ptr::null_mut(), depth)];

        while let Some((siblings, parent, depth)) = pending.pop() {
            let mut previous: *mut CamelFolderInfo = ptr::null_mut();
            for folder in siblings {
                let info = alloc_info(folder, parent);
                // SAFETY: `previous` and `parent` are infos allocated by
                // `alloc_info` earlier in this loop and not yet handed to
                // anyone; `info` is fresh.
                unsafe {
                    if !previous.is_null() {
                        (*previous).next = info;
                    } else if !parent.is_null() {
                        (*parent).child = info;
                    } else {
                        head = info;
                    }
                }
                previous = info;
                if !folder.children.is_empty() && depth != Some(0) {
                    pending.push((&folder.children, info, depth.map(|depth| depth - 1)));
                }
            }
        }

        Self { head }
    }

    /// The head of the root chain, still owned by this wrapper.
    ///
    /// For reading and for passing to a C function that only borrows, such as
    /// `camel_store_folder_created`.
    pub fn as_ptr(&self) -> *mut CamelFolderInfo {
        self.head
    }

    /// Hand the forest to the caller, who must free it with
    /// `camel_folder_info_free`.
    ///
    /// This is what a `get_folder_info_sync` override returns. Consuming
    /// `self`, so the `Drop` cannot also run.
    pub fn into_raw(self) -> *mut CamelFolderInfo {
        let head = self.head;
        std::mem::forget(self);
        head
    }
}

impl Drop for FolderInfoChain {
    fn drop(&mut self) {
        // SAFETY: `head` is NULL or the head of a forest built above and never
        // handed out (`into_raw` forgets `self`); `camel_folder_info_free`
        // tolerates NULL and frees the whole chain.
        unsafe { camel_folder_info_free(self.head) };
    }
}

/// One folder, as a fresh `CamelFolderInfo` with everything but its links to
/// siblings and children filled in.
///
/// `camel_folder_info_new` zeroes the struct — eds-sys's `tests/camel.rs` pins
/// that — so `next` and `child` are already NULL and the caller only has to
/// write the ones it has an answer for.
fn alloc_info(folder: &FolderInfo, parent: *mut CamelFolderInfo) -> *mut CamelFolderInfo {
    let full_name = c_string(&folder.path);
    let display_name = c_string(&folder.display_name);

    // SAFETY: the info is freshly allocated and owned here; `g_strdup` gives
    // the two name fields the g_malloc'ed copies `camel_folder_info_free` will
    // `g_free`.
    unsafe {
        let info = camel_folder_info_new();
        (*info).parent = parent;
        (*info).full_name = glib_sys::g_strdup(full_name.as_ptr());
        (*info).display_name = glib_sys::g_strdup(display_name.as_ptr());
        (*info).flags = flags(folder);
        (*info).total = count(folder.total);
        (*info).unread = count(folder.unread);
        info
    }
}

/// What Camel should believe about a folder, in one word.
fn flags(folder: &FolderInfo) -> CamelFolderInfoFlags {
    // The type is a small integer in a field of the flags word rather than a
    // bit of its own, so exactly one type is OR-ed in — see eds-sys's
    // `the_folder_type_is_a_field_inside_the_flags_word`.
    //
    // `SYSTEM` rides along with every role, which is what stops Evolution
    // offering to rename or delete the folder. The server would refuse, and on
    // JMAP the refusal arrives well after the user believed the folder was
    // gone. It is also how evolution-ews marks the same six folders.
    let mut flags = match folder.role {
        Some(FolderRole::Inbox) => CAMEL_FOLDER_TYPE_INBOX | CAMEL_FOLDER_SYSTEM,
        Some(FolderRole::Drafts) => CAMEL_FOLDER_TYPE_DRAFTS | CAMEL_FOLDER_SYSTEM,
        Some(FolderRole::Sent) => CAMEL_FOLDER_TYPE_SENT | CAMEL_FOLDER_SYSTEM,
        Some(FolderRole::Trash) => CAMEL_FOLDER_TYPE_TRASH | CAMEL_FOLDER_SYSTEM,
        Some(FolderRole::Junk) => CAMEL_FOLDER_TYPE_JUNK | CAMEL_FOLDER_SYSTEM,
        Some(FolderRole::Archive) => CAMEL_FOLDER_TYPE_ARCHIVE | CAMEL_FOLDER_SYSTEM,
        None => CAMEL_FOLDER_TYPE_NORMAL,
    };

    if folder.subscribed {
        flags |= CAMEL_FOLDER_SUBSCRIBED;
    }

    // Whether the folder tree draws an expander. Deliberately not
    // `NOINFERIORS`, which claims the folder can never *have* children: a JMAP
    // mailbox accepts one at any time, and the claim would remove "New
    // Subfolder" from every leaf for the life of the account.
    flags |= if folder.children.is_empty() {
        CAMEL_FOLDER_NOCHILDREN
    } else {
        CAMEL_FOLDER_CHILDREN
    };

    flags
}

/// A message count in the signed 32-bit field Camel keeps it in.
///
/// Saturating rather than truncating: Camel uses negative counts for "not known
/// yet", so a count whose top bit survives a cast would read as *unknown*
/// rather than as implausibly large. `jmap-mail-sync` already saturated the
/// server's 64 bits into 32; this is the second, signed half of the same
/// argument.
fn count(count: u32) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// A Rust string as a C string, with any NUL rewritten.
///
/// A JMAP string is a JSON string, so a mailbox name can contain a NUL even
/// though RFC 8621 §2 forbids it. Passing the bytes through would truncate the
/// name there — `Work\0Secret` displayed as `Work`, sitting next to the real
/// `Work` and indistinguishable from it. U+FFFD keeps the name distinct and
/// visibly broken, and it cannot fail, which is what lets the build above have
/// no error path and therefore no half-built forest to clean up.
pub(crate) fn c_string(text: &str) -> CString {
    match CString::new(text) {
        Ok(string) => string,
        Err(_) => CString::new(text.replace('\0', "\u{fffd}"))
            .expect("a string with every NUL replaced has no NUL left"),
    }
}
