// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `FolderTree`-to-`CamelFolderInfo` translation, checked by reading the C
//! chain back.
//!
//! `jmap-mail-sync` already tests the tree itself, on the inputs a server can
//! get wrong. This file tests the crossing: a `Vec<FolderInfo>` with owned
//! `String`s becomes a linked forest of `g_malloc`ed structs. Camel walks these
//! structs through the `next`, `parent`, and `child` pointers, and frees the
//! names with `g_free`. Each test builds a chain, then walks it the way Camel
//! does: pointer by pointer, `CStr` by `CStr`. This checks the C side instead
//! of trusting the Rust side of the boundary.
//!
//! Each test lets the chain drop, which frees it. Under a leak checker, this is
//! the other half of the assertion. Without a leak checker, this still causes
//! a use-after-free the moment `Drop` frees something Camel still owns.

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
    // SAFETY: the chain owns a forest built by the code under test. Its roots
    // have no parent.
    unsafe { read_chain(chain.as_ptr(), std::ptr::null_mut()) }
}

fn mailbox(id: &str, name: &str) -> Mailbox {
    Mailbox::new(name).with_id(Id::new(id))
}

fn child(id: &str, name: &str, parent: &str) -> Mailbox {
    mailbox(id, name).with_parent_id(Id::new(parent))
}

fn chain(mailboxes: &[Mailbox]) -> FolderInfoChain {
    let tree = FolderTree::from_mailboxes(mailboxes).expect("a well-formed mailbox list");
    FolderInfoChain::from_tree(&tree)
}

/// The same, cut to `depth` levels of descendants below the top-level folders.
fn forest(mailboxes: &[Mailbox], depth: Option<usize>) -> FolderInfoChain {
    let tree = FolderTree::from_mailboxes(mailboxes).expect("a well-formed mailbox list");
    FolderInfoChain::from_forest(tree.roots(), depth)
}

