// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings a child source is written with, and the id it is read back by.
//!
//! [`crate::children`] names the children a populate creates.
//! `e_collection_backend_new_child` hands back an `ESource` that is *empty*
//! except for its parent and its uid, so everything that makes it an address
//! book of this account rather than a blank source has to be set on it. This
//! module is that property list, as data.
//!
//! The rules below are read off EDS 3.52's own
//! `src/libebackend/e-collection-backend.c`, because the installed headers do
//! not state any of them.
//!
//! ## The resource id is opaque, and it is not stored
//!
//! `collection_backend_new_user_file()` names the child's `.source` file after
//! a freshly generated uid, not after the resource id, and the resource id is
//! only ever a `GHashTable` key compared with `g_strcmp0`. So there is no
//! character it may not contain — the colon in
//! [`ChildKind::resource_id`](crate::ChildKind::resource_id) is safe, which is
//! the open question the previous increment left.
//!
//! What it is not is *persisted*. On the next start `collection_backend_load_resources()`
//! reads the cached `.source` files back and asks the `dup_resource_id` vfunc
//! what each one is called; nothing else remembers. The resource id therefore
//! has to be a function of the child source's own properties — and a total one:
//! a child whose `dup_resource_id` answers `NULL` is not merely unrecognised,
//! its cache file is **deleted** (`remove_redundant`), taking the child's uid
//! and its offline data with it. [`resource_id_for`] is that function, and
//! [`Child::settings`] is what makes it total.
//!
//! ## Why the identity stays the bare JMAP id
//!
//! The obvious move is EDS's own: `EWebDAVCollectionBackend` writes
//! `"contacts::" + url` into `[Resource] Identity` and leaves `dup_resource_id`
//! at its default, which returns that string verbatim. It has to — a WebDAV
//! URL alone does not say whether it is the address book or the calendar.
//!
//! A JMAP child does say, in the extension it carries: an address book child
//! has `[Address Book]`, a calendar child has `[Calendar]`, and that is
//! precisely the pair EDS itself keys `collection_backend_child_is_contacts()`
//! and `…_is_calendar()` off. So the kind need not go into the identity, and it
//! must not: `[Resource] Identity` is the field the book and calendar backends
//! already read as *the JMAP object id* ([`jmap-backend-core`'s
//! `SourceConfig`]), and the hand-written `.source` in
//! `docs/examples/jmap-mock.source` says `Identity=Ab1`. Prefixing it here
//! would leave one field with two spellings, one written by this backend and
//! one by hand. The prefix lives in the resource id, which is derived.
//!
//! [`jmap-backend-core`'s `SourceConfig`]: ../../jmap_backend_core/source/struct.SourceConfig.html
//!
//! ## What EDS sets, and this therefore does not
//!
//! - **`Parent`** — `collection_backend_new_source()` sets it to the collection
//!   source's uid before the child is ever handed out.
//! - **`Enabled`** — `collection_backend_bind_child_enabled()` *binds* it to
//!   `ESourceCollection:contacts-enabled` / `:calendar-enabled` (and the
//!   collection's own `enabled`). Writing it here would be overwritten by the
//!   binding, and would put the user's "don't show this account's contacts"
//!   choice in two places.
//!
//! And what nothing sets: the child's `read_only`. No `ESource` property says
//! "this collection is read-only" — writability is a runtime answer the book
//! and calendar backends give, so [`Child::read_only`](crate::Child::read_only)
//! stays a fact for the backend to act on rather than a setting to write.
//!
//! ## What is copied from the collection, and why it has to be
//!
//! A child inherits nothing of the account's connection: EDS binds only
//! `oauth2-support` (and the display name, for mail children). Its own
//! `collection_backend_child_added()` is the whole list. But the JMAP backends
//! are handed the *child* source and read the server off it, so a child without
//! `[Authentication] Host` is a child whose every operation fails with "the
//! account does not name a JMAP server". `EWebDAVCollectionBackend` copies the
//! same settings for the same reason, one by one.
//!
//! `[Security]` is copied even when it says TLS, though `SourceConfig` reads an
//! absent `[Security]` as TLS anyway: it is the one setting whose omission
//! would silently *upgrade* a child past the account it belongs to, and a
//! child of a plain-HTTP account that refuses to talk plain HTTP is a child
//! that reports a TLS error the account's own settings do not explain.

use crate::children::{Child, ChildKind};

/// `ESourceBackend:backend-name` for both kinds of child: the name the address
/// book and calendar factories are registered under.
pub const BACKEND_NAME: &str = "jmap";

