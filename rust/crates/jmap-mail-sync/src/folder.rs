// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder tree: a `Mailbox/get` result in the form that `CamelStore` uses.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::mail::{Mailbox, role};

use crate::error::SyncError;
use crate::path::{SEPARATOR, encode_component, join};

/// The maximum depth of a `parentId` chain. A deeper mailbox becomes
/// top-level.
///
/// The build and the walk are iterative, but a [`FolderInfo`] owns a
/// `Vec<FolderInfo>`, so drop glue (and the derived `Clone` and `PartialEq`)
/// recurses once per level. The server selects the `parentId`s. A chain of
/// some tens of thousands of mailboxes overflows the stack and aborts the
/// process, without `unsafe` code. In Camel, that process serves all the mail
/// accounts of the user.
///
/// A deep mailbox becomes top-level, as a mailbox in a `parentId` loop does,
/// so no mailbox is lost. Real mail stores do not go near this depth.
pub const MAX_DEPTH: usize = 64;

/// The purpose of a folder for Evolution.
///
/// Only the roles that this crate uses. The other roles of the RFC 8457
/// registry (`\Important`, `\Flagged`, …) are views, not folder types of
/// Camel, so they become plain folders.
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
    /// The role that a JMAP `role` value names, if this crate uses it.
    ///
    /// The comparison ignores case, although RFC 8621 §2 requires lower
    /// case. Thus the inbox of a server that sends `Inbox` is still the inbox
    /// in Evolution.
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

    /// The JMAP `role` value, in the lower case that RFC 8621 §2 requires.
    ///
    /// The inverse of [`from_jmap`](Self::from_jmap). Error messages use it
    /// when the account has no mailbox with a role that Camel asks for (for
    /// example, trash or junk).
    pub fn as_jmap(self) -> &'static str {
        match self {
            Self::Inbox => role::INBOX,
            Self::Drafts => role::DRAFTS,
            Self::Sent => role::SENT,
            Self::Trash => role::TRASH,
            Self::Junk => role::JUNK,
            Self::Archive => role::ARCHIVE,
        }
    }
}

/// One folder, with its subfolders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// The JMAP mailbox id: the filter of each `Email/query`.
    pub id: Id,
    /// The Camel path and the key of the folder: the encoded names of the
    /// ancestors and of this folder, as `path.rs` describes.
    pub path: String,
    /// The mailbox name without changes, as the user sees it.
    pub display_name: String,
    pub role: Option<FolderRole>,
    /// The number of messages in the folder and the number of unread
    /// messages. Camel uses 32 bits for these.
    pub total: u32,
    pub unread: u32,
    pub subscribed: bool,
    /// `Mailbox.myRights.mayAddItems` (RFC 8621 §2): the right to file a
    /// message into this mailbox.
    ///
    /// `None` when the server said nothing about it. Treat `None` as
    /// allowed: most servers omit `myRights` for a mailbox the caller owns,
    /// and reading that silence as a refusal would mark every ordinary
    /// folder of every account read-only.
    pub may_add_items: Option<bool>,
    pub children: Vec<FolderInfo>,
}

/// All mailboxes of an account, as a forest.
///
/// JMAP has no root mailbox, so the top-level mailboxes are siblings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderTree {
    roots: Vec<FolderInfo>,
}

/// A validated mailbox and its id.
struct Node<'a> {
    mailbox: &'a Mailbox,
    id: &'a Id,
}

impl FolderTree {
    /// Build the tree from a `Mailbox/get` list.
    ///
    /// The list comes from the server without changes. Thus most of this
    /// function handles input that a correct server does not send: a
    /// `parentId` of a mailbox that is not in the list, or a `parentId` loop.
    /// Such a mailbox becomes top-level, so no mailbox is lost and nothing
    /// hangs. A mailbox without an id or name, or a duplicate id, is an error.
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

        // Sibling order, which is also the order of role claims and of the
        // walk. RFC 8621 §2 sorts by sortOrder, then name. The id breaks a
        // tie, so the result does not depend on the order of the reply.
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

