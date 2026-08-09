// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder tree: a `Mailbox/get` result in the shape `CamelStore` asks for.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

use crate::error::SyncError;
use crate::path::{SEPARATOR, encode_component, join};

/// How deep a `parentId` chain may make the tree before the chain is cut.
///
/// Building the tree is iterative and so is walking it, so neither is what
/// needs a limit. The tree itself is: a [`FolderInfo`] owns a
/// `Vec<FolderInfo>`, so the drop glue recurses once per level, and the depth
/// comes from `parentId`s a *server* chose. A few tens of thousands of
/// mailboxes in one chain therefore abort the process — "thread has overflowed
/// its stack" — on a path with no `unsafe` in it at all, and in Camel's case
/// that is a process serving every other mail account the user has.
///
/// Cutting the chain rather than rejecting the account is the same answer this
/// module already gives a `parentId` loop: no mailbox is lost, it just becomes
/// top-level. The number is far above what a mail store uses — Camel's own
/// folder paths become unusable long before it — so nothing real is reshaped.
pub const MAX_DEPTH: usize = 64;

/// The purpose Evolution treats a folder as having.
///
/// Only the roles this crate can act on: everything else in the RFC 8457
/// registry (`\Important`, `\Flagged`, …) describes a view rather than a
/// folder Camel has a type for, and is left as a plain folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FolderRole {
    Inbox,
    Drafts,
    Sent,
    Trash,
    Junk,
    Archive,
}

impl FolderRole {
    /// The role a JMAP `role` property names, if it is one we act on.
    ///
    /// Case-insensitively, though RFC 8621 §2 requires the value to be
    /// lower-case: a server that shouts `Inbox` is broken, but the user's
    /// inbox should still land in Evolution's inbox rather than becoming an
    /// ordinary folder.
    pub fn from_jmap(role: &str) -> Option<Self> {
        match role.to_ascii_lowercase().as_str() {
            role::INBOX => Some(Self::Inbox),
            role::DRAFTS => Some(Self::Drafts),
            role::SENT => Some(Self::Sent),
            role::TRASH => Some(Self::Trash),
            role::JUNK => Some(Self::Junk),
            role::ARCHIVE => Some(Self::Archive),
            _ => None,
        }
    }
}

/// One folder, with its subfolders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// The JMAP mailbox id — what every `Email/query` filters on.
    pub id: Id,
    /// The Camel path: this folder's key, and the concatenation of its
    /// ancestors' encoded names, encoded as `path.rs` describes.
    pub path: String,
    /// The mailbox name, verbatim — what the user is shown.
    pub display_name: String,
    pub role: Option<FolderRole>,
    /// Messages in the folder, and how many of them are unread. Camel counts
    /// these in 32 bits.
    pub total: u32,
    pub unread: u32,
    pub subscribed: bool,
    pub children: Vec<FolderInfo>,
}

/// Every mailbox of an account, as a forest.
///
/// A forest and not a tree: JMAP has no root mailbox, so an account's
/// top-level mailboxes are siblings of each other and of nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderTree {
    roots: Vec<FolderInfo>,
}

/// A validated mailbox, with the fields the ordering needs pulled out.
struct Node<'a> {
    mailbox: &'a Mailbox,
    id: &'a Id,
}

impl FolderTree {
    /// Build the tree from a `Mailbox/get` list.
    ///
    /// The list is taken as it comes off the wire, which means most of this
    /// function is about inputs a correct server never sends: a `parentId`
    /// naming a mailbox that is not in the list, or a chain of them that
    /// loops. Neither may lose a mailbox — a folder missing from the tree is
    /// mail the user cannot reach — and neither may hang, so both end up as
    /// extra top-level folders. Only a violation that leaves nothing to show
    /// at all (a mailbox with no id or no name, an id used twice) is an error.
    pub fn from_mailboxes(mailboxes: &[Mailbox]) -> Result<Self, SyncError> {
        let mut nodes = Vec::with_capacity(mailboxes.len());
        for mailbox in mailboxes {
            let id = mailbox.id.as_ref().ok_or_else(|| {
                SyncError::protocol("Mailbox/get returned a mailbox without an id")
            })?;
            if mailbox.name.is_empty() {
                return Err(SyncError::protocol(format!(
                    "Mailbox/get returned mailbox {id} without a name"
                )));
            }
            nodes.push(Node { mailbox, id });
        }

        // Sibling order, which is also the order roles are claimed in and the
        // order the tree is walked in. RFC 8621 §2 defines it as sortOrder
        // first, then the name; the id breaks the remaining tie so that the
        // result does not depend on the order the server happened to reply in.
        nodes.sort_by(|left, right| {
            left.sort_order()
                .cmp(&right.sort_order())
                .then_with(|| left.mailbox.name.cmp(&right.mailbox.name))
                .then_with(|| left.id.cmp(right.id))
        });

        let mut index_of: BTreeMap<&Id, usize> = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if index_of.insert(node.id, index).is_some() {
                return Err(SyncError::protocol(format!(
                    "Mailbox/get returned mailbox {} twice",
                    node.id
                )));
            }
        }

