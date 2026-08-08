// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `FolderTree`-to-`CamelFolderInfo` translation, checked by reading the C
//! chain back.
//!
//! `jmap-mail-sync` already tests the tree itself, on the inputs a server can
//! get wrong. What is left, and what this file is about, is the crossing: a
//! `Vec<FolderInfo>` with owned `String`s becomes a linked forest of
//! `g_malloc`ed structs whose `next`/`parent`/`child` pointers Camel will walk
//! and whose names Camel will `g_free`. So every test here builds a chain and
//! then walks it the way Camel does — pointer by pointer, `CStr` by `CStr` —
//! rather than trusting the Rust side of the boundary.
//!
//! The tests deliberately let the chain drop, which frees it. Under a leak
//! checker that is the other half of the assertion; without one it is at least
//! a use-after-free the moment a `Drop` frees something Camel still owns.

use std::ffi::CStr;

use eds_sys::{
    CAMEL_FOLDER_CHILDREN, CAMEL_FOLDER_NOCHILDREN, CAMEL_FOLDER_NOINFERIORS,
    CAMEL_FOLDER_SUBSCRIBED, CAMEL_FOLDER_SYSTEM, CAMEL_FOLDER_TYPE_ARCHIVE,
    CAMEL_FOLDER_TYPE_DRAFTS, CAMEL_FOLDER_TYPE_INBOX, CAMEL_FOLDER_TYPE_JUNK,
    CAMEL_FOLDER_TYPE_MASK, CAMEL_FOLDER_TYPE_NORMAL, CAMEL_FOLDER_TYPE_SENT,
    CAMEL_FOLDER_TYPE_TRASH, CamelFolderInfo, CamelFolderInfoFlags, camel_folder_info_free,
};
use jmap_mail::folder_info::FolderInfoChain;
use jmap_mail_sync::FolderTree;
use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

/// One level of the C chain, read back into something a test can assert on.
#[derive(Debug, PartialEq, Eq)]
struct Read {
    full_name: String,
    display_name: String,
    flags: CamelFolderInfoFlags,
    total: i32,
    unread: i32,
    children: Vec<Read>,
}

/// Walk a sibling chain, checking each entry's `parent` back-pointer on the way.
///
/// # Safety
///
/// `head` is NULL or the head of a `CamelFolderInfo` sibling chain whose
/// entries' `parent` fields all equal `parent`.
unsafe fn read_chain(head: *mut CamelFolderInfo, parent: *mut CamelFolderInfo) -> Vec<Read> {
    let mut chain = Vec::new();
    let mut info = head;
    while !info.is_null() {
        unsafe {
            assert_eq!(
                (*info).parent,
                parent,
                "an info's parent pointer does not point at its parent"
            );
            assert!(!(*info).full_name.is_null(), "an info has no full_name");
            assert!(
                !(*info).display_name.is_null(),
                "an info has no display_name"
            );
            chain.push(Read {
                full_name: CStr::from_ptr((*info).full_name)
                    .to_string_lossy()
                    .into_owned(),
                display_name: CStr::from_ptr((*info).display_name)
                    .to_string_lossy()
                    .into_owned(),
                flags: (*info).flags,
                total: (*info).total,
                unread: (*info).unread,
                children: read_chain((*info).child, info),
            });
            info = (*info).next;
        }
    }
    chain
}

fn read(chain: &FolderInfoChain) -> Vec<Read> {
    // SAFETY: the chain owns a forest built by the code under test; its roots
    // have no parent.
    unsafe { read_chain(chain.as_ptr(), std::ptr::null_mut()) }
}

fn mailbox(id: &str, name: &str) -> Mailbox {
    Mailbox {
        id: Some(Id::new(id)),
        name: name.to_owned(),
        ..Mailbox::default()
    }
}

fn child(id: &str, name: &str, parent: &str) -> Mailbox {
    Mailbox {
        parent_id: Some(Id::new(parent)),
        ..mailbox(id, name)
    }
}

fn chain(mailboxes: &[Mailbox]) -> FolderInfoChain {
    let tree = FolderTree::from_mailboxes(mailboxes).expect("a well-formed mailbox list");
    FolderInfoChain::from_tree(&tree)
}

/// The paths of a chain, parents before children, as Camel would collect them.
fn full_names(chain: &[Read]) -> Vec<String> {
    chain
        .iter()
        .flat_map(|info| std::iter::once(info.full_name.clone()).chain(full_names(&info.children)))
        .collect()
}

fn folder_type(flags: CamelFolderInfoFlags) -> CamelFolderInfoFlags {
    flags & CAMEL_FOLDER_TYPE_MASK as CamelFolderInfoFlags
}

/// An account with no mailboxes at all is a NULL chain, not an error and not an
/// empty struct: `get_folder_info_sync` returning NULL with no error set is how
/// Camel is told a store has no folders.
#[test]
fn an_empty_tree_is_a_null_chain() {
    let chain = chain(&[]);

    assert!(chain.as_ptr().is_null());
    assert_eq!(read(&chain), []);
}

