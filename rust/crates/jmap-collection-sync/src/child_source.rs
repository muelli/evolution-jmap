// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings of a child source, and the id that EDS reads back from it.
//!
//! [`crate::children`] names the children that a populate creates.
//! `e_collection_backend_new_child` returns an `ESource` that is *empty*
//! except for its parent and its uid. Thus this code must set all that makes
//! it an address book of this account, not a blank source. This module is
//! that property list, as data.
//!
//! The rules below come from `src/libebackend/e-collection-backend.c` in
//! EDS 3.52, because the installed headers do not state them.
//!
//! ## The resource id is opaque, and EDS does not store it
//!
//! `collection_backend_new_user_file()` names the `.source` file of the child
//! after a new random uid, not after the resource id. EDS uses the resource id
//! only as a `GHashTable` key and in `g_strcmp0` comparisons. Thus it can
//! contain all characters, and the colon in
//! [`ChildKind::resource_id`](crate::ChildKind::resource_id) is safe.
//!
//! But EDS does not *persist* it. On the next start,
//! `collection_backend_load_resources()` reads the cached `.source` files and
//! asks the `dup_resource_id` vfunc for the name of each one. Nothing else
//! keeps the name. Thus the resource id must be a function of the properties
//! of the child source, and the function must give a value for each child.
//! If `dup_resource_id` returns `NULL` for a child, EDS **deletes** its cache
//! file (`remove_redundant`), with the uid and the offline data of the child.
//! [`resource_id_for`] is that function. [`Child::settings`] makes sure that
//! it gives a value for each child.
//!
//! ## Why the identity has no kind
//!
//! EDS has a usual method: `EWebDAVCollectionBackend` writes
//! `"contacts::" + url` into `[Resource] Identity`, and keeps the default
//! `dup_resource_id`, which returns that string unchanged. WebDAV must do
//! this, because a WebDAV URL does not tell if it is an address book or a
//! calendar.
//!
//! A JMAP child tells its kind in its extension. An address book child has
//! `[Address Book]` and a calendar child has `[Calendar]`. EDS uses this
//! pair in `collection_backend_child_is_contacts()` and `…_is_calendar()`.
//! Thus the identity does not need the kind, and it must not have it. The book
//! and calendar backends read `[Resource] Identity` as the JMAP collection id,
//! or as `<accountId>/<collectionId>` for a shared collection
//! ([`jmap-backend-core`'s `SourceConfig`]). The example keyfile in the docs
//! of that module has `Identity=Ab1`. A prefix here gives one field two
//! spellings: one from this backend and one from a hand-written file. The
//! prefix is in the resource id, which is derived.
//!
//! [`jmap-backend-core`'s `SourceConfig`]: ../../jmap_backend_core/source/struct.SourceConfig.html
//!
//! ## What EDS sets, so that this module does not
//!
//! - **`Parent`**: `collection_backend_new_source()` sets it to the uid of the
//!   collection source before it returns the child.
//! - **`Enabled`**: `collection_backend_bind_child_enabled()` *binds* it to
//!   `ESourceCollection:contacts-enabled` / `:calendar-enabled` (and to the
//!   `enabled` of the collection). The binding overwrites a value written
//!   here. Also, the "don't show this account's contacts" choice of the user
//!   then has two locations.
//!
//! Nothing sets the `read_only` of the child. No `ESource` property says
//! "this collection is read-only". The book and calendar backends give
//! writability at runtime. Thus [`Child::read_only`](crate::Child::read_only)
//! is a fact for the backend, not a setting to write.
//!
//! ## What this module copies from the collection, and why
//!
//! A child gets no connection settings from the account. EDS binds only
//! `oauth2-support` (and the display name, for mail children):
//! `collection_backend_child_added()` has the full list. But the JMAP
//! backends get the *child* source and read the server from it. Without
//! `[Authentication] Host`, each operation of the child fails with "the
//! account does not name a JMAP server". `EWebDAVCollectionBackend` copies
//! the same settings, one at a time, for the same reason.
//!
//! This module copies `[Security]` also when it says TLS, although
//! `SourceConfig` reads an absent `[Security]` as TLS. It is the only setting
//! whose omission silently *upgrades* a child above its account. A child of a
//! plain-HTTP account that refuses plain HTTP reports a TLS error that the
//! settings of the account do not explain.

use crate::children::{Child, ChildKind};

/// `ESourceBackend:backend-name` for both kinds of child: the name of the
/// address book factory and of the calendar factory.
pub const BACKEND_NAME: &str = "jmap";

/// The `ESource` extension of an address book child. The spelling is the EDS
/// spelling, because the string *is* the keyfile group and the argument to
/// `e_source_get_extension`.
pub const EXTENSION_ADDRESS_BOOK: &str = "Address Book";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_CALENDAR: &str = "Calendar";

/// The keyfile group of the `ESource` itself. It is not an extension. It is
/// the only one of these six groups with no `E_SOURCE_EXTENSION_*` constant,
/// because EDS spells it in `e-source.c`, not in a header.
pub const EXTENSION_DATA_SOURCE: &str = "Data Source";
/// See [`EXTENSION_ADDRESS_BOOK`]: the group is the extension name.
pub const EXTENSION_RESOURCE: &str = "Resource";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_AUTHENTICATION: &str = "Authentication";
/// See [`EXTENSION_ADDRESS_BOOK`].
pub const EXTENSION_SECURITY: &str = "Security";