        // A parent this account cannot see is no parent: the mailbox becomes
        // top-level rather than unreachable. A mailbox that is its own parent
        // needs no case of its own — it is a cycle of one, and `walk` cuts it.
        let mut parent: Vec<Option<usize>> = nodes
            .iter()
            .map(|node| {
                node.mailbox
                    .parent_id
                    .as_ref()
                    .and_then(|parent_id| index_of.get(parent_id).copied())
            })
            .collect();

        let roles = Self::claim_roles(&nodes);

        let mut children: Vec<Vec<usize>> = nodes.iter().map(|_| Vec::new()).collect();
        let mut roots: Vec<usize> = Vec::new();
        for index in 0..nodes.len() {
            match parent[index] {
                Some(parent) => children[parent].push(index),
                None => roots.push(index),
            }
        }

        let order = Self::walk(&mut parent, &mut children, &roots);
        let paths = Self::paths(&nodes, &parent, &order);
        Ok(Self {
            roots: Self::assemble(&nodes, &parent, &roles, &paths, &order),
        })
    }

    /// The account's top-level folders, in sibling order.
    pub fn roots(&self) -> &[FolderInfo] {
        &self.roots
    }

    /// Every folder, depth first, a parent before its children.
    pub fn iter(&self) -> impl Iterator<Item = &FolderInfo> {
        Iter {
            stack: self.roots.iter().rev().collect(),
        }
    }

    /// The folder at a Camel path, if the account has one.
    pub fn find(&self, path: &str) -> Option<&FolderInfo> {
        self.iter().find(|folder| folder.path == path)
    }

    /// The folder a role names, if any mailbox of the account claims it.
    ///
    /// The role that has a caller today is [`FolderRole::Inbox`], which is what
    /// `camel_store_get_inbox_folder_sync` is answered from. The lookup walks
    /// the whole tree rather than the roots: RFC 8621 §2 puts `role` on the
    /// mailbox, with nothing said about where in the hierarchy it sits, and an
    /// account whose inbox hangs under a per-address parent is ordinary.
    ///
    /// It reads the role this crate *assigned* — see
    /// [`claim_roles`](Self::claim_roles), which gives a contested role to the
    /// first mailbox in sibling order — rather than the mailbox's own `role`
    /// property. That is what keeps the answer the same folder as the one the
    /// listing marked `CAMEL_FOLDER_TYPE_INBOX`.
    pub fn role(&self, role: FolderRole) -> Option<&FolderInfo> {
        self.iter().find(|folder| folder.role == Some(role))
    }

    /// Records that a mailbox is now subscribed, or is not, reporting whether
    /// the tree had it at all.
    ///
    /// The first edit this type offers, and it exists because the property is
    /// the one thing in a folder listing a *client* decides. Everything else
    /// here is the server's — counts, roles, where a mailbox hangs — and the
    /// honest way to bring any of it up to date is to list the account again. A
    /// subscription is different: the client has just been told what the new
    /// value is by the server accepting its write, so re-listing to learn it
    /// would be asking a question already answered — which is the reason every
    /// edit below it is here too.
    ///
    /// By mailbox id rather than by path, because the caller is a write that
    /// named a mailbox and paths are this crate's invention: a folder whose
    /// parent was renamed between the listing and the write has a different
    /// path and the same id.
    pub fn set_subscribed(&mut self, id: &Id, subscribed: bool) -> bool {
        let mut stack: Vec<&mut FolderInfo> = self.roots.iter_mut().collect();
        while let Some(folder) = stack.pop() {
            if folder.id == *id {
                folder.subscribed = subscribed;
                return true;
            }
            stack.extend(folder.children.iter_mut());
        }
        false
    }

    /// Adds a folder the account has just gained, reporting whether the tree
    /// had somewhere to put it.
    ///
    /// The second edit this type offers, and it is here for the reason the
    /// first one is: the caller has just been told by the server that the
    /// folder exists, so re-listing the account to learn it would be asking a
    /// question already answered — and until something asks, a store holding a
    /// listing without the new folder in it is a store that cannot open the
    /// folder Evolution was just handed.
    ///
    /// Where it goes is read out of its own path rather than passed in beside
    /// it. A folder's path *is* its position — it is its parent's path with one
    /// encoded component after it, which is the invariant
    /// [`paths`](Self::paths) maintains for every folder here — and a component
    /// can never contain the separator, so the last one splits the path
    /// unambiguously. A caller that passed the parent separately could pass one
    /// that disagrees with the path; there is no such second version to
    /// disagree with.
    ///
    /// A parent path no folder answers to inserts nothing. Hanging the folder
    /// off the roots instead would draw it at the top level of an account that
    /// has it somewhere else entirely, which is worse than not drawing it until
    /// the next listing.
    ///
    /// A sibling that already has the new folder's path is dropped. The tree
    /// can be out of date in exactly one direction the server will not correct
    /// — it can still hold a mailbox another client destroyed, whose name is
    /// then free for this create to reuse — and of the two folders with that
    /// one path, the one the server has just confirmed is the one to keep.
    pub fn insert(&mut self, folder: FolderInfo) -> bool {
        let siblings = match folder.path.rsplit_once(SEPARATOR) {
            Some((parent, _)) => match self.find_mut(parent) {
                Some(parent) => &mut parent.children,
                None => return false,
            },
            None => &mut self.roots,
        };

        siblings.retain(|sibling| sibling.path != folder.path);
        siblings.push(folder);
        true
    }

    /// Moves a folder to the path it now has, reporting whether the tree could
    /// put it there.
    ///
    /// The third edit, and the one that could not be spelled as the other two:
    /// a folder's path is *where it is*, so renaming one rewrites the path of
    /// everything under it as well, and a remove followed by an insert would
    /// have to rebuild that subtree by hand from a listing nobody has taken.
    ///
    /// A rename and a move are one operation here because they are one to the
    /// caller — Camel names a folder by path, so the folder's name and its
    /// parent both live in the string it is handed, and it never says which of
    /// the two the user changed.
    ///
    /// `display_name` comes beside the path rather than out of it. The path's
    /// last component is the name with this crate's encoding applied, and this
    /// crate has no decoder — while the caller has the name it just sent to the
    /// server, which is the same name the next listing will report.
    ///
    /// Where the folder ends up is read out of the new path, exactly as
    /// [`insert`](Self::insert) reads one, and the two refusals are the same:
    /// a parent path no folder answers to moves nothing, and a folder already
    /// at the destination path is dropped in favour of this one. A move *into*
    /// the folder's own subtree is refused too — the server will not do it, and
    /// a tree that tried would be lifting a subtree out and having nowhere left
    /// to put it back.
    pub fn rename(&mut self, id: &Id, path: &str, display_name: &str) -> bool {
        let Some(from) = self.iter().find(|folder| folder.id == *id) else {
            return false;
        };
        let from = from.path.clone();

        // Both checks happen before the folder is lifted out, because a folder
        // taken out of the tree and refused at its destination is one that has
        // simply disappeared.
        if let Some((parent, _)) = path.rsplit_once(SEPARATOR)
            && (within(parent, &from) || self.find(parent).is_none())
        {
            return false;
        }

        let Some(mut folder) = self.take(id) else {
            return false;
        };
        folder.display_name = display_name.to_owned();
        repath(&mut folder, path.to_owned());

        // The destination was there a moment ago and the only thing removed
        // since is this folder, which the check above kept out of its own
        // subtree — so this cannot be the refusal `insert` reports. If it
        // somehow were, the folder waits for the next listing rather than
        // hanging at the top level of an account that has it elsewhere, which
        // is the judgement `insert` already makes.
        self.insert(folder)
    }

    /// Takes a folder out of the tree, reporting whether it was in it.
    ///
    /// By mailbox id, and for the reason [`set_subscribed`](Self::set_subscribed)
    /// is: the caller is a write that named a mailbox.
    ///
    /// Whatever hung under it goes too. RFC 8621 §2.5 has the server refuse to
    /// destroy a mailbox that still has children, so a destroy that succeeded
    /// says the server had none — and the children this tree still lists are
    /// ones another client removed first. Keeping them would leave folders
    /// under a parent that is not there, at paths nothing answers to.
    pub fn remove(&mut self, id: &Id) -> bool {
        self.take(id).is_some()
    }

    /// The same, handing back what was taken: what [`rename`](Self::rename)
    /// needs, since a move is a removal that puts the subtree down again
    /// somewhere else.
    fn take(&mut self, id: &Id) -> Option<FolderInfo> {
        let mut stack: Vec<&mut Vec<FolderInfo>> = vec![&mut self.roots];
        while let Some(siblings) = stack.pop() {
            if let Some(index) = siblings.iter().position(|folder| folder.id == *id) {
                return Some(siblings.remove(index));
            }
            stack.extend(siblings.iter_mut().map(|folder| &mut folder.children));
        }
        None
    }

    /// The folder at a Camel path, to be edited. The read-only half is
    /// [`find`](Self::find), and this one is deliberately not public: handing
    /// out a `&mut FolderInfo` would be handing out the ability to edit a path
    /// or an id, which is the tree's own structure rather than a property of a
    /// folder.
    fn find_mut(&mut self, path: &str) -> Option<&mut FolderInfo> {
        let mut stack: Vec<&mut FolderInfo> = self.roots.iter_mut().collect();
        while let Some(folder) = stack.pop() {
            if folder.path == path {
                return Some(folder);
            }
            stack.extend(folder.children.iter_mut());
        }
        None
    }

    /// How many folders the account has, at any depth.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Assign each role to the first mailbox claiming it.
    ///
    /// Two inboxes is a server bug, and picking one is the only way to answer
    /// `camel_store_get_inbox_folder`. First in sibling order, so the choice
    /// is at least the same on every client run.
    fn claim_roles(nodes: &[Node<'_>]) -> Vec<Option<FolderRole>> {
        let mut claimed: BTreeSet<FolderRole> = BTreeSet::new();
        nodes
            .iter()
            .map(|node| {
                node.mailbox
                    .role
                    .as_deref()
                    .and_then(FolderRole::from_jmap)
                    .filter(|role| claimed.insert(*role))
            })
            .collect()
    }

    /// Depth-first pre-order over the forest, cutting cycles as it finds them.
    ///
    /// A mailbox left unvisited after walking the real roots is in a
    /// `parentId` loop, or hangs off one. Cutting the first such mailbox's
    /// parent link makes it a root, which walks it and everything below it —
    /// including the rest of its cycle, since the cut is the only link
    /// removed. Repeating until nothing is unvisited terminates because each
    /// pass cuts one mailbox, and leaves a forest that mirrors the tree this
    /// returns an order for.
    ///
    /// A chain longer than [`MAX_DEPTH`] is cut the same way and for a reason
    /// given there: the tree this describes is dropped recursively, so its depth
    /// is a stack the server picks the size of. The cut mailbox becomes a root
    /// and its own subtree starts counting again, so nothing is lost and the
    /// depth of what comes out is bounded.
    fn walk(
        parent: &mut [Option<usize>],
        children: &mut [Vec<usize>],
        roots: &[usize],
    ) -> Vec<usize> {
        let mut visited = vec![false; parent.len()];
        let mut order = Vec::with_capacity(parent.len());
        let mut stack: Vec<(usize, usize)> = roots.iter().rev().map(|&index| (index, 1)).collect();

        let mut cut = 0;
        loop {
            while let Some((index, depth)) = stack.pop() {
                if visited[index] {
                    continue;
                }
                let depth = if depth > MAX_DEPTH {
                    // Too deep: this mailbox becomes top-level, exactly as one
                    // in a `parentId` loop does.
                    if let Some(deep_parent) = parent[index].take() {
                        children[deep_parent].retain(|&child| child != index);
                    }
                    1
                } else {
                    depth
                };
                visited[index] = true;
                order.push(index);
                stack.extend(
                    children[index]
                        .iter()
                        .rev()
                        .map(|&child| (child, depth + 1)),
                );
            }
            let Some(orphan) = (cut..visited.len()).find(|&index| !visited[index]) else {
                return order;
            };
            cut = orphan + 1;
            if let Some(looping_parent) = parent[orphan].take() {
                children[looping_parent].retain(|&child| child != orphan);
            }
            stack.push((orphan, 1));
        }
    }

    /// The Camel path of every mailbox, indexed like `nodes`.
    ///
    /// In `order`, so a parent's path is known before its children need it.
    /// Distinct mailboxes get distinct paths: the encoding is injective, so
    /// the only way two siblings can collide is the illegal duplicate name,
    /// and that one is settled with the id — which no encoded name can end in,
    /// because `%` in a name is escaped.
    fn paths(nodes: &[Node<'_>], parent: &[Option<usize>], order: &[usize]) -> Vec<String> {
        let mut paths = vec![String::new(); nodes.len()];
        let mut used: BTreeSet<String> = BTreeSet::new();
        for &index in order {
            let parent_path = parent[index].map(|parent| paths[parent].as_str());
            let component = encode_component(&nodes[index].mailbox.name);
            let mut path = join(parent_path, &component);
            if used.contains(&path) {
                path = join(
                    parent_path,
                    &format!(
                        "{component}%23{}",
                        encode_component(nodes[index].id.as_str())
                    ),
                );
            }
            used.insert(path.clone());
            paths[index] = path;
        }
        paths
    }

    /// Turn the walked order into owned [`FolderInfo`]s.
    ///
    /// Backwards, so that a folder's children are finished before it is: in a
    /// pre-order they all come after it. Sibling order is restored on the way
    /// out, since collecting from the reversed walk reverses it.
    fn assemble(
        nodes: &[Node<'_>],
        parent: &[Option<usize>],
        roles: &[Option<FolderRole>],
        paths: &[String],
        order: &[usize],
    ) -> Vec<FolderInfo> {
        let mut built: Vec<Vec<FolderInfo>> = nodes.iter().map(|_| Vec::new()).collect();
        let mut roots: Vec<FolderInfo> = Vec::new();

        for &index in order.iter().rev() {
            let mut children = std::mem::take(&mut built[index]);
            children.reverse();
            let mailbox = nodes[index].mailbox;
            let folder = FolderInfo {
                id: nodes[index].id.clone(),
                path: paths[index].clone(),
                display_name: mailbox.name.clone(),
                role: roles[index],
                total: saturate(mailbox.total_emails),
                unread: saturate(mailbox.unread_emails),
                // A server that does not model subscriptions reports nothing
                // here, and treating that as "not subscribed" would hide the
                // whole account's mail.
                subscribed: mailbox.is_subscribed.unwrap_or(true),
                children,
            };
            match parent[index] {
                Some(parent) => built[parent].push(folder),
                None => roots.push(folder),
            }
        }

        roots.reverse();
        roots
    }
}

impl Node<'_> {
    fn sort_order(&self) -> u32 {
        self.mailbox.sort_order.unwrap_or(0)
    }
}

/// Whether `path` is `ancestor` or sits somewhere below it.
///
/// A prefix test, and it is one because that is what a path here *is*: every
/// folder's is its parent's with one more component after it, so the strings
/// answer the question about the tree. The separator has to be part of the
/// match — "Work" is not below "Wo".
fn within(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|below| below.starts_with(SEPARATOR))
}