#[test]
fn top_level_folders_become_one_sibling_chain() {
    let chain = chain(&[mailbox("M1", "Inbox"), mailbox("M2", "Sent")]);

    assert_eq!(full_names(&read(&chain)), ["Inbox", "Sent"]);
}

/// Both name fields, and the fact that they are different fields: `full_name`
/// is the path Camel keys the folder by, `display_name` the string the user
/// sees. A mailbox whose name needs encoding is where the two come apart, and
/// swapping them is a folder tree that looks right and cannot be opened.
#[test]
fn the_path_is_the_full_name_and_the_mailbox_name_is_displayed() {
    let chain = chain(&[mailbox("M1", "Two/Parts")]);
    let read = read(&chain);

    assert_eq!(read[0].full_name, "Two%2FParts");
    assert_eq!(read[0].display_name, "Two/Parts");
}

/// The shape Camel walks: a parent's first child hangs off `child`, the rest
/// off that child's `next`, and every one of them points back at the parent.
/// `read_chain` asserts the back-pointers, which is the field a store gets
/// wrong by building the chain top-down and forgetting a level.
#[test]
fn children_hang_off_their_parent_in_sibling_order() {
    let chain = chain(&[
        mailbox("M1", "Parent"),
        child("M3", "Second", "M1"),
        child("M2", "First", "M1"),
        child("M4", "Deeper", "M2"),
    ]);
    let read = read(&chain);

    assert_eq!(
        full_names(&read),
        [
            "Parent",
            "Parent/First",
            "Parent/First/Deeper",
            "Parent/Second"
        ]
    );
    assert_eq!(read.len(), 1, "only the parent is top-level");
    assert_eq!(read[0].children.len(), 2);
}

/// `CHILDREN` and `NOCHILDREN` are what the folder tree draws its expander
/// from. `NOINFERIORS` is a different claim — that the folder can never have
/// children — and JMAP mailboxes accept a child at any time, so a leaf must not
/// carry it or Evolution hides "New Subfolder" forever.
#[test]
fn a_leaf_says_it_has_no_children_but_not_that_it_can_have_none() {
    let chain = chain(&[mailbox("M1", "Parent"), child("M2", "Leaf", "M1")]);
    let read = read(&chain);

    assert_ne!(read[0].flags & CAMEL_FOLDER_CHILDREN, 0, "parent: CHILDREN");
    assert_eq!(read[0].flags & CAMEL_FOLDER_NOCHILDREN, 0);

    let leaf = &read[0].children[0];
    assert_ne!(leaf.flags & CAMEL_FOLDER_NOCHILDREN, 0, "leaf: NOCHILDREN");
    assert_eq!(leaf.flags & CAMEL_FOLDER_CHILDREN, 0);
    assert_eq!(
        leaf.flags & CAMEL_FOLDER_NOINFERIORS,
        0,
        "a JMAP mailbox can always be given a child"
    );
}

/// The six roles `jmap-mail-sync` maps, in the flags word. Evolution reads the
/// type field to decide which folder Send puts a copy in, which one Delete
/// moves to, and which icon each gets — so a role that arrives as
/// `TYPE_NORMAL` is a Trash folder the user has to find themselves.
#[test]
fn roles_become_camel_folder_types() {
    let with_role = |id: &str, name: &str, role: &str| Mailbox {
        role: Some(role.to_owned()),
        ..mailbox(id, name)
    };
    let chain = chain(&[
        with_role("M1", "In", role::INBOX),
        with_role("M2", "Out", role::SENT),
        with_role("M3", "Bin", role::TRASH),
        with_role("M4", "Spam", role::JUNK),
        with_role("M5", "Unsent", role::DRAFTS),
        with_role("M6", "Old", role::ARCHIVE),
        mailbox("M7", "Ordinary"),
    ]);
    let read = read(&chain);
    let by_name = |name: &str| {
        read.iter()
            .find(|info| info.display_name == name)
            .unwrap_or_else(|| panic!("no folder called {name}"))
    };

    assert_eq!(folder_type(by_name("In").flags), CAMEL_FOLDER_TYPE_INBOX);
    assert_eq!(folder_type(by_name("Out").flags), CAMEL_FOLDER_TYPE_SENT);
    assert_eq!(folder_type(by_name("Bin").flags), CAMEL_FOLDER_TYPE_TRASH);
    assert_eq!(folder_type(by_name("Spam").flags), CAMEL_FOLDER_TYPE_JUNK);
    assert_eq!(
        folder_type(by_name("Unsent").flags),
        CAMEL_FOLDER_TYPE_DRAFTS
    );
    assert_eq!(folder_type(by_name("Old").flags), CAMEL_FOLDER_TYPE_ARCHIVE);
    assert_eq!(
        folder_type(by_name("Ordinary").flags),
        CAMEL_FOLDER_TYPE_NORMAL
    );
}

/// A role folder is also a *system* folder, which is what stops Evolution from
/// offering to rename or delete it — the server would refuse, and on JMAP the
/// refusal arrives long after the user believed the folder was gone.
#[test]
fn a_role_folder_is_a_system_folder_and_an_ordinary_one_is_not() {
    let chain = chain(&[
        Mailbox {
            role: Some(role::INBOX.to_owned()),
            ..mailbox("M1", "In")
        },
        mailbox("M2", "Ordinary"),
    ]);
    let read = read(&chain);

    assert_ne!(
        read[0].flags & CAMEL_FOLDER_SYSTEM,
        0,
        "the inbox is system"
    );
    assert_eq!(read[1].flags & CAMEL_FOLDER_SYSTEM, 0);
}