/// The `ESource` extension a child of that kind carries — spelled as EDS
/// spells it, since the string *is* the keyfile group and the argument to
/// `e_source_get_extension`.
pub const EXTENSION_ADDRESS_BOOK: &str = "Address Book";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_CALENDAR: &str = "Calendar";

/// The `ESource`'s own keyfile group — the one that is not an extension, and
/// the only one of these five with no `E_SOURCE_EXTENSION_*` constant behind
/// it, because EDS spells it in `e-source.c` rather than in a header.
pub const EXTENSION_DATA_SOURCE: &str = "Data Source";
/// See [`EXTENSION_ADDRESS_BOOK`]: the group is the extension name.
pub const EXTENSION_RESOURCE: &str = "Resource";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_AUTHENTICATION: &str = "Authentication";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_SECURITY: &str = "Security";

/// Where the collection source says its server is.
///
/// The four settings a child has to repeat in order to reach the same server as
/// the account it belongs to, read off the collection's own `[Authentication]`
/// and `[Security]` by the caller. Not a URL: `SourceConfig` assembles the
/// origin at the far end, and assembling it twice is two chances to disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// `[Authentication] Host`.
    pub host: String,
    /// `[Authentication] Port`, when the account names one. `None` leaves the
    /// child's port at zero, which is "the scheme's default".
    pub port: Option<u16>,
    /// `[Authentication] User`.
    pub user: Option<String>,
    /// `[Authentication] Method` — how EDS is to obtain the credentials, not a
    /// credential. Copied verbatim because it is the collection's answer and a
    /// child that answers differently gets asked for a password differently.
    pub auth_method: Option<String>,
    /// `[Security] Method`, as the boolean `ESourceSecurity:secure` reads it.
    pub secure: bool,
}

/// One property to set on a child `ESource`.
///
/// Named the way the keyfile spells it — `("Address Book", "BackendName")` —
/// because that is both the extension name `e_source_get_extension` takes and
/// the group a hand-written `.source` uses, so one description serves the vfunc
/// and the documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// The `ESource` extension, i.e. the keyfile group.
    pub group: &'static str,
    /// The property, spelled as the keyfile spells it.
    pub key: &'static str,
    pub value: String,
}

impl Setting {
    fn new(group: &'static str, key: &'static str, value: impl Into<String>) -> Self {
        Self {
            group,
            key,
            value: value.into(),
        }
    }
}

impl ChildKind {
    /// The `ESource` extension a child of this kind carries.
    ///
    /// Load-bearing twice over: it is what makes the source an address book
    /// rather than a calendar to EDS, and it is the half of the resource id
    /// that is not the identity — see [`resource_id_for`].
    pub fn extension(self) -> &'static str {
        match self {
            Self::AddressBook => EXTENSION_ADDRESS_BOOK,
            Self::Calendar => EXTENSION_CALENDAR,
        }
    }
}

impl Child {
    /// Everything to set on the source `e_collection_backend_new_child` returns.
    ///
    /// In order, and complete: a source that gets all of these is one the
    /// matching JMAP backend can connect with, and one that
    /// [`resource_id_for`] reads back as this child.
    pub fn settings(&self, connection: &Connection) -> Vec<Setting> {
        let mut settings = vec![
            Setting::new(EXTENSION_DATA_SOURCE, "DisplayName", &*self.display_name),
            Setting::new(self.kind.extension(), "BackendName", BACKEND_NAME),
            // Without this the child has no resource id, and a child with no
            // resource id is one EDS deletes from the cache — see the module
            // comment.
            Setting::new(EXTENSION_RESOURCE, "Identity", self.collection_id.as_str()),
            Setting::new(EXTENSION_AUTHENTICATION, "Host", &*connection.host),
        ];
        if let Some(port) = connection.port {
            settings.push(Setting::new(
                EXTENSION_AUTHENTICATION,
                "Port",
                port.to_string(),
            ));
        }
        if let Some(user) = &connection.user {
            settings.push(Setting::new(EXTENSION_AUTHENTICATION, "User", &**user));
        }
        if let Some(method) = &connection.auth_method {
            settings.push(Setting::new(EXTENSION_AUTHENTICATION, "Method", &**method));
        }
        settings.push(Setting::new(
            EXTENSION_SECURITY,
            "Method",
            if connection.secure { "tls" } else { "none" },
        ));
        // Only a calendar has a color to begin with (`Resource::color` is
        // always `None` for an address book), and only when the server named
        // one — an absent color is left unwritten rather than written empty,
        // the same rule `Port`/`User`/`Method` above follow.
        if self.kind == ChildKind::Calendar
            && let Some(color) = &self.color
        {
            settings.push(Setting::new(EXTENSION_CALENDAR, "Color", &**color));
        }
        settings
    }
}