/// Three levels, to have something for a cut to leave out.
fn three_levels() -> Vec<Mailbox> {
    vec![
        mailbox("M1", "Parent"),
        child("M2", "Child", "M1"),
        child("M3", "Grandchild", "M2"),
    ]
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

/// An account with no mailboxes is a NULL chain, not an error and not an empty
/// struct. `get_folder_info_sync` returns NULL with no error set to tell Camel
/// a store has no folders.
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

/// Both name fields are different. `full_name` is the path Camel keys the
/// folder by. `display_name` is the string the user sees. When a mailbox name
/// needs encoding, the two values differ. Swapped values produce a folder tree
/// that looks right but cannot be opened.
#[test]
fn the_path_is_the_full_name_and_the_mailbox_name_is_displayed() {
    let chain = chain(&[mailbox("M1", "Two/Parts")]);
    let read = read(&chain);

    assert_eq!(read[0].full_name, "Two%2FParts");
    assert_eq!(read[0].display_name, "Two/Parts");
}

/// The shape Camel walks: a parent's first child hangs off `child`. The rest
/// hang off that child's `next`. Each child points back at the parent.
/// `read_chain` asserts these back-pointers, the field a store gets wrong when
/// it builds the chain top-down and forgets a level.
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

/// The folder tree draws its expander from `CHILDREN` and `NOCHILDREN`.
/// `NOINFERIORS` is a different claim: the folder can never have children. A
/// JMAP mailbox accepts a child at any time, so a leaf must not carry
/// `NOINFERIORS`, or Evolution hides "New Subfolder" forever.
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
/// type field to choose the folder Send copies to, the folder Delete moves to,
/// and the icon for each folder. A role that arrives as `TYPE_NORMAL` becomes a
/// Trash folder the user must find manually.
#[test]
fn roles_become_camel_folder_types() {
    let with_role = |id: &str, name: &str, role: &str| mailbox(id, name).with_role(role);
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

/// A role folder is also a *system* folder. This stops Evolution from offering
/// to rename or delete it. The server would refuse the request, and on JMAP
/// the refusal arrives long after the user believes the folder is gone.
#[test]
fn a_role_folder_is_a_system_folder_and_an_ordinary_one_is_not() {
    let chain = chain(&[
        mailbox("M1", "In").with_role(role::INBOX),
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
        mailbox("M1", "Hidden").is_subscribed(false),
        mailbox("M2", "Shown").is_subscribed(true),
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
    let chain = chain(&[mailbox("M1", "Inbox")
        .with_total_emails(42)
        .with_unread_emails(7)]);
    let read = read(&chain);

    assert_eq!(read[0].total, 42);
    assert_eq!(read[0].unread, 7);
}

/// Camel counts use a *signed* 32-bit field, with negative values meaning "not
/// known yet". A count larger than `i32::MAX` must not be truncated: the top
/// bit would turn it into a small negative number, read as unknown rather than
/// as implausible. The count saturates instead, the same way `jmap-mail-sync`
/// saturates into 32 bits in the first place.
#[test]
fn a_count_too_large_for_camel_saturates_rather_than_going_negative() {
    let chain = chain(&[mailbox("M1", "Enormous")
        .with_total_emails(u64::from(u32::MAX))
        .with_unread_emails(i64::from(i32::MAX) as u64 + 1)]);
    let read = read(&chain);

    assert_eq!(read[0].total, i32::MAX);
    assert_eq!(read[0].unread, i32::MAX);
}

/// A JMAP string is a JSON string, so a mailbox name may contain a NUL even
/// though RFC 8621 forbids it. A C string may not contain a NUL. Passing the
/// bytes through unchanged would truncate the display name at the NUL: a
/// folder called `Work\0Secret` would show as `Work`, indistinguishable from a
/// real folder named `Work`. The replacement character keeps the name distinct
/// and visibly broken. The path has no such problem, because `jmap-mail-sync`
/// already encodes the NUL.
#[test]
fn a_nul_in_a_mailbox_name_does_not_truncate_the_display_name() {
    let chain = chain(&[mailbox("M1", "Work\0Secret")]);
    let read = read(&chain);

    assert_eq!(read[0].display_name, "Work\u{fffd}Secret");
    assert_eq!(read[0].full_name, "Work%00Secret");
}

/// `CAMEL_STORE_FOLDER_INFO_RECURSIVE` chooses between the whole subtree and
/// one level of it. This function applies the depth at build time. Building
/// the whole forest first and pruning it afterwards would allocate every
/// folder of a deep account only to discard all but the first level.
#[test]
fn no_depth_at_all_emits_every_level() {
    let chain = forest(&three_levels(), None);

    assert_eq!(
        full_names(&read(&chain)),
        ["Parent", "Parent/Child", "Parent/Child/Grandchild"]
    );
}

/// A non-recursive call with no `top` asks for the account's top-level
/// folders, and nothing under them.
#[test]
fn a_depth_of_zero_emits_the_roots_and_nothing_below_them() {
    let chain = forest(&three_levels(), Some(0));

    assert_eq!(full_names(&read(&chain)), ["Parent"]);
}

/// A non-recursive call with depth 1 returns the named folder, plus its
/// immediate subfolders.
#[test]
fn a_depth_of_one_emits_the_immediate_children_and_stops() {
    let chain = forest(&three_levels(), Some(1));

    assert_eq!(full_names(&read(&chain)), ["Parent", "Parent/Child"]);
}

/// The cut applies to what is emitted, not to what the folder *is*. A folder
/// whose children were left out still carries `CHILDREN`, because that flag
/// makes the folder tree draw an expander. The expander is how the user asks
/// for the level that was cut. A cut folder that reported itself as a leaf
/// would hide a subtree nothing could ever fetch.
#[test]
fn a_folder_whose_children_were_cut_still_says_it_has_them() {
    let chain = forest(&three_levels(), Some(0));
    let read = read(&chain);

    assert!(read[0].children.is_empty(), "the cut did not happen");
    assert_ne!(read[0].flags & CAMEL_FOLDER_CHILDREN, 0, "CHILDREN");
    assert_eq!(read[0].flags & CAMEL_FOLDER_NOCHILDREN, 0);
}

/// A chain rooted at one folder, not at the account: the answer to a call with
/// a `top` argument. The folder's siblings are not in the answer, and the
/// folder that is has no `next`. A root chain that still linked the siblings
/// would hand Camel the whole account under the name of a subtree.
#[test]
fn a_forest_of_one_folder_is_a_chain_of_one() {
    let tree = FolderTree::from_mailboxes(&[
        mailbox("M1", "Work"),
        mailbox("M2", "Personal"),
        child("M3", "Invoices", "M1"),
    ])
    .expect("a well-formed mailbox list");
    let work = tree.find("Work").expect("the folder");

    let chain = FolderInfoChain::from_forest(std::slice::from_ref(work), None);

    assert_eq!(full_names(&read(&chain)), ["Work", "Work/Invoices"]);
    // SAFETY: the chain owns the forest just built.
    unsafe {
        assert!((*chain.as_ptr()).next.is_null(), "a sibling came along");
        assert!((*chain.as_ptr()).parent.is_null(), "the root has a parent");
    }
}

/// The other half of the ownership story. `get_folder_info_sync` hands the
/// chain to its caller, who frees it with `camel_folder_info_free`. The wrapper
/// must let go of the chain without also freeing it. If `into_raw` left the
/// `Drop` in place, this would be a double free, so the test performs the
/// caller's free itself.
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

/// A `parentId` chain the server chose can be arbitrarily long. Each layer that
/// walks it recursively uses stack space the server must size: `FolderTree`'s
/// own `Drop`, and `camel_folder_info_free`'s recursion over `child` and
/// `next`. `FolderTree` therefore caps the depth it produces (see
/// `jmap_mail_sync::folder::MAX_DEPTH`). This test checks the C side of that
/// cap: the forest stays broad rather than deep, and no folder is dropped to
/// get there.
#[test]
fn a_deep_parent_chain_becomes_a_broad_forest_rather_than_a_deep_one() {
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
    // Walking it back would recurse in `read_chain`, so count iteratively.
    let mut deepest = 0;
    let mut count = 0;
    let mut roots = 0;
    let mut root = chain.as_ptr();
    while !root.is_null() {
        roots += 1;
        let mut depth = 0;
        let mut info = root;
        while !info.is_null() {
            depth += 1;
            count += 1;
            // SAFETY: `info` is an entry of the chain the wrapper owns.
            info = unsafe { (*info).child };
        }
        deepest = deepest.max(depth);
        // SAFETY: as above.
        root = unsafe { (*root).next };
    }

    assert_eq!(count, DEPTH, "a folder went missing");
    assert_eq!(roots, DEPTH.div_ceil(jmap_mail_sync::folder::MAX_DEPTH));
    assert_eq!(deepest, jmap_mail_sync::folder::MAX_DEPTH);
}