        // A parent that is not in the list is no parent: the mailbox becomes
        // top-level. A mailbox that is its own parent is a cycle of one, and
        // `walk` cuts it.
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

    /// The top-level folders of the account, in sibling order.
    pub fn roots(&self) -> &[FolderInfo] {
        &self.roots
    }

    /// All folders, depth first, each parent before its children.
    pub fn iter(&self) -> impl Iterator<Item = &FolderInfo> {
        Iter {
            stack: self.roots.iter().rev().collect(),
        }
    }

    /// Whether `self` and `other` have the same folders at the same paths,
    /// with the same names, roles and subscriptions. This ignores `total` and
    /// `unread`.
    ///
    /// A pushed `Mailbox` state change does not tell which mailbox or property
    /// changed, and a new message changes the state as a create or a rename
    /// does. The refresh of the open folder sends counts to Camel, so a caller
    /// uses this to decide whether the folder list of Camel is out of date.
    pub fn same_shape(&self, other: &Self) -> bool {
        same_shape(&self.roots, &other.roots)
    }

    /// The folder at a Camel path, if the account has one.
    pub fn find(&self, path: &str) -> Option<&FolderInfo> {
        self.iter().find(|folder| folder.path == path)
    }

    /// The folder with a mailbox id, if the account has one.
    ///
    /// By id, not by path. [`set_subscribed`](Self::set_subscribed) uses the
    /// same key for the same reason: the caller names a mailbox, not a place
    /// in the tree.
    pub fn by_id(&self, id: &Id) -> Option<&FolderInfo> {
        self.iter().find(|folder| folder.id == *id)
    }

    /// The folder with a role, if a mailbox of the account has that role.
    ///
    /// Callers use it for the inbox, trash and junk lookups of Camel, and for
    /// the Sent and Drafts folders. The lookup examines the full tree:
    /// RFC 8621 §2 does not limit `role` to the top level, and an inbox under
    /// a parent folder is usual.
    ///
    /// It reads the role that `claim_roles` assigned, not the `role` property
    /// of the mailbox. `claim_roles` gives a role to the first claimant only,
    /// so this result agrees with the folder types of the listing.
    pub fn role(&self, role: FolderRole) -> Option<&FolderInfo> {
        self.iter().find(|folder| folder.role == Some(role))
    }

    /// Sets the subscription of a mailbox. Returns `false` if the tree does not
    /// have the mailbox.
    ///
    /// The server accepted the write, so the caller knows the new value. A new
    /// listing is not necessary. The edits below exist for the same reason.
    ///
    /// The key is the mailbox id, not the path: paths are an invention of this
    /// crate, and the path changes if a parent gets a new name.
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

    /// Adds a new folder of the account. Returns `false` if the tree has no
    /// parent for it.
    ///
    /// The server confirmed the folder, so a new listing is not necessary.
    /// Without this, the store cannot open the folder that it just gave to
    /// Evolution.
    ///
    /// The position comes from the path of the folder: the path is the parent
    /// path, a separator and one encoded component (see `paths`). A component
    /// cannot contain the separator, so the last separator splits the path.
    /// There is no parent argument that can disagree with the path.
    ///
    /// If no folder has the parent path, this inserts nothing. A folder at the
    /// top level of an account that has it at a different position is worse
    /// than no folder until the next listing.
    ///
    /// A sibling with the same path (and its subtree) is removed. The tree can
    /// still hold a mailbox that another client deleted, and a create can use
    /// its name again. Of the two, keep the folder that the server confirmed.
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

