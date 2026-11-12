// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folder tree: a `Mailbox/get` result in the form that `CamelStore` uses.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::State;
use jmap_proto::mail::{Mailbox, role};
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::path::{SEPARATOR, encode_component, join};
use crate::shared::SharedMailAccount;

/// The path of the synthetic root [`FolderTree::merge_shared`] adds.
const SHARED_ROOT_PATH: &str = "Shared";

/// Whether `id` names a synthetic folder [`FolderTree::merge_shared`] adds:
/// the "Shared" root, or an owner heading below it.
///
/// Neither is a real mailbox. The id routes to no account and no server has
/// ever heard of it (ROADMAP item 179(b), finding F2: a real Evolution sent
/// one to a real JMAP server and got back `invalidArguments: invalid JMAP
/// ID`). A caller that is about to ask the server about a mailbox id must
/// check this first and treat a match as an always-empty folder that never
/// changes.
pub fn is_synthetic_folder_id(id: &Id) -> bool {
    let id = id.as_str();
    id == "shared-root" || id.starts_with("shared-owner:")
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The account this folder belongs to, if not the account that the
    /// enclosing `MailSync`/`CamelJmapStore` is itself configured for.
    ///
    /// `None` for an ordinary folder of the store's own account: the
    /// common case, and every folder [`FolderTree::from_mailboxes`] builds.
    /// [`FolderTree::merge_shared`] sets `Some` on each folder of a shared
    /// account's subtree, so [`FolderTree::owning_account`] can tell a
    /// caller which account to address for that mailbox (ROADMAP item
    /// 163's next slice: routing folder, message and flag operations to
    /// the owning account, not only building the merged tree).
    ///
    /// `#[serde(default)]` lets an on-disk cache written before this field
    /// existed (`folder_tree_to_json`) still deserialise, as a tree with no
    /// shared folders, rather than being discarded as unreadable.
    #[serde(default)]
    pub account_id: Option<Id>,
    pub children: Vec<FolderInfo>,
}

/// How a new listing differs from the held one, in the terms of the folder
/// signals of Camel. [`FolderTree::change_to`] makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderListChange {
    /// The same folders at the same paths. Counts can differ.
    Same,
    /// Only new folders, destroyed folders and subscription changes. Each
    /// folder has its own signal, and the other rows stay as they are.
    Delta(FolderDelta),
    /// A folder has a new path, name or role, or the sibling order changed.
    /// Camel must get the full list again.
    Reshaped,
}

/// The folders of a [`FolderListChange::Delta`]. Each folder has no children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderDelta {
    /// Destroyed folders, as the old listing has them. Children come before
    /// their parents.
    pub destroyed: Vec<FolderInfo>,
    /// New folders. Parents come before their children.
    pub created: Vec<FolderInfo>,
    /// Folders with only a new subscription. Each has the new value.
    pub resubscribed: Vec<FolderInfo>,
}

impl FolderInfo {
    /// A copy of the folder without its children.
    fn leaf(&self) -> Self {
        Self {
            id: self.id.clone(),
            path: self.path.clone(),
            display_name: self.display_name.clone(),
            role: self.role,
            total: self.total,
            unread: self.unread,
            subscribed: self.subscribed,
            may_add_items: self.may_add_items,
            account_id: self.account_id.clone(),
            children: Vec::new(),
        }
    }
}