/// Where the collection source says its server is.
///
/// The five settings that a child must repeat to reach the server of its
/// account. The caller reads them from `[Authentication]` and `[Security]` of
/// the collection. This is not a URL: `SourceConfig` makes the origin in the
/// backend, and a second assembly here can give a different result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// `[Authentication] Host`.
    pub host: String,
    /// `[Authentication] Port`, when the account names one. `None` leaves the
    /// port of the child at zero, which means "the default of the scheme".
    pub port: Option<u16>,
    /// `[Authentication] User`.
    pub user: Option<String>,
    /// `[Authentication] Method`: how EDS gets the credentials, not a
    /// credential. The child copies the value of the collection unchanged. A
    /// different value makes EDS ask for the password in a different way.
    pub auth_method: Option<String>,
    /// `[Security] Method`, as the boolean `ESourceSecurity:secure` reads it.
    pub secure: bool,
}

/// One property to set on a child `ESource`.
///
/// The names are the keyfile spelling, for example
/// `("Address Book", "BackendName")`. This is the extension name for
/// `e_source_get_extension` and the group in a hand-written `.source`. Thus one
/// description is correct for the vfunc and for the documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// The `ESource` extension, i.e. the keyfile group.
    pub group: &'static str,
    /// The property, with the keyfile spelling.
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
    /// The `ESource` extension of a child of this kind.
    ///
    /// It has two functions. It tells EDS that the source is an address book
    /// or a calendar. It is also the half of the resource id that is not the
    /// identity (see [`resource_id_for`]).
    pub fn extension(self) -> &'static str {
        match self {
            Self::AddressBook => EXTENSION_ADDRESS_BOOK,
            Self::Calendar => EXTENSION_CALENDAR,
        }
    }

    /// The kind of a source with `extension`, or `None` for a group that is
    /// not a kind of this backend.
    ///
    /// The inverse of [`ChildKind::extension`], and deliberately the only one.
    /// [`resource_id_for`] reads a child that this backend wrote. A
    /// `create_resource_sync` reads a *scratch* source that has no identity
    /// yet, so it cannot use [`resource_id_for`]. Two matches over the same two
    /// constants can become different.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            EXTENSION_ADDRESS_BOOK => Some(Self::AddressBook),
            EXTENSION_CALENDAR => Some(Self::Calendar),
            _ => None,
        }
    }
}

impl Child {
    /// All settings for the source that `e_collection_backend_new_child`
    /// returns, in order.
    ///
    /// The list is complete. With all of these, the related JMAP backend can
    /// connect, and [`resource_id_for`] reads the source back as this child.
    pub fn settings(&self, connection: &Connection) -> Vec<Setting> {
        let mut settings = vec![
            Setting::new(EXTENSION_DATA_SOURCE, "DisplayName", &*self.display_name),
            Setting::new(self.kind.extension(), "BackendName", BACKEND_NAME),
            // Without this, the child has no resource id, and EDS deletes it
            // from the cache (see the module comment).
            Setting::new(EXTENSION_RESOURCE, "Identity", self.identity()),
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
        // Only a calendar can have a color. As for `Port`, `User` and
        // `Method`, an absent value is not written, not written empty.
        if self.kind == ChildKind::Calendar
            && let Some(color) = &self.color
        {
            settings.push(Setting::new(EXTENSION_CALENDAR, "Color", &**color));
        }
        settings
    }
}

/// The resource id of a child source with `extension` and `identity`: the
/// return value of the `dup_resource_id` vfunc.
///
/// The inverse of [`Child::settings`] over the two fields that stay after a
/// restart. Thus [`Child::settings`] always writes these two. `None` is a
/// source that this backend did not create, and the vfunc returns `NULL` for
/// it. For a source that this backend *did* create, `NULL` makes EDS delete
/// the cache file. Thus each child of this crate must give its resource id
/// back.
pub fn resource_id_for(extension: &str, identity: &str) -> Option<String> {
    if identity.is_empty() {
        return None;
    }
    let kind = ChildKind::from_extension(extension)?;
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
            remote_deletable: true,
        }
    }

    /// The settings as `(group, key, value)`, for easy comparison.
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
        // `collection_backend_child_is_calendar()` looks for the extension. The
        // calendar factory and the book factory have the same backend name.
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
        // `SourceConfig` reads `[Resource] Identity` as the collection id. The
        // kind is in the resource id, which is derived, not stored.
        let child = child(ChildKind::AddressBook, "AB1", "Personal");
        let settings = child.settings(&connection());

        assert!(triples(&settings).contains(&("Resource", "Identity", "AB1")));
        assert_eq!(child.resource_id, "addressbook:AB1");
    }

    #[test]
    fn a_child_source_reads_back_as_the_resource_id_it_was_created_under() {
        // At each start, EDS asks `dup_resource_id` for each cached `.source`.
        // A different answer gives a second source for the collection, and
        // `NULL` deletes the cache file.
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
        // EDS asks `dup_resource_id` about each cached child of this
        // collection. For a source of a different backend, `NULL` tells EDS
        // that this backend does not own it.
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
        // EDS binds only `oauth2-support`. Without a host, each operation of
        // the child fails with "the account does not name a JMAP server".
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
        // `SourceConfig` reads an *absent* `[Security]` as TLS. Without this
        // setting, a child of `jmap-mockd` does not work.
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
        // EDS sets `Parent` and binds `Enabled`. The binding overwrites a
        // written value (see the module comment).
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

        // `Resource::color` is always `None` for an address book, but
        // `settings` also examines the kind. `jmap-backend-collection`'s
        // `child_source::apply` has no property for `[Address Book] Color`.
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