#[test]
fn subscription_is_a_flag_and_defaults_to_subscribed() {
    let chain = chain(&[
        Mailbox {
            is_subscribed: Some(false),
            ..mailbox("M1", "Hidden")
        },
        Mailbox {
            is_subscribed: Some(true),
            ..mailbox("M2", "Shown")
        },
        mailbox("M3", "Silent"),
    ]);
    let read = read(&chain);

    assert_eq!(read[0].flags & CAMEL_FOLDER_SUBSCRIBED, 0, "Hidden");
    assert_ne!(read[1].flags & CAMEL_FOLDER_SUBSCRIBED, 0, "Shown");
    assert_ne!(
        read[2].flags & CAMEL_FOLDER_SUBSCRIBED,
        0,
        "a server that models no subscriptions must not hide every folder"
    );
}

#[test]
fn counts_cross_as_they_are() {
    let chain = chain(&[Mailbox {
        total_emails: Some(42),
        unread_emails: Some(7),
        ..mailbox("M1", "Inbox")
    }]);
    let read = read(&chain);

    assert_eq!(read[0].total, 42);
    assert_eq!(read[0].unread, 7);
}

/// Camel counts in a *signed* 32-bit field, and uses negative values for "not
/// known yet". A count that arrives larger than `i32::MAX` therefore cannot be
/// truncated — the top bit would make it a small negative number, which reads
/// as an unknown count rather than as an implausible one. It saturates instead,
/// the same way `jmap-mail-sync` saturates into 32 bits in the first place.
#[test]
fn a_count_too_large_for_camel_saturates_rather_than_going_negative() {
    let chain = chain(&[Mailbox {
        total_emails: Some(u64::from(u32::MAX)),
        unread_emails: Some(i64::from(i32::MAX) as u64 + 1),
        ..mailbox("M1", "Enormous")
    }]);
    let read = read(&chain);

    assert_eq!(read[0].total, i32::MAX);
    assert_eq!(read[0].unread, i32::MAX);
}

/// A JMAP string is a JSON string, so a mailbox name may contain a NUL even
/// though RFC 8621 forbids it; a C string may not. Handing the bytes over
/// unchanged would truncate the display name at the NUL — a folder called
/// `Work\0Secret` shown as `Work`, indistinguishable from the real `Work` next
/// to it. The replacement character keeps the name distinct and visibly broken.
/// The path has no such problem: `jmap-mail-sync` already encodes the NUL.
#[test]
fn a_nul_in_a_mailbox_name_does_not_truncate_the_display_name() {
    let chain = chain(&[mailbox("M1", "Work\0Secret")]);
    let read = read(&chain);

    assert_eq!(read[0].display_name, "Work\u{fffd}Secret");
    assert_eq!(read[0].full_name, "Work%00Secret");
}

/// The other half of the ownership story. `get_folder_info_sync` hands the
/// chain to its caller, who frees it with `camel_folder_info_free`; the wrapper
/// has to be able to let go without also freeing. If `into_raw` left the `Drop`
/// in place this is a double free, which is why the test does the caller's free
/// itself.
#[test]
fn into_raw_hands_the_chain_over_and_the_wrapper_stops_owning_it() {
    let chain = chain(&[mailbox("M1", "Parent"), child("M2", "Child", "M1")]);
    let head = chain.into_raw();

    // SAFETY: `into_raw` gave us the only owning pointer, and the chain is a
    // forest of infos allocated with `camel_folder_info_new`.
    unsafe {
        assert_eq!(CStr::from_ptr((*head).full_name), c"Parent");
        assert!((*head).next.is_null());
        assert!(!(*head).child.is_null());
        camel_folder_info_free(head);
    }
}

/// A tree deeper than the C recursion in `camel_folder_info_free` would like is
/// still a tree this side has to build without a Rust stack overflow: the depth
/// comes from a `parentId` chain the server chose. The build is iterative, so
/// this is a test of that and not of Camel — which is also why the depth is
/// modest enough for Camel's own recursive free.
#[test]
fn a_deep_tree_does_not_overflow_the_stack() {
    const DEPTH: usize = 2_000;
    let mut mailboxes = vec![mailbox("M0", "Root")];
    for level in 1..DEPTH {
        mailboxes.push(child(
            &format!("M{level}"),
            &format!("Level{level}"),
            &format!("M{}", level - 1),
        ));
    }

    let chain = chain(&mailboxes);
    // Walking it back would recurse in `read_chain`, so count the levels
    // iteratively instead.
    let mut depth = 0;
    let mut info = chain.as_ptr();
    while !info.is_null() {
        depth += 1;
        // SAFETY: `info` is an entry of the chain the wrapper owns.
        info = unsafe { (*info).child };
    }

    assert_eq!(depth, DEPTH);
}
