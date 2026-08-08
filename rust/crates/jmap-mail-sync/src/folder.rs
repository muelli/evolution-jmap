// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder tree: a `Mailbox/get` result in the shape `CamelStore` asks for.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

use crate::error::SyncError;
use crate::path::{encode_component, join};

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
    fn walk(
        parent: &mut [Option<usize>],
        children: &mut [Vec<usize>],
        roots: &[usize],
    ) -> Vec<usize> {
        let mut visited = vec![false; parent.len()];
        let mut order = Vec::with_capacity(parent.len());
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();

        let mut cut = 0;
        loop {
            while let Some(index) = stack.pop() {
                if visited[index] {
                    continue;
                }
                visited[index] = true;
                order.push(index);
                stack.extend(children[index].iter().rev().copied());
            }
            let Some(orphan) = (cut..visited.len()).find(|&index| !visited[index]) else {
                return order;
            };
            cut = orphan + 1;
            if let Some(looping_parent) = parent[orphan].take() {
                children[looping_parent].retain(|&child| child != orphan);
            }
            stack.push(orphan);
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