/// All mailboxes of an account, as a forest.
///
/// JMAP has no root mailbox, so the top-level mailboxes are siblings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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

    /// Merges other accounts' mail into this tree, the shape evolution-ews
    /// and stock IMAP both use for a foreign mailbox (`notes/
    /// SHARED-MAILBOXES-2026-10-30.md` section 4, Option 1): one extra,
    /// always-present "Shared" root, one child per shared account named for
    /// its owner, that account's own folders moved below it unchanged
    /// otherwise.
    ///
    /// `shared` pairs each account [`crate::shared_mail_accounts`] found
    /// with the `FolderTree` that account's own `MailSync` already builds.
    /// An empty `shared` returns `self` unchanged: no "Shared" root appears
    /// unless at least one account shares mail.
    ///
    /// A merged folder keeps the mailbox id and role its own account's
    /// listing gave it, so [`FolderTree::by_id`] still finds it by that id.
    /// [`FolderTree::find`] then searches the whole merged tree, including
    /// shared accounts, which is correct: a user clicks a path under
    /// `Shared/<owner>` the same as any other. [`FolderTree::role`] excludes
    /// a folder this function stamped with `account_id`, so it still answers
    /// only for the store's own account, not a shared one.
    ///
    /// An account whose [`SharedMailAccount::read_only`] is true gets
    /// `may_add_items: Some(false)` on every one of its folders, even one
    /// whose own `myRights` said otherwise: the account-level restriction
    /// cannot be looser than the share itself.
    ///
    /// Every merged folder also gets `subscribed: true`, regardless of the
    /// owner's own `isSubscribed` for it (ROADMAP item 179(a) finding F3): a
    /// grantee often cannot change that flag, and it was never a sign of the
    /// grantee's own wish to see the folder. This is the same "show what the
    /// server grants, regardless of `isSubscribed`" rule item 163 already
    /// applies to shared calendars and address books.
    pub fn merge_shared(mut self, shared: &[(SharedMailAccount, FolderTree)]) -> Self {
        if shared.is_empty() {
            return self;
        }

        // A personal mailbox named "Shared" already took this path: give the
        // synthetic root a disambiguated one instead, the same `%23` marker
        // `paths` uses for a duplicate sibling name (no encoded name ever
        // contains `%23`, because the encoding escapes `%`).
        let shared_root_path = if self.roots.iter().any(|root| root.path == SHARED_ROOT_PATH) {
            format!("{SHARED_ROOT_PATH}%23shared-root")
        } else {
            SHARED_ROOT_PATH.to_owned()
        };

        let mut owner_paths: BTreeSet<String> = BTreeSet::new();
        let owners = shared
            .iter()
            .map(|(account, tree)| {
                let mut path = join(Some(&shared_root_path), &encode_component(&account.name));
                if owner_paths.contains(&path) {
                    path = format!("{path}%23{}", encode_component(account.id.as_str()));
                }
                owner_paths.insert(path.clone());

                let mut children = tree.roots.clone();
                for child in &mut children {
                    if account.read_only {
                        lock_read_only(child);
                    }
                    stamp_account(child, &account.id);
                    force_visible(child);
                    let component = child
                        .path
                        .rsplit(SEPARATOR)
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    let below = join(Some(&path), &component);
                    repath(child, below);
                }
                FolderInfo {
                    id: Id::new(format!("shared-owner:{}", account.id)),
                    path,
                    display_name: account.name.clone(),
                    role: None,
                    total: 0,
                    unread: 0,
                    subscribed: true,
                    may_add_items: Some(false),
                    // Not a real mailbox: no operation can ever address this
                    // synthetic id, so it names no account to route to.
                    account_id: None,
                    children,
                }
            })
            .collect();

        self.roots.push(FolderInfo {
            id: Id::new("shared-root"),
            path: shared_root_path,
            display_name: "Shared".to_owned(),
            role: None,
            total: 0,
            unread: 0,
            subscribed: true,
            may_add_items: Some(false),
            account_id: None,
            children: owners,
        });
        self
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

    /// The change from `self` to `after`, in the terms of the folder signals.
    ///
    /// A folder is the same folder if it has the same mailbox id. A rename or
    /// a move also changes the paths of the descendants, so it gives
    /// [`FolderListChange::Reshaped`].
    pub fn change_to(&self, after: &Self) -> FolderListChange {
        if self.same_shape(after) {
            return FolderListChange::Same;
        }
        let before: BTreeMap<&Id, &FolderInfo> =
            self.iter().map(|folder| (&folder.id, folder)).collect();
        let mut delta = FolderDelta::default();
        for folder in after.iter() {
            match before.get(&folder.id) {
                None => delta.created.push(folder.leaf()),
                Some(old)
                    if old.path != folder.path
                        || old.display_name != folder.display_name
                        || old.role != folder.role =>
                {
                    return FolderListChange::Reshaped;
                }
                Some(old) if old.subscribed != folder.subscribed => {
                    delta.resubscribed.push(folder.leaf());
                }
                Some(_) => {}
            }
        }
        let kept: BTreeSet<&Id> = after.iter().map(|folder| &folder.id).collect();
        delta.destroyed = self
            .iter()
            .filter(|folder| !kept.contains(&folder.id))
            .map(FolderInfo::leaf)
            .collect();
        // The pre-order reversed puts each child before its parent.
        delta.destroyed.reverse();
        // No folder changed, so only the sibling order did.
        if delta == FolderDelta::default() {
            return FolderListChange::Reshaped;
        }
        FolderListChange::Delta(delta)
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

    /// The account that owns the mailbox `id`, if it is not the store's own
    /// account.
    ///
    /// `None` both for an id this tree does not have and for an ordinary
    /// folder of the store's own account: a caller that routes a folder,
    /// message or flag operation (ROADMAP item 163's next slice) treats
    /// `None` as "use the account this `MailSync` already addresses", and
    /// `Some` as "address this account instead, over the same client".
    pub fn owning_account(&self, id: &Id) -> Option<&Id> {
        self.by_id(id)?.account_id.as_ref()
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
    ///
    /// Only the store's own account answers: a folder [`FolderTree::merge_
    /// shared`] stamped with `account_id` never does, even when the store's
    /// own account has no mailbox with that role. Reading a shared account's
    /// role here would file or expunge the user's own mail into a mailbox
    /// another principal owns (SECURITY-AUDIT Area 13, S13-3).
    pub fn role(&self, role: FolderRole) -> Option<&FolderInfo> {
        self.iter()
            .find(|folder| folder.role == Some(role) && folder.account_id.is_none())
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
                // This listing came from the account's own `Mailbox/get`;
                // `merge_shared` is what sets `Some` for a shared account.
                account_id: None,
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

/// A folder tree and the state it answers `Mailbox/changes` with, as bytes
/// for a disk cache. See `jmap-mail::store::JmapStore::folders` (item 176(b)
/// of the roadmap): a store with no connection reads this to answer "what
/// folders are there" from the last successful listing of a previous
/// process, instead of failing outright.
///
/// Serialisation never fails: every field of [`FolderTree`] and [`State`] is
/// a string, a number, a bool or a collection of those. Deserialisation can
/// fail on a file from an older or a newer version of this crate, or one a
/// disk error corrupted; [`folder_tree_from_json`] treats that the same as a
/// missing file, not an error to report.
pub fn folder_tree_to_json(state: &State, tree: &FolderTree) -> String {
    #[derive(Serialize)]
    struct Persisted<'a> {
        state: &'a State,
        tree: &'a FolderTree,
    }
    serde_json::to_string(&Persisted { state, tree })
        .expect("a FolderTree and a State always serialise")
}

/// The inverse of [`folder_tree_to_json`]. `None` for anything that is not
/// exactly what that function wrote, including an empty or truncated file.
pub fn folder_tree_from_json(json: &str) -> Option<(State, FolderTree)> {
    #[derive(Deserialize)]
    struct Persisted {
        state: State,
        tree: FolderTree,
    }
    let persisted: Persisted = serde_json::from_str(json).ok()?;
    Some((persisted.state, persisted.tree))
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

/// Sets `may_add_items` to `Some(false)` on a folder and every folder below
/// it, for [`FolderTree::merge_shared`]'s read-only accounts.
fn lock_read_only(folder: &mut FolderInfo) {
    let mut stack: Vec<&mut FolderInfo> = vec![folder];
    while let Some(folder) = stack.pop() {
        folder.may_add_items = Some(false);
        stack.extend(folder.children.iter_mut());
    }
}

/// Marks `folder` and every folder below it as belonging to `id`, for
/// [`FolderTree::owning_account`].
fn stamp_account(folder: &mut FolderInfo, id: &Id) {
    let mut stack: Vec<&mut FolderInfo> = vec![folder];
    while let Some(folder) = stack.pop() {
        folder.account_id = Some(id.clone());
        stack.extend(folder.children.iter_mut());
    }
}

/// Sets `subscribed` to `true` on a folder and every folder below it, for
/// [`FolderTree::merge_shared`].
///
/// `isSubscribed` on a shared mailbox is the owner's own flag, not the
/// grantee's wish to see it, and a grantee often cannot change it. Camel's
/// `Request` hides an unsubscribed folder's children when Evolution asks
/// with `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`, so without this a shared
/// folder whose owner never subscribed to it would show in the tree with
/// no children: the owner's own folder list decides what the grantee sees,
/// the same bug ROADMAP item 163 already avoids for shared calendars and
/// address books by showing them regardless of `isSubscribed`.
fn force_visible(folder: &mut FolderInfo) {
    let mut stack: Vec<&mut FolderInfo> = vec![folder];
    while let Some(folder) = stack.pop() {
        folder.subscribed = true;
        stack.extend(folder.children.iter_mut());
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
        let mut mailbox = Mailbox::new(name)
            .with_id(Id::from(id))
            .with_total_emails(total)
            .with_unread_emails(unread)
            .is_subscribed(true);
        if let Some(parent) = parent {
            mailbox = mailbox.with_parent_id(Id::from(parent));
        }
        mailbox
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

    /// A mailbox named with a `\r` or `\n` still gets a path `find` can look
    /// up by. `jmap-mail`'s `c_string` rewrites either character to U+FFFD
    /// when the path crosses into Camel (SECURITY-AUDIT item 167(a), Area
    /// 13); before `encode_component` escaped them here too, the path
    /// `find` held never matched the one Camel echoed back.
    #[test]
    fn a_mailbox_named_with_a_control_character_has_a_path_find_can_use() {
        let tree = FolderTree::from_mailboxes(&[mailbox("1", "Inbox\r\nX", None, 0, 0)]).unwrap();

        let folder = tree
            .find("Inbox%0D%0AX")
            .expect("the escaped path is found");
        assert_eq!(folder.display_name, "Inbox\r\nX");
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

    fn ids(folders: &[FolderInfo]) -> Vec<&str> {
        folders.iter().map(|folder| folder.id.as_str()).collect()
    }

    fn delta(before: &[Mailbox], after: &[Mailbox]) -> FolderDelta {
        let before = FolderTree::from_mailboxes(before).unwrap();
        let after = FolderTree::from_mailboxes(after).unwrap();
        match before.change_to(&after) {
            FolderListChange::Delta(delta) => delta,
            other => panic!("expected a delta, got {other:?}"),
        }
    }

    /// New counts only are no change for the folder list.
    #[test]
    fn change_to_ignores_total_and_unread() {
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 3, 1)]).unwrap();
        let after = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 4, 2)]).unwrap();

        assert_eq!(before.change_to(&after), FolderListChange::Same);
    }

    /// A new parent with a new child: the parent comes first, and neither
    /// carries children.
    #[test]
    fn change_to_names_new_folders_parents_first() {
        let delta = delta(
            &[mailbox("1", "Inbox", None, 0, 0)],
            &[
                mailbox("1", "Inbox", None, 0, 0),
                mailbox("2", "Work", None, 0, 0),
                mailbox("3", "Receipts", Some("2"), 0, 0),
            ],
        );

        assert_eq!(ids(&delta.created), ["2", "3"]);
        assert!(
            delta
                .created
                .iter()
                .all(|folder| folder.children.is_empty())
        );
        assert!(delta.destroyed.is_empty() && delta.resubscribed.is_empty());
    }

    /// A destroyed parent with its child: the child comes first.
    #[test]
    fn change_to_names_destroyed_folders_children_first() {
        let delta = delta(
            &[
                mailbox("1", "Inbox", None, 0, 0),
                mailbox("2", "Work", None, 0, 0),
                mailbox("3", "Receipts", Some("2"), 0, 0),
            ],
            &[mailbox("1", "Inbox", None, 0, 0)],
        );

        assert_eq!(ids(&delta.destroyed), ["3", "2"]);
        assert_eq!(delta.destroyed[0].path, "Work/Receipts");
        assert!(delta.created.is_empty() && delta.resubscribed.is_empty());
    }

    /// A subscription change names the folder with its new value.
    #[test]
    fn change_to_names_a_subscription_change() {
        let mut unsubscribed = mailbox("2", "Work", None, 0, 0);
        unsubscribed.is_subscribed = Some(false);
        let delta = delta(
            &[
                mailbox("1", "Inbox", None, 0, 0),
                mailbox("2", "Work", None, 0, 0),
            ],
            &[mailbox("1", "Inbox", None, 0, 0), unsubscribed],
        );

        assert_eq!(ids(&delta.resubscribed), ["2"]);
        assert!(!delta.resubscribed[0].subscribed);
    }

    /// A rename, a move and a new role each reshape the list.
    #[test]
    fn change_to_reshapes_for_a_rename_a_move_or_a_role() {
        let before = [
            mailbox("1", "Work", None, 0, 0),
            mailbox("2", "Personal", None, 0, 0),
            mailbox("3", "Receipts", Some("1"), 0, 0),
        ];
        let mut renamed = before.clone();
        renamed[2].name = "Bills".to_owned();
        let mut moved = before.clone();
        moved[2].parent_id = Some(Id::from("2"));
        let mut role_given = before.clone();
        role_given[1].role = Some(role::ARCHIVE.to_owned());

        let tree = FolderTree::from_mailboxes(&before).unwrap();
        for after in [renamed, moved, role_given] {
            let after = FolderTree::from_mailboxes(&after).unwrap();
            assert_eq!(tree.change_to(&after), FolderListChange::Reshaped);
        }
    }

    /// A new folder beside a rename still reshapes: a partial delta would
    /// leave the renamed row at its old path.
    #[test]
    fn change_to_reshapes_when_a_rename_comes_with_a_new_folder() {
        let before = FolderTree::from_mailboxes(&[mailbox("1", "Work", None, 0, 0)]).unwrap();
        let after = FolderTree::from_mailboxes(&[
            mailbox("1", "Plans", None, 0, 0),
            mailbox("2", "Receipts", None, 0, 0),
        ])
        .unwrap();

        assert_eq!(before.change_to(&after), FolderListChange::Reshaped);
    }

    /// An on-disk listing from before this field existed still deserialises,
    /// as a tree with no shared folders, instead of being discarded as
    /// unreadable (the `#[serde(default)]` on `FolderInfo::account_id`).
    #[test]
    fn folder_tree_from_json_reads_a_listing_with_no_account_id_field() {
        let json = serde_json::json!({
            "state": State::new("s1"),
            "tree": {
                "roots": [{
                    "id": "1",
                    "path": "Inbox",
                    "display_name": "Inbox",
                    "role": null,
                    "total": 3,
                    "unread": 1,
                    "subscribed": true,
                    "may_add_items": null,
                    "children": [],
                }],
            },
        })
        .to_string();

        let (_, tree) = folder_tree_from_json(&json).expect("an old-format listing still reads");
        assert_eq!(tree.by_id(&Id::new("1")).unwrap().account_id, None);
    }

    /// `folder_tree_to_json`/`folder_tree_from_json` round-trip an
    /// `account_id`, the same as every other field.
    #[test]
    fn folder_tree_json_round_trip_keeps_account_id() {
        let shared = FolderTree::from_mailboxes(&[mailbox("S1", "Inbox", None, 0, 0)]).unwrap();
        let tree = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 0, 0)])
            .unwrap()
            .merge_shared(&[(
                SharedMailAccount {
                    id: Id::new("bob"),
                    name: "bob@example.com".to_owned(),
                    read_only: false,
                },
                shared,
            )]);

        let json = folder_tree_to_json(&State::new("s1"), &tree);
        let (_, read_back) = folder_tree_from_json(&json).expect("a written listing reads back");

        assert_eq!(
            read_back.owning_account(&Id::new("S1")),
            Some(&Id::new("bob"))
        );
        assert_eq!(read_back.owning_account(&Id::new("1")), None);
    }

    /// An empty shared `Account.name` must still give a non-empty owner
    /// path, not the bare "Shared" root with a trailing slash:
    /// CROSS-PROTOCOL-AUDIT X5-5(a). Before the fix, `encode_component("")`
    /// returned `""`, so the owner path joined it onto `SHARED_ROOT_PATH` as
    /// `"Shared/"`.
    #[test]
    fn an_empty_shared_account_name_gets_a_non_empty_owner_path() {
        let shared = FolderTree::from_mailboxes(&[mailbox("s1", "Inbox", None, 0, 0)]).unwrap();

        let tree = FolderTree::from_mailboxes(&[]).unwrap().merge_shared(&[(
            SharedMailAccount {
                id: Id::new("acc"),
                name: String::new(),
                read_only: false,
            },
            shared,
        )]);

        let owner = tree
            .iter()
            .find(|folder| folder.id == Id::new("shared-owner:acc"))
            .unwrap();
        assert!(!owner.path.ends_with(SEPARATOR));
        assert_ne!(owner.path, SHARED_ROOT_PATH);
    }

    /// A personal mailbox named "Shared" must not collide with the synthetic
    /// "Shared" root `merge_shared` adds, and two shared accounts with the
    /// same display name must not collide with each other either:
    /// CROSS-PROTOCOL-AUDIT X5-5(b).
    #[test]
    fn merge_shared_dedupes_against_a_personal_shared_folder_and_duplicate_owner_names() {
        let team_a = FolderTree::from_mailboxes(&[mailbox("sa1", "Inbox", None, 0, 0)]).unwrap();
        let team_b = FolderTree::from_mailboxes(&[mailbox("sb1", "Inbox", None, 0, 0)]).unwrap();

        let tree = FolderTree::from_mailboxes(&[mailbox("1", "Shared", None, 0, 0)])
            .unwrap()
            .merge_shared(&[
                (
                    SharedMailAccount {
                        id: Id::new("acc-a"),
                        name: "Team".to_owned(),
                        read_only: false,
                    },
                    team_a,
                ),
                (
                    SharedMailAccount {
                        id: Id::new("acc-b"),
                        name: "Team".to_owned(),
                        read_only: false,
                    },
                    team_b,
                ),
            ]);

        let mut paths = Vec::new();
        for folder in tree.iter() {
            assert!(
                !paths.contains(&folder.path),
                "duplicate path: {}",
                folder.path
            );
            paths.push(folder.path.clone());
        }
        assert_eq!(tree.by_id(&Id::new("sa1")).unwrap().path, {
            let owner_a = tree
                .iter()
                .find(|folder| folder.id == Id::new("shared-owner:acc-a"))
                .unwrap();
            format!("{}/Inbox", owner_a.path)
        });
    }

    /// ROADMAP item 179(a) finding F3 (confirmed live against Stalwart): a
    /// grantee's own `Mailbox/get` for a shared account returns the owner's
    /// `isSubscribed`, not anything about the grantee's own wish to see it,
    /// and a grantee often cannot change it. Camel hides an unsubscribed
    /// folder's children from the tree whenever Evolution asks with
    /// `CAMEL_STORE_FOLDER_INFO_SUBSCRIBED`, so a merged folder must show
    /// regardless of the owner's own flag, not inherit it.
    #[test]
    fn merge_shared_shows_a_folder_the_owner_never_subscribed_to() {
        let shared = FolderTree::from_mailboxes(&[
            Mailbox::new("Team Shared")
                .with_id(Id::from("t"))
                .is_subscribed(false),
            Mailbox::new("Q4")
                .with_id(Id::from("q"))
                .with_parent_id(Id::from("t"))
                .is_subscribed(false),
        ])
        .unwrap();

        let tree = FolderTree::from_mailboxes(&[]).unwrap().merge_shared(&[(
            SharedMailAccount {
                id: Id::new("bob"),
                name: "bob@example.com".to_owned(),
                read_only: false,
            },
            shared,
        )]);

        assert!(tree.by_id(&Id::new("t")).unwrap().subscribed);
        assert!(tree.by_id(&Id::new("q")).unwrap().subscribed);
    }

    /// `role` must never answer with a shared account's folder, even when
    /// the store's own account has no mailbox with that role: a caller that
    /// reads it for the account's own Trash or Junk (SECURITY-AUDIT Area 13,
    /// S13-3) would file or expunge the user's own mail into a mailbox
    /// another principal owns.
    #[test]
    fn role_never_answers_from_a_shared_account() {
        let own = FolderTree::from_mailboxes(&[mailbox("1", "Inbox", None, 0, 0)]).unwrap();
        let shared = FolderTree::from_mailboxes(&[with_role(
            mailbox("s1", "Papierkorb", None, 0, 0),
            role::TRASH,
        )])
        .unwrap();

        let tree = own.merge_shared(&[(
            SharedMailAccount {
                id: Id::new("bob"),
                name: "bob@example.com".to_owned(),
                read_only: false,
            },
            shared,
        )]);

        assert!(tree.role(FolderRole::Trash).is_none());
    }

    fn with_role(mut mailbox: Mailbox, role: &str) -> Mailbox {
        mailbox.role = Some(role.to_owned());
        mailbox
    }
}