    /// Moves a folder to its new path. Returns `false` if the tree cannot put
    /// the folder there.
    ///
    /// The path of a folder is its position, so this also changes the paths
    /// of all folders below it. A remove and an insert cannot do that without
    /// a new listing.
    ///
    /// A rename and a move are one operation, because Camel gives one new path
    /// and does not tell which part changed.
    ///
    /// `display_name` is an argument because this crate has no decoder for
    /// the encoded last component. The caller has the name that it sent to
    /// the server, which is the name of the next listing.
    ///
    /// The new parent comes from the new path, as in [`insert`](Self::insert),
    /// with the same refusals: if no folder has the parent path, nothing
    /// moves, and a folder at the destination path is removed. A move into
    /// the subtree of the folder is also refused. The server refuses it too,
    /// and the tree has no position for the subtree after that move.
    pub fn rename(&mut self, id: &Id, path: &str, display_name: &str) -> bool {
        let Some(from) = self.iter().find(|folder| folder.id == *id) else {
            return false;
        };
        let from = from.path.clone();

        // Do the checks before the folder leaves the tree. Otherwise a refused
        // folder is lost.
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

        // The destination parent existed before `take`, and `take` removed only
        // this folder, which is not an ancestor of the destination. Thus
        // `insert` does not refuse. If it does, the folder stays out of the
        // tree until the next listing, as `insert` does for a missing parent.
        self.insert(folder)
    }

    /// Removes a folder from the tree. Returns `false` if the tree does not
    /// have it.
    ///
    /// The key is the mailbox id, as in [`set_subscribed`](Self::set_subscribed).
    ///
    /// This also removes the subtree. RFC 8621 §2.5 makes the server refuse to
    /// destroy a mailbox with children, so after a successful destroy the
    /// server had no children of it. Children in this tree are mailboxes that
    /// another client removed, and their parent path is now invalid.
    pub fn remove(&mut self, id: &Id) -> bool {
        self.take(id).is_some()
    }

    /// Removes a folder and returns it. [`rename`](Self::rename) uses it to
    /// move a subtree.
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

    /// The folder at a Camel path, for an edit. The read-only version is
    /// [`find`](Self::find). This one is private: a `&mut FolderInfo` lets the
    /// caller change a path or an id, which is the structure of the tree.
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

    /// The number of folders of the account, at all depths.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Give each role to the first mailbox that claims it.
    ///
    /// Two inboxes are a server error, but `camel_store_get_inbox_folder_sync`
    /// needs one answer. The order is the sort order of `nodes` (sortOrder,
    /// name, id), so the result is the same each time.
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

    /// Depth-first pre-order of the forest. This cuts cycles.
    ///
    /// After the walk from the real roots, an unvisited mailbox is in a
    /// `parentId` loop or below one. The walk cuts the parent link of the
    /// first unvisited mailbox and walks it as a root, with its subtree and the
    /// remainder of its cycle. Each pass visits at least one more mailbox, so
    /// the loop ends. After the loop, `parent` and `children` agree with the
    /// returned order.
    ///
    /// A chain deeper than [`MAX_DEPTH`] is cut in the same way. The cut
    /// mailbox becomes a root and its subtree counts depth again from 1, so no
    /// mailbox is lost and the depth has a limit.
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
                    // Too deep: the mailbox becomes top-level, as in a loop.
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

    /// The Camel path of each mailbox, with the indices of `nodes`.
    ///
    /// In `order`, so the path of a parent is known before its children.
    /// Distinct mailboxes get distinct paths. The encoding is injective, so
    /// only an illegal duplicate sibling name causes a collision. Then the
    /// path gets `%23` and the encoded id. No encoded name contains `%23`,
    /// because the encoding escapes `%`.
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

    /// Make owned [`FolderInfo`]s from the walk order.
    ///
    /// In reverse order, so the children of a folder are complete before the
    /// folder: in a pre-order, they come after it. Each list of children is
    /// then reversed again to restore sibling order.
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
                // A server without subscriptions sends no value. If that means
                // "not subscribed", all mail of the account is hidden.
                subscribed: mailbox.is_subscribed.unwrap_or(true),
                may_add_items: mailbox
                    .my_rights
                    .as_ref()
                    .and_then(|rights| rights.may_add_items),
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

/// Whether `path` is `ancestor` or below it.
///
/// A prefix test is sufficient, because each path is the parent path with one
/// more component. The match must include the separator: "Work" is not below
/// "Wo".
fn within(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|below| below.starts_with(SEPARATOR))
}