/// Writes `path` onto a folder and rebuilds every path below it.
///
/// Structurally rather than by rewriting a prefix: a child's path is its
/// parent's plus its own encoded name, and that last component is the only part
/// of the old path still worth anything once the folder has moved. It is the
/// same reading of a path [`FolderTree::insert`] makes.
///
/// Iterative, for [`MAX_DEPTH`]'s reason turned around — the depth of a tree is
/// a number a server chose, and this walk should not be one more thing that
/// makes it a stack.
fn repath(folder: &mut FolderInfo, path: String) {
    let mut stack: Vec<(&mut FolderInfo, String)> = vec![(folder, path)];
    while let Some((folder, path)) = stack.pop() {
        let FolderInfo {
            path: at, children, ..
        } = folder;
        *at = path;
        for child in children.iter_mut() {
            let component = child
                .path
                .rsplit(SEPARATOR)
                .next()
                .unwrap_or_default()
                .to_owned();
            let below = join(Some(at), &component);
            stack.push((child, below));
        }
    }
}

/// A JMAP count in Camel's 32 bits. Saturating, because a count that wraps to
/// a small number reads as a nearly-empty folder.
fn saturate(count: Option<u64>) -> u32 {
    u32::try_from(count.unwrap_or(0)).unwrap_or(u32::MAX)
}

struct Iter<'a> {
    stack: Vec<&'a FolderInfo>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a FolderInfo;

    fn next(&mut self) -> Option<Self::Item> {
        let folder = self.stack.pop()?;
        self.stack.extend(folder.children.iter().rev());
        Some(folder)
    }
}