/// The resource id a child source carrying `extension` and `identity` was
/// created under — what the `dup_resource_id` vfunc answers.
///
/// The inverse of [`Child::settings`] over the two fields that survive a
/// restart, and the reason those two are always written. `None` is a source
/// this backend did not create, which the vfunc turns into `NULL`; for a source
/// it *did* create that would be an instruction to EDS to throw the cache file
/// away, so every child this crate describes must round-trip.
pub fn resource_id_for(extension: &str, identity: &str) -> Option<String> {
    if identity.is_empty() {
        return None;
    }
    let kind = match extension {
        EXTENSION_ADDRESS_BOOK => ChildKind::AddressBook,
        EXTENSION_CALENDAR => ChildKind::Calendar,
        _ => return None,
    };
    Some(format!("{}:{identity}", kind.prefix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use jmap_proto::Id;

    use crate::children::parse_resource_id;

    fn connection() -> Connection {
        Connection {
            host: "jmap.example.com".to_owned(),
            port: Some(8443),
            user: Some("vera@example.com".to_owned()),
            auth_method: Some("plain/password".to_owned()),
            secure: true,
        }
    }

    fn child(kind: ChildKind, collection: &str, name: &str) -> Child {
        Child {
            resource_id: kind.resource_id(&Id::new(collection)),
            kind,
            display_name: name.to_owned(),
            account_id: Id::new("A1"),
            collection_id: Id::new(collection),
            is_default: false,
            color: None,
            read_only: false,
        }
    }

    /// The settings as `(group, key, value)`, which is how they read.
    fn triples(settings: &[Setting]) -> Vec<(&str, &str, &str)> {
        settings
            .iter()
            .map(|setting| (setting.group, setting.key, setting.value.as_str()))
            .collect()
    }

    #[test]
    fn an_address_book_child_carries_the_address_book_extension_and_the_account() {
        let child = child(ChildKind::AddressBook, "AB1", "Personal");

        assert_eq!(
            triples(&child.settings(&connection())),
            [
                ("Data Source", "DisplayName", "Personal"),
                ("Address Book", "BackendName", "jmap"),
                ("Resource", "Identity", "AB1"),
                ("Authentication", "Host", "jmap.example.com"),
                ("Authentication", "Port", "8443"),
                ("Authentication", "User", "vera@example.com"),
                ("Authentication", "Method", "plain/password"),
                ("Security", "Method", "tls"),
            ]
        );
    }

    #[test]
    fn a_calendar_child_differs_only_in_the_extension_it_is_written_under() {
        // The extension is what makes it a calendar to EDS — it is what
        // `collection_backend_child_is_calendar()` looks for — and the calendar
        // factory is registered under the same backend name as the book one.
        let settings = child(ChildKind::Calendar, "Cal1", "Work").settings(&connection());

        assert!(triples(&settings).contains(&("Calendar", "BackendName", "jmap")));
        assert!(
            !settings
                .iter()
                .any(|setting| setting.group == "Address Book"),
            "a calendar child that also carries [Address Book] is a calendar \
             EDS hands to the address book factory"
        );
    }

    #[test]
    fn the_identity_is_the_jmap_id_and_not_the_resource_id() {
        // One field, one meaning: `SourceConfig` reads `[Resource] Identity` as
        // the id to call `AddressBook/get` with, and the hand-written source in
        // docs/examples spells it that way. The kind lives in the resource id,
        // which is derived rather than stored.
        let child = child(ChildKind::AddressBook, "AB1", "Personal");
        let settings = child.settings(&connection());

        assert!(triples(&settings).contains(&("Resource", "Identity", "AB1")));
        assert_eq!(child.resource_id, "addressbook:AB1");
    }

    #[test]
    fn a_child_source_reads_back_as_the_resource_id_it_was_created_under() {
        // The round trip EDS performs on every start: it loads the cached
        // `.source` files and asks `dup_resource_id` what each one is. An
        // answer that does not match the string the child was created under is
        // a second source for a collection that already has one — and no answer
        // at all deletes the cache file.
        for (kind, collection) in [
            (ChildKind::AddressBook, "AB1"),
            (ChildKind::Calendar, "Cal1"),
            (ChildKind::AddressBook, "AB1"), // an id both kinds share
            (ChildKind::Calendar, "AB1"),
        ] {
            let child = child(kind, collection, "Whatever");
            let settings = child.settings(&connection());
            let identity = settings
                .iter()
                .find(|setting| (setting.group, setting.key) == ("Resource", "Identity"))
                .expect("every child is written with an identity");

            assert_eq!(
                resource_id_for(kind.extension(), &identity.value),
                Some(child.resource_id.clone())
            );
            assert_eq!(
                parse_resource_id(&child.resource_id),
                Some((kind, Id::new(collection))),
                "and the same string still parses back to the child it names"
            );
        }
    }

    #[test]
    fn a_source_this_backend_did_not_create_has_no_resource_id() {
        // `dup_resource_id` is asked about every child of this collection, and
        // will be asked about the mail children once they exist. Claiming one
        // is claiming a source that is not ours; the `NULL` it becomes is EDS's
        // "not one of yours".
        for (extension, identity) in [
            ("Mail Account", "A1"),
            ("Task List", "T1"),
            ("Address Book", ""),
            ("Calendar", ""),
            ("", ""),
        ] {
            assert_eq!(
                resource_id_for(extension, identity),
                None,
                "{extension:?}/{identity:?} was read as a child of ours"
            );
        }
    }

    #[test]
    fn a_child_repeats_the_server_its_account_named() {
        // A child inherits none of this: EDS binds `oauth2-support` and
        // nothing else. A child without a host is one whose every operation
        // fails with "the account does not name a JMAP server".
        let settings = child(ChildKind::AddressBook, "AB1", "Personal").settings(&Connection {
            host: "127.0.0.1".to_owned(),
            port: Some(31415),
            user: None,
            auth_method: None,
            secure: false,
        });

        assert_eq!(
            triples(&settings),
            [
                ("Data Source", "DisplayName", "Personal"),
                ("Address Book", "BackendName", "jmap"),
                ("Resource", "Identity", "AB1"),
                ("Authentication", "Host", "127.0.0.1"),
                ("Authentication", "Port", "31415"),
                ("Security", "Method", "none"),
            ],
            "a setting the account does not state is left unwritten rather \
             than written empty"
        );
    }

    #[test]
    fn a_plain_http_account_does_not_get_children_that_insist_on_tls() {
        // `SourceConfig` reads an *absent* `[Security]` as TLS, so this is the
        // one setting whose omission would upgrade the child past its account
        // — and against `jmap-mockd`, past working at all.
        let child = child(ChildKind::AddressBook, "AB1", "Personal");
        let mut plain = connection();
        plain.secure = false;

        assert!(triples(&child.settings(&plain)).contains(&("Security", "Method", "none")));
        assert!(
            triples(&child.settings(&connection())).contains(&("Security", "Method", "tls")),
            "and a TLS account's children still say so explicitly"
        );
    }

    #[test]
    fn no_child_writes_its_own_enabled_flag() {
        // `collection_backend_bind_child_enabled()` binds `enabled` to
        // `ESourceCollection:contacts-enabled` / `:calendar-enabled`. Writing
        // it here would be overwritten, and would put the user's "don't show
        // this account's contacts" choice in two places.
        let settings = child(ChildKind::AddressBook, "AB1", "Personal").settings(&connection());

        assert!(
            !settings
                .iter()
                .any(|setting| setting.key == "Enabled" || setting.key == "Parent"),
            "EDS sets Parent and binds Enabled; a backend that writes either \
             is fighting it"
        );
    }

    #[test]
    fn a_calendars_color_is_written_and_an_address_books_never_is() {
        let mut calendar = child(ChildKind::Calendar, "Cal1", "Work");
        calendar.color = Some("#ff8800".to_owned());

        assert!(
            triples(&calendar.settings(&connection())).contains(&("Calendar", "Color", "#ff8800"))
        );

        // An address book's `color` is always `None` in practice (see
        // `Resource::color`), but the emitter itself gates on the kind rather
        // than trusting that — a `Setting` under `[Address Book] Color` is one
        // `jmap-backend-collection::child_source::apply` has no property for.
        let mut book = child(ChildKind::AddressBook, "AB1", "Personal");
        book.color = Some("#ff8800".to_owned());
        assert!(
            !book
                .settings(&connection())
                .iter()
                .any(|setting| setting.key == "Color"),
            "an address book child must never carry a Color setting"
        );
    }

    #[test]
    fn a_calendar_the_server_named_no_color_for_writes_none() {
        let settings = child(ChildKind::Calendar, "Cal1", "Work").settings(&connection());
        assert!(
            !settings.iter().any(|setting| setting.key == "Color"),
            "an unset color is left unwritten rather than written empty"
        );
    }
}