/// Sets `path` on a folder and makes the paths of all folders below it again.
///
/// The path of a child is the parent path and the encoded name of the child.
/// Only the last component of the old path is still correct after a move.
/// [`FolderTree::insert`] reads a path the same way.
///
/// Iterative, for the reason of [`MAX_DEPTH`]: the server selects the depth.
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

/// A JMAP count in the 32 bits of Camel. Saturating: a count that wraps to a
/// small number makes a full folder look almost empty.
fn saturate(count: Option<u64>) -> u32 {
    u32::try_from(count.unwrap_or(0)).unwrap_or(u32::MAX)
}

/// [`FolderTree::same_shape`] for two lists of siblings, recursively. The
/// sibling order of [`FolderTree::from_mailboxes`] does not use counts, so two
/// listings of the same mailboxes have the same order.
fn same_shape(left: &[FolderInfo], right: &[FolderInfo]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id == right.id
                && left.path == right.path
                && left.display_name == right.display_name
                && left.role == right.role
                && left.subscribed == right.subscribed
                && same_shape(&left.children, &right.children)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox(id: &str, name: &str, parent: Option<&str>, total: u64, unread: u64) -> Mailbox {
        Mailbox {
            id: Some(Id::from(id)),
            name: name.to_owned(),
            parent_id: parent.map(Id::from),
            total_emails: Some(total),
            unread_emails: Some(unread),
            is_subscribed: Some(true),
            ..Mailbox::default()
        }
    }

    /// A delivery changes the counts of mailboxes, and that is not a shape
    /// change. Refer to [`FolderTree::same_shape`].
    #[test]
    fn same_shape_ignores_total_and_unread() {
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();
        let after = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 4, 2)]).unwrap();

        assert!(before.same_shape(&after));
    }

    /// A new mailbox is a shape change.
    #[test]
    fn same_shape_notices_a_new_mailbox() {
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();
        let after = FolderTree::from_mailboxes(&[
            mailbox("1", "Inbox", None, 3, 1),
            mailbox("2", "Receipts", None, 0, 0),
        ])
        .unwrap();

        assert!(!before.same_shape(&after));
    }

    /// A destroyed mailbox is a shape change too.
    #[test]
    fn same_shape_notices_a_destroyed_mailbox() {
        let before = FolderTree::from_mailboxes(&[
            mailbox("1", "Inbox", None, 3, 1),
            mailbox("2", "Receipts", None, 0, 0),
        ])
        .unwrap();
        let after = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();

        assert!(!before.same_shape(&after));
    }

    /// A rename changes `display_name` and `path`. Each is a shape change.
    #[test]
    fn same_shape_notices_a_rename() {
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();
        let after = FolderTree::from_mailboxes(&[mailbox("1", "Archive", None, 3, 1)]).unwrap();

        assert!(!before.same_shape(&after));
    }

    /// A move to a new parent changes `path` without touching `display_name`.
    #[test]
    fn same_shape_notices_a_move() {
        let before = FolderTree::from_mailboxes(&[
            mailbox("1", "Work", None, 0, 0),
            mailbox("2", "Personal", None, 0, 0),
            mailbox("3", "Receipts", Some("1"), 3, 1),
        ])
        .unwrap();
        let after = FolderTree::from_mailboxes(&[
            mailbox("1", "Work", None, 0, 0),
            mailbox("2", "Personal", None, 0, 0),
            mailbox("3", "Receipts", Some("2"), 3, 1),
        ])
        .unwrap();

        assert!(!before.same_shape(&after));
    }

    /// A subscription change is a shape change: Camel filters the tree on
    /// `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`.
    #[test]
    fn same_shape_notices_a_subscription_change() {
        let mut unsubscribed = mailbox("1", "Inbox", None, 3, 1);
        unsubscribed.is_subscribed = Some(false);
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();
        let after = FolderTree::from_mailboxes(&[unsubscribed]).unwrap();

        assert!(!before.same_shape(&after));
    }
}
