// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `CamelProvider` is the first struct that the mail provider gives to C by value.
// Most types that this crate crosses are GObjects, and `tests/layout.rs` checks
// their layout against `g_type_query`. A provider is not a GObject: it is a plain
// struct behind a boxed GType, so GObject has no size to query. This file uses a
// round trip instead: build a provider in Rust, register it, and get it back from
// Camel. A field at the wrong offset gives back incorrect values.

use eds_sys::*;
use std::ptr;

/// `camel_provider_get_type()` registers a *boxed* type, so `g_type_query()`
/// gives zero for both sizes. In `tests/layout.rs`, `assert_layout!` would compare
/// `size_of` against zero and always fail, so this check is in this file.
#[test]
fn a_provider_is_a_boxed_type_so_gtype_knows_nothing_of_its_size() {
    // SAFETY: a plain type accessor, and `g_type_query` fills a struct that this
    // test owns.
    let (fundamental, query) = unsafe {
        let gtype = camel_provider_get_type();
        let mut query = std::mem::zeroed::<GTypeQuery>();
        g_type_query(gtype, &mut query);
        (g_type_fundamental(gtype), query)
    };
    assert_eq!(fundamental, gobject_sys::G_TYPE_BOXED);
    assert_eq!(query.instance_size, 0, "a boxed type has no instance size");
    assert_eq!(query.class_size, 0, "a boxed type has no class");
}

/// The provider struct is a static description. The test builds it as the real
/// module does: `'static` C string literals, no allocation, no transfer of
/// ownership. `camel_provider_register` keeps the pointer, so the provider of the
/// module must live longer than the module.
#[test]
fn a_provider_built_in_rust_reads_back_out_of_camels_registry() {
    // `camel_provider_init()` scans the provider directory. A loaded
    // `libcameljmap.so` is in this state, because Camel calls
    // `camel_provider_module_init` from inside `camel_provider_init()`.
    // Registration works without it (Camel creates the table lazily, checked), but
    // this test uses the same state as the module.
    // SAFETY: no arguments, and repeated calls are harmless (checked).
    let store_type = unsafe {
        camel_provider_init();
        camel_offline_store_get_type()
    };

    // No other provider registers this protocol, so the test cannot collide with
    // a provider that Camel loads from disk.
    let protocol = c"jmap-eds-sys-roundtrip";
    let mut provider = CamelProvider {
        protocol: protocol.as_ptr(),
        name: c"JMAP round trip".as_ptr(),
        description: c"only ever registered by this test".as_ptr(),
        domain: c"mail".as_ptr(),
        flags: (CAMEL_PROVIDER_IS_REMOTE | CAMEL_PROVIDER_IS_SOURCE) as CamelProviderFlags,
        url_flags: CAMEL_URL_ALLOW_USER as CamelProviderURLFlags,
        extra_conf: ptr::null_mut(),
        port_entries: ptr::null_mut(),
        // EDS 3.58 removed `auto_detect`, and 3.60 `url_hash` and `url_equal`.
        // A provider there has no place for these callbacks.
        // See `docs/eds-version-matrix.md`.
        #[cfg(camel_provider_auto_detect)]
        auto_detect: None,
        // CamelProviderType is the array index: a store and no transport.
        object_types: [store_type, G_TYPE_INVALID],
        authtypes: ptr::null_mut(),
        #[cfg(camel_provider_url_helpers)]
        url_hash: None,
        #[cfg(camel_provider_url_helpers)]
        url_equal: None,
        translation_domain: ptr::null(),
        priv_: ptr::null_mut(),
    };

    // SAFETY: `provider` stays valid for the call, and all that it points to is
    // 'static. `camel_provider_get` returns the registered pointer, or NULL with
    // `error` set. On success, that pointer is the struct above.
    unsafe {
        camel_provider_register(&mut provider);

        let mut error: *mut GError = ptr::null_mut();
        let found = camel_provider_get(protocol.as_ptr(), &mut error);
        assert!(error.is_null(), "camel_provider_get set an error");
        assert!(!found.is_null(), "the provider we just registered is gone");

        assert_eq!(std::ffi::CStr::from_ptr((*found).protocol), protocol);
        assert_eq!(
            std::ffi::CStr::from_ptr((*found).name),
            c"JMAP round trip",
            "the name field reads back from the wrong offset"
        );
        assert_eq!(std::ffi::CStr::from_ptr((*found).domain), c"mail");
        assert_eq!(
            (*found).flags & CAMEL_PROVIDER_IS_REMOTE as CamelProviderFlags,
            CAMEL_PROVIDER_IS_REMOTE as CamelProviderFlags
        );
        assert_eq!(
            (*found).object_types[CAMEL_PROVIDER_STORE as usize],
            store_type
        );
        assert_eq!(
            (*found).object_types[CAMEL_PROVIDER_TRANSPORT as usize],
            G_TYPE_INVALID,
            "a store-only provider must leave the transport slot invalid"
        );
    }
}

/// `object_types` is a fixed-size array in the struct. Its length is an enum value
/// in a different header. A third provider type in EDS would move `authtypes` and
/// all fields after it. Thus the test pins the length and does not trust bindgen
/// to follow the enum.
#[test]
fn the_object_types_array_is_indexed_by_provider_type() {
    assert_eq!(CAMEL_NUM_PROVIDER_TYPES, 2);
    assert_eq!(CAMEL_PROVIDER_STORE, 0);
    assert_eq!(CAMEL_PROVIDER_TRANSPORT, 1);

    let provider = unsafe { std::mem::zeroed::<CamelProvider>() };
    assert_eq!(
        provider.object_types.len(),
        CAMEL_NUM_PROVIDER_TYPES as usize
    );
}

/// The manual test recipe uses this variable to point Camel at an uninstalled
/// build. Camel gives this string to `g_getenv` as the key, so a typo makes Camel
/// never find the provider. Take the string from the header.
#[test]
fn the_provider_directory_override_is_the_variable_camel_reads() {
    assert_eq!(EDS_CAMEL_PROVIDER_DIR, c"EDS_CAMEL_PROVIDER_DIR");
}

/// `CamelFolderInfo` is the second struct that this layer gives to C by value.
/// Like `CamelProvider`, it is not a GObject: `camel_folder_info_get_type` is a
/// boxed type, so there is no instance size to compare. The builder in `jmap-mail`
/// uses the allocator contract instead: `camel_folder_info_new` returns a struct
/// with all fields zeroed. Thus the builder sets only the fields it has data for,
/// and `next`, `parent` and `child` are NULL, not garbage. If upstream used
/// `g_slice_new` instead of `g_slice_new0`, Camel would follow that chain into
/// freed memory the first time.
#[test]
fn a_fresh_folder_info_is_zeroed() {
    // SAFETY: no arguments. The test owns the returned struct until it frees it,
    // and `camel_folder_info_free` walks the (empty) chain and frees the names.
    unsafe {
        let info = camel_folder_info_new();
        assert!(!info.is_null(), "camel_folder_info_new returned NULL");

        assert!((*info).next.is_null(), "next is not NULL on a fresh info");
        assert!((*info).parent.is_null(), "parent is not NULL");
        assert!((*info).child.is_null(), "child is not NULL");
        assert!((*info).full_name.is_null(), "full_name is not NULL");
        assert!((*info).display_name.is_null(), "display_name is not NULL");
        assert_eq!((*info).flags, 0);
        assert_eq!((*info).total, 0);
        assert_eq!((*info).unread, 0);

        camel_folder_info_free(info);
    }
}

/// `camel_folder_info_free` frees the names in a `CamelFolderInfo` with `g_free`.
/// Thus they must come from `g_malloc`, not from a Rust allocator. The type system
/// does not show this, because both are `*mut gchar`. A leak checker examines the
/// round trip below. A `CString::into_raw` in its place causes heap corruption
/// that usually stays hidden until it corrupts other data.
#[test]
fn folder_info_names_survive_a_g_strdup_and_a_free() {
    // SAFETY: `g_strdup` allocates with g_malloc. `camel_folder_info_free`
    // releases the two name fields with the matching deallocator.
    unsafe {
        let info = camel_folder_info_new();
        (*info).full_name = glib_sys::g_strdup(c"Parent/Child".as_ptr());
        (*info).display_name = glib_sys::g_strdup(c"Child".as_ptr());

        assert_eq!(
            std::ffi::CStr::from_ptr((*info).full_name),
            c"Parent/Child",
            "the full_name field reads back from the wrong offset"
        );
        assert_eq!(std::ffi::CStr::from_ptr((*info).display_name), c"Child");

        camel_folder_info_free(info);
    }
}

/// The *type* of a folder is a small integer in the flags word, not one bit for
/// each type. `CAMEL_FOLDER_TYPE_MASK` isolates it, and the usual bit flags are
/// outside the mask. The role mapping sets a type with a bitwise OR. That is
/// correct only while these two facts are true, and both are header `#define`s
/// that the compiler does not check.
#[test]
fn the_folder_type_is_a_field_inside_the_flags_word() {
    assert_eq!(CAMEL_FOLDER_TYPE_MASK, 0x3F << CAMEL_FOLDER_TYPE_BIT);

    let types = [
        CAMEL_FOLDER_TYPE_NORMAL,
        CAMEL_FOLDER_TYPE_INBOX,
        CAMEL_FOLDER_TYPE_TRASH,
        CAMEL_FOLDER_TYPE_JUNK,
        CAMEL_FOLDER_TYPE_SENT,
        CAMEL_FOLDER_TYPE_ARCHIVE,
        CAMEL_FOLDER_TYPE_DRAFTS,
    ];
    for folder_type in types {
        assert_eq!(
            folder_type & CAMEL_FOLDER_TYPE_MASK as CamelFolderInfoFlags,
            folder_type,
            "a folder type has bits outside CAMEL_FOLDER_TYPE_MASK"
        );
    }

    // The flags that a JMAP folder also has are outside the type field, so a
    // bitwise OR with them cannot change the type.
    let flags = CAMEL_FOLDER_SUBSCRIBED
        | CAMEL_FOLDER_SYSTEM
        | CAMEL_FOLDER_CHILDREN
        | CAMEL_FOLDER_NOCHILDREN;
    assert_eq!(flags & CAMEL_FOLDER_TYPE_MASK as CamelFolderInfoFlags, 0);
}

/// None of the stock Camel settings classes implements `CamelNetworkSettings`.
/// Thus the provider declares its own settings type and does not use
/// `CamelOfflineSettings`, which `CamelOfflineStore` otherwise instantiates.
/// Host, port, user and security method are on that interface. Each provider in
/// the tree (IMAPx, POP, SMTP) implements it on its own settings subclass. A
/// store whose settings do not implement it has no server. The symptom is a NULL
/// host at connect time, not an error at build time.
#[test]
fn no_stock_camel_settings_class_carries_the_network_properties() {
    // SAFETY: plain type accessors, and `g_type_is_a` only reads the type
    // system.
    unsafe {
        let network = camel_network_settings_get_type();
        for settings in [
            camel_settings_get_type(),
            camel_store_settings_get_type(),
            camel_offline_settings_get_type(),
        ] {
            assert_eq!(
                g_type_is_a(settings, network),
                glib_sys::GFALSE,
                "{:?} implements CamelNetworkSettings after all; the provider's \
                 own settings type may be redundant",
                std::ffi::CStr::from_ptr(gobject_sys::g_type_name(settings))
            );
        }

        // A `CamelOfflineStore` subclass inherits the *offline* settings type. The
        // settings type of the provider replaces this default and keeps it as the
        // parent.
        assert_ne!(
            g_type_is_a(
                camel_offline_settings_get_type(),
                camel_store_settings_get_type()
            ),
            glib_sys::GFALSE
        );
    }
}

/// The three values of `security-method`, and the only one that means "no
/// encryption". `STARTTLS_ON_STANDARD_PORT` is the value that Camel recommends.
/// Its name refers to a protocol that JMAP does not have: JMAP uses HTTP. Thus the
/// two non-`NONE` values both mean TLS here. Only `NONE` sends credentials in the
/// clear.
///
/// The test pins `NONE` as *zero* because a settings object reads back zero when
/// no code has set the property. An implementer that claims `CamelNetworkSettings`
/// but does not override its properties never gets the default of the interface.
/// Thus it starts insecure without a warning. The default that applies with the
/// overrides is pinned with the overrides, in `tests/settings.rs` of `jmap-mail`.
#[test]
fn the_security_method_that_means_plaintext_is_the_zero_one() {
    assert_eq!(CAMEL_NETWORK_SECURITY_METHOD_NONE, 0);
    assert_ne!(CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT, 0);
    assert_ne!(CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT, 0);
}

/// The width of a stored message id, from the union that Camel declares it with.
/// `CamelSummaryMessageID` puts a `guint64` over eight bytes. The digest of a
/// provider is those eight bytes read as the integer, so both views must have the
/// same size, and that size must be eight. `message_id_digest` in `jmap-mail`
/// takes exactly that many bytes from the start of an MD5. If the union grew, the
/// digest would fill only half of the field, and all messages would thread on a
/// truncated id.
#[test]
fn a_stored_message_id_is_eight_bytes_of_a_digest() {
    let id = unsafe { std::mem::zeroed::<CamelSummaryMessageID>() };
    assert_eq!(size_of_val(&id), size_of::<u64>());
    // SAFETY: a read of either arm of a fully zeroed union is defined, and both
    // arms are plain data.
    unsafe {
        assert_eq!(size_of_val(id.id.hash.as_ref()), size_of::<u64>());
        assert_eq!(*id.id.id.as_ref(), 0);
    }
}

/// `CamelFolderChangeInfo` is a different plain struct that this provider gives
/// across the boundary. A folder uses it to tell Camel what changed. Like
/// `CamelProvider`, it is behind a boxed type, so `g_type_query` has no size and
/// `tests/layout.rs` has nothing to check. Unlike `CamelProvider`, it has public
/// fields *and* accessors for the same four arrays. A comparison of the two
/// replaces the GObject layout check. A field at the wrong offset reads an added
/// uid from the removed list.
///
/// The refresh vfunc uses the empty contract: a fresh change info reports no
/// changes. Thus "no different data came back" and "nothing to tell Camel" are
/// the same test. A folder that always emitted its `changed` signal would redraw
/// the message list of the user on each poll.
#[test]
fn a_change_info_is_empty_until_something_is_put_in_it() {
    // SAFETY: this block allocates, fills and frees the change info. The uid is a
    // NUL-terminated literal that the call copies.
    unsafe {
        let changes = camel_folder_change_info_new();
        assert!(!changes.is_null());
        assert_eq!(
            camel_folder_change_info_changed(changes),
            glib_sys::GFALSE,
            "a fresh change info claims to carry a change"
        );

        camel_folder_change_info_add_uid(changes, c"M1001".as_ptr());
        assert_ne!(camel_folder_change_info_changed(changes), glib_sys::GFALSE);

        let added = camel_folder_change_info_get_added_uids(changes);
        assert_eq!(
            added,
            (*changes).uid_added,
            "uid_added is not the added list"
        );
        assert_eq!((*added).len, 1);
        assert_eq!(
            std::ffi::CStr::from_ptr((*added).pdata.read().cast()),
            c"M1001"
        );
        assert_eq!((*(*changes).uid_removed).len, 0);
        assert_eq!((*(*changes).uid_changed).len, 0);
        assert_eq!((*(*changes).uid_recent).len, 0);

        camel_folder_change_info_free(changes);
    }
}

/// The store implements `CamelSubscribable` so that a user can subscribe and
/// unsubscribe folders of the account.
///
/// This test is not in `tests/layout.rs` for the same reason as `CamelProvider`:
/// `g_type_query` gives no data about an interface, so there is no size to
/// compare with `size_of`. The test checks the shape of the contract that the
/// provider signs. Each of the three checks below is a decision that the store
/// implementation uses.
#[test]
fn subscribing_is_an_interface_a_store_has_to_implement_itself() {
    // SAFETY: plain type-system reads. `g_type_interface_prerequisites` returns a
    // g_malloc'd array, and the test frees it.
    unsafe {
        let subscribable = camel_subscribable_get_type();

        // It is an interface, so `tests/layout.rs` cannot check it.
        assert_eq!(
            g_type_fundamental(subscribable),
            gobject_sys::G_TYPE_INTERFACE
        );
        let mut query = std::mem::zeroed::<GTypeQuery>();
        g_type_query(subscribable, &mut query);
        assert_eq!(query.type_, 0, "g_type_query knows an interface after all");

        // Only a store can implement it. Thus the `CamelJmapStore` of the provider
        // is the implementer, not an object next to it.
        let mut n = 0;
        let prerequisites = gobject_sys::g_type_interface_prerequisites(subscribable, &mut n);
        let required = std::slice::from_raw_parts(prerequisites, n as usize).to_vec();
        glib_sys::g_free(prerequisites.cast());
        assert!(
            required.contains(&camel_store_get_type()),
            "a CamelStore is not what CamelSubscribable requires"
        );

        // The offline store of Camel (the parent class of the provider) does not
        // implement it. Thus the provider must supply all that the interface
        // promises.
        assert_eq!(
            gobject_sys::g_type_is_a(camel_offline_store_get_type(), subscribable),
            glib_sys::GFALSE,
            "CamelOfflineStore implements CamelSubscribable already"
        );
    }
}

/// Camel has no default for the three methods. If an implementer leaves one NULL,
/// the store does not give a safe answer: the result is a call through a NULL
/// pointer from inside `camel_subscribable_folder_is_subscribed`. Thus, to
/// implement the interface, fill the vtable. A declaration alone is not
/// sufficient.
#[test]
fn the_interface_has_no_default_behind_any_of_its_three_methods() {
    // SAFETY: the test refs the default vtable for the reads and unrefs it after.
    unsafe {
        let vtable = gobject_sys::g_type_default_interface_ref(camel_subscribable_get_type())
            .cast::<CamelSubscribableInterface>();
        assert!(!vtable.is_null(), "the interface has no default vtable");

        assert!((*vtable).folder_is_subscribed.is_none());
        assert!((*vtable).subscribe_folder_sync.is_none());
        assert!((*vtable).unsubscribe_folder_sync.is_none());

        gobject_sys::g_type_default_interface_unref(vtable.cast());
    }
}

/// Tests whether `camel_provider_register` writes back into the struct it gets.
///
/// `provider::register` in `jmap-mail` gives the registered struct out as a
/// `&'static CamelProvider`. That is a *shared* Rust reference to memory that
/// Camel keeps a mutable pointer to for the life of the process. The Rust model
/// allows this only while the C side never writes there. Camel documents in-place
/// work on `extra_conf`, and a JMAP account leaves it NULL. Thus the bytes must
/// come back unchanged. If a Camel release starts to fill in `priv_` or to set a
/// default `url_hash`, this test shows it. Otherwise the symptom is a `&'static`
/// that changes for no clear reason.
#[test]
fn registering_a_provider_does_not_write_back_into_the_struct() {
    // SAFETY: as in the round-trip test: no arguments, and repeated calls are
    // harmless.
    let store_type = unsafe {
        camel_provider_init();
        camel_offline_store_get_type()
    };

    let protocol = c"jmap-eds-sys-immutable";
    let mut provider = CamelProvider {
        protocol: protocol.as_ptr(),
        name: c"JMAP immutability".as_ptr(),
        description: c"only ever registered by this test".as_ptr(),
        domain: c"mail".as_ptr(),
        flags: (CAMEL_PROVIDER_IS_REMOTE | CAMEL_PROVIDER_IS_SOURCE) as CamelProviderFlags,
        url_flags: CAMEL_URL_ALLOW_USER as CamelProviderURLFlags,
        extra_conf: ptr::null_mut(),
        port_entries: ptr::null_mut(),
        // Not in 3.58 or later, as above.
        #[cfg(camel_provider_auto_detect)]
        auto_detect: None,
        object_types: [store_type, G_TYPE_INVALID],
        authtypes: ptr::null_mut(),
        #[cfg(camel_provider_url_helpers)]
        url_hash: None,
        #[cfg(camel_provider_url_helpers)]
        url_equal: None,
        translation_domain: ptr::null(),
        priv_: ptr::null_mut(),
    };

    // SAFETY: this reads the bytes of the struct itself, which are initialised
    // above.
    let before = unsafe {
        std::slice::from_raw_parts(
            (&raw const provider).cast::<u8>(),
            std::mem::size_of::<CamelProvider>(),
        )
        .to_vec()
    };

    // SAFETY: `provider` is valid for this call. It is a local of this test, so
    // the pointer that Camel keeps is not valid after the test returns. No later
    // code asks Camel for it.
    unsafe { camel_provider_register(&mut provider) };

    // SAFETY: as `before`.
    let after = unsafe {
        std::slice::from_raw_parts(
            (&raw const provider).cast::<u8>(),
            std::mem::size_of::<CamelProvider>(),
        )
        .to_vec()
    };

    assert_eq!(
        before, after,
        "camel_provider_register mutated the provider struct; \
         handing it out as a shared &'static is no longer sound"
    );
}

/// Tests `CamelInternetAddress` creation, multiple recipient formatting, index inspection,
/// cloning, removal, and cleanup in EDS 3.52.
#[test]
fn camel_internet_address_formatting_and_lifecycle_in_eds() {
    unsafe {
        let addr = camel_internet_address_new();
        assert!(!addr.is_null());
        assert_eq!(camel_address_length(addr.cast()), 0);

        let formatted_empty = camel_address_format(addr.cast());
        if !formatted_empty.is_null() {
            glib_sys::g_free(formatted_empty.cast());
        }

        // Add 3 addresses. `camel_internet_address_add` returns the 0-based index.
        let added1 = camel_internet_address_add(
            addr,
            c"Alice Smith".as_ptr(),
            c"alice@example.com".as_ptr(),
        );
        assert_eq!(added1, 0);

        let added2 =
            camel_internet_address_add(addr, c"Doe, Bob".as_ptr(), c"bob@example.com".as_ptr());
        assert_eq!(added2, 1);

        let added3 =
            camel_internet_address_add(addr, std::ptr::null(), c"carol@example.org".as_ptr());
        assert_eq!(added3, 2);

        assert_eq!(camel_address_length(addr.cast()), 3);

        // Inspect index 0
        let mut name_ptr: *const gchar = std::ptr::null();
        let mut email_ptr: *const gchar = std::ptr::null();
        let res0 = camel_internet_address_get(addr, 0, &mut name_ptr, &mut email_ptr);
        assert_ne!(res0, glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(name_ptr).to_str().unwrap(),
            "Alice Smith"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(email_ptr).to_str().unwrap(),
            "alice@example.com"
        );

        // Inspect index 1
        let res1 = camel_internet_address_get(addr, 1, &mut name_ptr, &mut email_ptr);
        assert_ne!(res1, glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(name_ptr).to_str().unwrap(),
            "Doe, Bob"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(email_ptr).to_str().unwrap(),
            "bob@example.com"
        );

        // Inspect index 2 (NULL name)
        let res2 = camel_internet_address_get(addr, 2, &mut name_ptr, &mut email_ptr);
        assert_ne!(res2, glib_sys::GFALSE);
        assert!(
            name_ptr.is_null()
                || std::ffi::CStr::from_ptr(name_ptr)
                    .to_str()
                    .unwrap()
                    .is_empty()
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(email_ptr).to_str().unwrap(),
            "carol@example.org"
        );

        // Format into RFC 5322 text
        let formatted = camel_address_format(addr.cast());
        assert!(!formatted.is_null());
        let formatted_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(
            formatted_str.contains("Alice Smith") || formatted_str.contains("alice@example.com")
        );
        assert!(formatted_str.contains("bob@example.com"));
        assert!(formatted_str.contains("carol@example.org"));
        glib_sys::g_free(formatted.cast());

        // Clone
        let cloned = camel_address_new_clone(addr.cast());
        assert!(!cloned.is_null());
        assert_eq!(camel_address_length(cloned), 3);

        // Remove index 1 from original
        camel_address_remove(addr.cast(), 1);
        assert_eq!(camel_address_length(addr.cast()), 2);
        assert_eq!(camel_address_length(cloned), 3);

        gobject_sys::g_object_unref(cloned.cast());
        gobject_sys::g_object_unref(addr.cast());
    }
}

/// Tests `CamelMimeMessage` parsing, header inspection (Subject, From, Message-ID, Date),
/// medium content access, and in-place header mutation in EDS 3.52.
#[test]
fn camel_mime_message_construction_and_header_access_in_eds() {
    let msg_bytes = b"From: Alice Smith <alice@example.com>\r\n\
To: Bob Doe <bob@example.com>\r\n\
Subject: Project Update\r\n\
Message-ID: <update-2026@example.com>\r\n\
Date: Thu, 15 Jan 2026 12:00:00 +0000\r\n\
\r\n\
Hello Bob,\r\n\
Here is the project update.\r\n";

    unsafe {
        let msg = camel_mime_message_new();
        assert!(!msg.is_null());

        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        let parsed = camel_data_wrapper_construct_from_data_sync(
            msg.cast::<CamelDataWrapper>(),
            msg_bytes.as_ptr().cast(),
            msg_bytes.len() as glib_sys::gssize,
            std::ptr::null_mut(),
            &mut error,
        );
        assert_ne!(parsed, glib_sys::GFALSE);
        assert!(error.is_null());

        // Inspect Subject
        let subject = camel_mime_message_get_subject(msg);
        assert!(!subject.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(subject).to_str().unwrap(),
            "Project Update"
        );

        // Inspect Message-ID (Camel removes the angle brackets)
        let msg_id = camel_mime_message_get_message_id(msg);
        assert!(!msg_id.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(msg_id).to_str().unwrap(),
            "update-2026@example.com"
        );

        // Inspect From
        let from_addr = camel_mime_message_get_from(msg);
        assert!(!from_addr.is_null());
        assert_eq!(camel_address_length(from_addr.cast()), 1);

        let mut from_name: *const gchar = std::ptr::null();
        let mut from_email: *const gchar = std::ptr::null();
        let get_res = camel_internet_address_get(from_addr, 0, &mut from_name, &mut from_email);
        assert_ne!(get_res, glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(from_name).to_str().unwrap(),
            "Alice Smith"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(from_email).to_str().unwrap(),
            "alice@example.com"
        );

        // Date
        let mut offset: glib_sys::gint = 0;
        let date_ts = camel_mime_message_get_date(msg, &mut offset);
        assert!(date_ts > 0);

        // Content wrapper
        let content = camel_medium_get_content(msg.cast::<CamelMedium>());
        assert!(!content.is_null());

        // Change the subject in place
        camel_mime_message_set_subject(msg, c"Revised Project Update".as_ptr());
        let updated_subj = camel_mime_message_get_subject(msg);
        assert_eq!(
            std::ffi::CStr::from_ptr(updated_subj).to_str().unwrap(),
            "Revised Project Update"
        );

        gobject_sys::g_object_unref(msg.cast());
    }
}

/// Tests `CamelNamedFlags` user labels / keywords and summary `bdata` encoding in EDS 3.52.
#[test]
fn camel_named_flags_and_bdata_encoding_in_eds() {
    unsafe {
        // 1. CamelNamedFlags
        let flags = camel_named_flags_new();
        assert!(!flags.is_null());
        assert_eq!(camel_named_flags_get_length(flags), 0);

        camel_named_flags_insert(flags, c"\\Seen".as_ptr());
        camel_named_flags_insert(flags, c"\\Flagged".as_ptr());
        camel_named_flags_insert(flags, c"$label1".as_ptr());
        camel_named_flags_insert(flags, c"custom_tag".as_ptr());

        assert_eq!(camel_named_flags_get_length(flags), 4);
        assert_ne!(camel_named_flags_contains(flags, c"\\Seen".as_ptr()), 0);
        assert_ne!(camel_named_flags_contains(flags, c"$label1".as_ptr()), 0);
        assert_eq!(camel_named_flags_contains(flags, c"\\Draft".as_ptr()), 0);

        let copy = camel_named_flags_copy(flags);
        assert!(!copy.is_null());
        assert_ne!(camel_named_flags_equal(flags, copy), 0);

        camel_named_flags_remove(flags, c"$label1".as_ptr());
        assert_eq!(camel_named_flags_get_length(flags), 3);
        assert_eq!(camel_named_flags_contains(flags, c"$label1".as_ptr()), 0);
        assert_eq!(camel_named_flags_equal(flags, copy), 0);

        camel_named_flags_free(copy);
        camel_named_flags_free(flags);

        // 2. camel_util_bdata
        let gstr = glib_sys::g_string_new(c"".as_ptr());
        assert!(!gstr.is_null());

        camel_util_bdata_put_string(gstr, c"test_keyword".as_ptr());
        camel_util_bdata_put_number(gstr, 42);
        camel_util_bdata_put_string(gstr, c"state_v1".as_ptr());

        let mut read_ptr: *mut gchar = (*gstr).str;
        let s1 = camel_util_bdata_get_string(&mut read_ptr, std::ptr::null());
        assert!(!s1.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(s1).to_str().unwrap(),
            "test_keyword"
        );
        glib_sys::g_free(s1.cast());

        let num = camel_util_bdata_get_number(&mut read_ptr, 0);
        assert_eq!(num, 42);

        let s2 = camel_util_bdata_get_string(&mut read_ptr, std::ptr::null());
        assert!(!s2.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(s2).to_str().unwrap(), "state_v1");
        glib_sys::g_free(s2.cast());

        glib_sys::g_string_free(gstr, 1);
    }
}

/// Tests `CamelDataCache` disk cache directory path, file naming, and removal in EDS 3.52.
#[test]
fn camel_data_cache_operations_in_eds() {
    unsafe {
        let mut tmp_error: *mut glib_sys::GError = std::ptr::null_mut();
        let tmp_dir_raw =
            glib_sys::g_dir_make_tmp(c"eds-test-cache-XXXXXX".as_ptr(), &mut tmp_error);
        assert!(!tmp_dir_raw.is_null(), "g_dir_make_tmp failed");
        assert!(tmp_error.is_null());

        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        let cache = camel_data_cache_new(tmp_dir_raw, &mut error);
        assert!(!cache.is_null());
        assert!(error.is_null());

        let cached_path = camel_data_cache_get_path(cache);
        assert!(!cached_path.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(cached_path).to_str().unwrap(),
            std::ffi::CStr::from_ptr(tmp_dir_raw).to_str().unwrap()
        );

        let fn_ptr =
            camel_data_cache_get_filename(cache, c"cache_folder".as_ptr(), c"blob-100".as_ptr());
        assert!(!fn_ptr.is_null());
        let fn_str = std::ffi::CStr::from_ptr(fn_ptr).to_str().unwrap();
        assert!(fn_str.contains("cache_folder"));
        assert!(fn_str.contains("blob-100"));
        glib_sys::g_free(fn_ptr.cast());

        let _ = camel_data_cache_remove(
            cache,
            c"cache_folder".as_ptr(),
            c"blob-100".as_ptr(),
            &mut error,
        );
        if !error.is_null() {
            glib_sys::g_error_free(error);
        }

        gobject_sys::g_object_unref(cache.cast());
        glib_sys::g_free(tmp_dir_raw.cast());
    }
}

/// Tests `CamelNameValueArray` construction, header insertion, named lookup, copying,
/// equality, index removal, clearing, and cleanup in EDS 3.52.
#[test]
fn camel_name_value_array_lifecycle_and_operations_in_eds() {
    unsafe {
        let array = camel_name_value_array_new();
        assert!(!array.is_null());
        assert_eq!(camel_name_value_array_get_length(array), 0);

        camel_name_value_array_append(array, c"Subject".as_ptr(), c"Quarterly Report".as_ptr());
        camel_name_value_array_append(
            array,
            c"X-Custom-Header".as_ptr(),
            c"CustomValue42".as_ptr(),
        );
        camel_name_value_array_append(
            array,
            c"Received".as_ptr(),
            c"from mx1.example.com".as_ptr(),
        );
        camel_name_value_array_append(
            array,
            c"Received".as_ptr(),
            c"from mx2.example.com".as_ptr(),
        );

        assert_eq!(camel_name_value_array_get_length(array), 4);

        // Name and Value access by index
        let name0 = camel_name_value_array_get_name(array, 0);
        let val0 = camel_name_value_array_get_value(array, 0);
        assert!(!name0.is_null());
        assert!(!val0.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(name0).to_str().unwrap(), "Subject");
        assert_eq!(
            std::ffi::CStr::from_ptr(val0).to_str().unwrap(),
            "Quarterly Report"
        );

        // Named lookup
        let custom_val = camel_name_value_array_get_named(
            array,
            CAMEL_COMPARE_CASE_INSENSITIVE,
            c"x-custom-header".as_ptr(),
        );
        assert!(!custom_val.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(custom_val).to_str().unwrap(),
            "CustomValue42"
        );

        let missing_val = camel_name_value_array_get_named(
            array,
            CAMEL_COMPARE_CASE_SENSITIVE,
            c"Non-Existent".as_ptr(),
        );
        assert!(missing_val.is_null());

        // Get by out-pointers
        let mut out_name: *const gchar = std::ptr::null();
        let mut out_val: *const gchar = std::ptr::null();
        let get_res = camel_name_value_array_get(array, 1, &mut out_name, &mut out_val);
        assert_ne!(get_res, glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(out_name).to_str().unwrap(),
            "X-Custom-Header"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(out_val).to_str().unwrap(),
            "CustomValue42"
        );

        // Copy and equality
        let copy = camel_name_value_array_copy(array);
        assert!(!copy.is_null());
        assert_eq!(
            camel_name_value_array_get_length(copy),
            camel_name_value_array_get_length(array)
        );

        // Remove by index
        camel_name_value_array_remove(array, 1);
        assert_eq!(camel_name_value_array_get_length(array), 3);
        assert_eq!(camel_name_value_array_get_length(copy), 4);

        // Clear
        camel_name_value_array_clear(array);
        assert_eq!(camel_name_value_array_get_length(array), 0);

        camel_name_value_array_free(copy);
        camel_name_value_array_free(array);
    }
}

/// Tests `CamelMessageFlags` and `CamelFolderFlags` bitmask constants and bit independence
/// in EDS 3.52.
#[test]
fn camel_message_flags_and_folder_flags_in_eds() {
    // 1. CamelMessageFlags bitmask isolation
    assert_eq!(CAMEL_MESSAGE_ANSWERED, 1 << 0);
    assert_eq!(CAMEL_MESSAGE_DELETED, 1 << 1);
    assert_eq!(CAMEL_MESSAGE_DRAFT, 1 << 2);
    assert_eq!(CAMEL_MESSAGE_FLAGGED, 1 << 3);
    assert_eq!(CAMEL_MESSAGE_SEEN, 1 << 4);
    assert_eq!(CAMEL_MESSAGE_ATTACHMENTS, 1 << 5);
    assert_eq!(CAMEL_MESSAGE_ANSWERED_ALL, 1 << 6);
    assert_eq!(CAMEL_MESSAGE_JUNK, 1 << 7);
    assert_eq!(CAMEL_MESSAGE_SECURE, 1 << 8);
    assert_eq!(CAMEL_MESSAGE_NOTJUNK, 1 << 9);
    assert_eq!(CAMEL_MESSAGE_FORWARDED, 1 << 10);
    assert_eq!(CAMEL_MESSAGE_FOLDER_FLAGGED, 1 << 16);
    assert_eq!(CAMEL_MESSAGE_JUNK_LEARN, 1 << 30);
    assert_eq!(CAMEL_MESSAGE_USER, 1 << 31);

    let combined_msg_flags: CamelMessageFlags =
        CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED | CAMEL_MESSAGE_ATTACHMENTS;
    assert_ne!(combined_msg_flags & CAMEL_MESSAGE_SEEN, 0);
    assert_ne!(combined_msg_flags & CAMEL_MESSAGE_FLAGGED, 0);
    assert_ne!(combined_msg_flags & CAMEL_MESSAGE_ATTACHMENTS, 0);
    assert_eq!(combined_msg_flags & CAMEL_MESSAGE_ANSWERED, 0);
    assert_eq!(combined_msg_flags & CAMEL_MESSAGE_DELETED, 0);
    assert_eq!(combined_msg_flags & CAMEL_MESSAGE_DRAFT, 0);

    // 2. CamelFolderFlags (on folder objects) and CamelFolderInfoFlags (on folder info nodes)
    assert_eq!(CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY, 1 << 0);
    assert_eq!(CAMEL_FOLDER_FILTER_RECENT, 1 << 2);
    assert_eq!(CAMEL_FOLDER_HAS_BEEN_DELETED, 1 << 3);
    assert_eq!(CAMEL_FOLDER_IS_TRASH, 1 << 4);
    assert_eq!(CAMEL_FOLDER_IS_JUNK, 1 << 5);
    assert_eq!(CAMEL_FOLDER_FILTER_JUNK, 1 << 6);

    let combined_obj_flags: CamelFolderFlags =
        CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY | CAMEL_FOLDER_FILTER_RECENT | CAMEL_FOLDER_IS_TRASH;
    assert_ne!(combined_obj_flags & CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY, 0);
    assert_ne!(combined_obj_flags & CAMEL_FOLDER_FILTER_RECENT, 0);
    assert_ne!(combined_obj_flags & CAMEL_FOLDER_IS_TRASH, 0);
    assert_eq!(combined_obj_flags & CAMEL_FOLDER_IS_JUNK, 0);

    // CamelFolderInfoFlags
    assert_eq!(CAMEL_FOLDER_NOSELECT, 1 << 0);
    assert_eq!(CAMEL_FOLDER_NOINFERIORS, 1 << 1);
    assert_eq!(CAMEL_FOLDER_CHILDREN, 1 << 2);
    assert_eq!(CAMEL_FOLDER_NOCHILDREN, 1 << 3);
    assert_eq!(CAMEL_FOLDER_SUBSCRIBED, 1 << 4);
    assert_eq!(CAMEL_FOLDER_VIRTUAL, 1 << 5);
    assert_eq!(CAMEL_FOLDER_SYSTEM, 1 << 6);
    assert_eq!(CAMEL_FOLDER_SHARED_TO_ME, 1 << 8);
    assert_eq!(CAMEL_FOLDER_SHARED_BY_ME, 1 << 9);
    assert_eq!(CAMEL_FOLDER_READONLY, 1 << 16);
    assert_eq!(CAMEL_FOLDER_WRITEONLY, 1 << 17);

    let combined_info_flags: CamelFolderInfoFlags =
        CAMEL_FOLDER_SYSTEM | CAMEL_FOLDER_SUBSCRIBED | CAMEL_FOLDER_NOCHILDREN;
    assert_ne!(combined_info_flags & CAMEL_FOLDER_SYSTEM, 0);
    assert_ne!(combined_info_flags & CAMEL_FOLDER_SUBSCRIBED, 0);
    assert_ne!(combined_info_flags & CAMEL_FOLDER_NOCHILDREN, 0);
    assert_eq!(combined_info_flags & CAMEL_FOLDER_NOSELECT, 0);
    assert_eq!(combined_info_flags & CAMEL_FOLDER_READONLY, 0);
    assert_eq!(combined_info_flags & CAMEL_FOLDER_CHILDREN, 0);
}

/// Tests `CamelMIRecord` and `CamelFIRecord` summary database records in EDS 3.52.
///
/// EDS 3.60 made both structs private, so this test has nothing to examine there.
/// The summary and message-info classes of `jmap-mail` read and write them, so
/// this is a real port, not only a gap in the tests. `docs/eds-version-matrix.md`
/// records it.
#[cfg(camel_summary_records)]
#[test]
fn camel_summary_records_mirecord_and_firecord_in_eds() {
    unsafe {
        // CamelMIRecord (Message Info Record)
        let mut mi_rec: CamelMIRecord = std::mem::zeroed();
        let uid_ptr = glib_sys::g_strdup(c"msg-0042".as_ptr());
        let subj_ptr = glib_sys::g_strdup(c"Test Subject".as_ptr());
        let from_ptr = glib_sys::g_strdup(c"sender@example.com".as_ptr());
        let to_ptr = glib_sys::g_strdup(c"recipient@example.com".as_ptr());
        let bdata_ptr = glib_sys::g_strdup(c"keyword1 keyword2".as_ptr());

        mi_rec.uid = uid_ptr;
        mi_rec.subject = subj_ptr;
        mi_rec.from = from_ptr;
        mi_rec.to = to_ptr;
        mi_rec.flags = CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED;
        mi_rec.size = 2048;
        mi_rec.dsent = 1700000000;
        mi_rec.dreceived = 1700000100;
        mi_rec.bdata = bdata_ptr;

        assert_eq!(
            std::ffi::CStr::from_ptr(mi_rec.uid).to_str().unwrap(),
            "msg-0042"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(mi_rec.subject).to_str().unwrap(),
            "Test Subject"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(mi_rec.from).to_str().unwrap(),
            "sender@example.com"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(mi_rec.bdata).to_str().unwrap(),
            "keyword1 keyword2"
        );
        assert_eq!(mi_rec.size, 2048);
        assert_eq!(mi_rec.flags, CAMEL_MESSAGE_SEEN | CAMEL_MESSAGE_FLAGGED);

        glib_sys::g_free(uid_ptr.cast());
        glib_sys::g_free(subj_ptr.cast());
        glib_sys::g_free(from_ptr.cast());
        glib_sys::g_free(to_ptr.cast());
        glib_sys::g_free(bdata_ptr.cast());

        // CamelFIRecord (Folder Info Record)
        let mut fi_rec: CamelFIRecord = std::mem::zeroed();
        let fi_bdata = glib_sys::g_strdup(c"mailbox-state-token-12345".as_ptr());
        fi_rec.version = 1;
        fi_rec.flags = CAMEL_FOLDER_SYSTEM | CAMEL_FOLDER_SUBSCRIBED;
        fi_rec.nextuid = 100;
        fi_rec.timestamp = 1700000200;
        fi_rec.saved_count = 50;
        fi_rec.unread_count = 5;
        fi_rec.deleted_count = 2;
        fi_rec.junk_count = 0;
        fi_rec.bdata = fi_bdata;

        assert_eq!(fi_rec.version, 1);
        assert_eq!(fi_rec.flags, CAMEL_FOLDER_SYSTEM | CAMEL_FOLDER_SUBSCRIBED);
        assert_eq!(fi_rec.nextuid, 100);
        assert_eq!(fi_rec.saved_count, 50);
        assert_eq!(fi_rec.unread_count, 5);
        assert_eq!(
            std::ffi::CStr::from_ptr(fi_rec.bdata).to_str().unwrap(),
            "mailbox-state-token-12345"
        );

        glib_sys::g_free(fi_bdata.cast());
    }
}

/// Tests `CamelURL` construction, parameter setting/getting, cloning, equality, hash,
/// string formatting, and cleanup in EDS 3.52.
#[test]
fn camel_url_lifecycle_and_parameter_operations_in_eds() {
    unsafe {
        let raw_url_str = c"jmap://alice@mail.example.com:8443/inbox;param1=val1#anchor";
        let url = camel_url_new(raw_url_str.as_ptr(), std::ptr::null_mut());
        assert!(!url.is_null());

        assert_eq!(
            std::ffi::CStr::from_ptr((*url).protocol).to_str().unwrap(),
            "jmap"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr((*url).user).to_str().unwrap(),
            "alice"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr((*url).host).to_str().unwrap(),
            "mail.example.com"
        );
        assert_eq!((*url).port, 8443);
        assert_eq!(
            std::ffi::CStr::from_ptr((*url).path).to_str().unwrap(),
            "/inbox"
        );

        // Parameter get
        let p1 = camel_url_get_param(url, c"param1".as_ptr());
        assert!(!p1.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(p1).to_str().unwrap(), "val1");

        // Parameter set
        camel_url_set_param(url, c"param2".as_ptr(), c"val2".as_ptr());
        let p2 = camel_url_get_param(url, c"param2".as_ptr());
        assert!(!p2.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(p2).to_str().unwrap(), "val2");

        // Copy, equality, and hash
        let copy = camel_url_copy(url);
        assert!(!copy.is_null());
        assert_ne!(camel_url_equal(url, copy), glib_sys::GFALSE);
        assert_eq!(camel_url_hash(url), camel_url_hash(copy));

        // Format to string
        let formatted = camel_url_to_string(url, 0);
        assert!(!formatted.is_null());
        let formatted_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(formatted_str.starts_with("jmap://"));
        assert!(formatted_str.contains("mail.example.com"));
        assert!(formatted_str.contains("param1=val1"));
        assert!(formatted_str.contains("param2=val2"));
        glib_sys::g_free(formatted.cast());

        camel_url_free(copy);
        camel_url_free(url);
    }
}

/// Tests `CamelFolderChangeInfo` multi-event batching (add, remove, change, recent),
/// concatenation (`camel_folder_change_info_cat`), clearing, and array access in EDS 3.52.
#[test]
fn camel_folder_change_info_batching_and_concatenation_in_eds() {
    unsafe {
        let info1 = camel_folder_change_info_new();
        assert!(!info1.is_null());

        camel_folder_change_info_add_uid(info1, c"msg-101".as_ptr());
        camel_folder_change_info_remove_uid(info1, c"msg-102".as_ptr());
        camel_folder_change_info_change_uid(info1, c"msg-103".as_ptr());
        camel_folder_change_info_recent_uid(info1, c"msg-104".as_ptr());

        assert_ne!(camel_folder_change_info_changed(info1), glib_sys::GFALSE);
        assert_eq!((*camel_folder_change_info_get_added_uids(info1)).len, 1);
        assert_eq!((*camel_folder_change_info_get_removed_uids(info1)).len, 1);
        assert_eq!((*camel_folder_change_info_get_changed_uids(info1)).len, 1);
        assert_eq!((*camel_folder_change_info_get_recent_uids(info1)).len, 1);

        let info2 = camel_folder_change_info_new();
        assert!(!info2.is_null());
        camel_folder_change_info_add_uid(info2, c"msg-201".as_ptr());
        camel_folder_change_info_change_uid(info2, c"msg-202".as_ptr());

        // Concatenate info2 into info1
        camel_folder_change_info_cat(info1, info2);

        let added = camel_folder_change_info_get_added_uids(info1);
        let removed = camel_folder_change_info_get_removed_uids(info1);
        let changed = camel_folder_change_info_get_changed_uids(info1);
        let recent = camel_folder_change_info_get_recent_uids(info1);

        assert_eq!((*added).len, 2);
        assert_eq!((*removed).len, 1);
        assert_eq!((*changed).len, 2);
        assert_eq!((*recent).len, 1);

        // Clear info1
        camel_folder_change_info_clear(info1);
        assert_eq!(camel_folder_change_info_changed(info1), glib_sys::GFALSE);
        assert_eq!((*camel_folder_change_info_get_added_uids(info1)).len, 0);

        camel_folder_change_info_free(info2);
        camel_folder_change_info_free(info1);
    }
}

/// Tests Camel error domain quarks (`camel_folder_error_quark`, `camel_service_error_quark`,
/// `camel_store_error_quark`) and error code constants in EDS 3.52.
#[test]
fn camel_error_quarks_and_distinct_domains_in_eds() {
    unsafe {
        let folder_q = camel_folder_error_quark();
        let service_q = camel_service_error_quark();
        let store_q = camel_store_error_quark();
        let client_q = e_client_error_quark();
        let book_q = e_book_client_error_quark();
        let cal_q = e_cal_client_error_quark();

        assert_ne!(folder_q, 0);
        assert_ne!(service_q, 0);
        assert_ne!(store_q, 0);

        // All 6 quarks are pairwise distinct
        assert_ne!(folder_q, service_q);
        assert_ne!(folder_q, store_q);
        assert_ne!(service_q, store_q);
        assert_ne!(folder_q, client_q);
        assert_ne!(folder_q, book_q);
        assert_ne!(folder_q, cal_q);
        assert_ne!(service_q, client_q);
        assert_ne!(store_q, client_q);

        // Distinct error codes
        assert_eq!(CAMEL_FOLDER_ERROR_INVALID, 0);
        assert_eq!(CAMEL_FOLDER_ERROR_INVALID_STATE, 1);
        assert_eq!(CAMEL_FOLDER_ERROR_INVALID_UID, 6);
        assert_eq!(CAMEL_SERVICE_ERROR_INVALID, 0);
        assert_eq!(CAMEL_SERVICE_ERROR_URL_INVALID, 1);
        assert_eq!(CAMEL_SERVICE_ERROR_CANT_AUTHENTICATE, 3);
        assert_eq!(CAMEL_STORE_ERROR_INVALID, 0);
        assert_eq!(CAMEL_STORE_ERROR_NO_FOLDER, 1);
    }
}

/// Tests `CamelMimePart` description, disposition, filename, content-ID, content-location,
/// transfer encoding, and content payload setting in EDS 3.52.
#[test]
fn camel_mime_part_headers_disposition_and_encoding_in_eds() {
    unsafe {
        let part = camel_mime_part_new();
        assert!(!part.is_null());

        // Description
        camel_mime_part_set_description(part, c"Monthly Financial Report".as_ptr());
        let desc = camel_mime_part_get_description(part);
        assert!(!desc.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(desc).to_str().unwrap(),
            "Monthly Financial Report"
        );

        // Disposition & Filename
        camel_mime_part_set_disposition(part, c"attachment".as_ptr());
        let disp = camel_mime_part_get_disposition(part);
        assert!(!disp.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(disp).to_str().unwrap(),
            "attachment"
        );

        camel_mime_part_set_filename(part, c"report-jan2026.pdf".as_ptr());
        let fname = camel_mime_part_get_filename(part);
        assert!(!fname.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(fname).to_str().unwrap(),
            "report-jan2026.pdf"
        );

        // Content-ID and Content-Location. Camel expects an ID without brackets and
        // writes it as `Content-ID: <...>`.
        camel_mime_part_set_content_id(part, c"pdf-part-001@example.com".as_ptr());
        let cid = camel_mime_part_get_content_id(part);
        assert!(!cid.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(cid).to_str().unwrap(),
            "pdf-part-001@example.com"
        );

        camel_mime_part_set_content_location(part, c"https://example.com/reports/jan.pdf".as_ptr());
        let cloc = camel_mime_part_get_content_location(part);
        assert!(!cloc.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(cloc).to_str().unwrap(),
            "https://example.com/reports/jan.pdf"
        );

        // Transfer Encoding
        camel_mime_part_set_encoding(part, CAMEL_TRANSFER_ENCODING_BASE64);
        assert_eq!(
            camel_mime_part_get_encoding(part),
            CAMEL_TRANSFER_ENCODING_BASE64
        );

        camel_mime_part_set_encoding(part, CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE);
        assert_eq!(
            camel_mime_part_get_encoding(part),
            CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE
        );

        // Content Setting
        let dummy_pdf_data = b"%PDF-1.5 test document stream payload";
        camel_mime_part_set_content(
            part,
            dummy_pdf_data.as_ptr().cast(),
            dummy_pdf_data.len() as glib_sys::gint,
            c"application/pdf".as_ptr(),
        );

        let content = camel_medium_get_content(part.cast());
        assert!(!content.is_null());
        let mime_type = camel_data_wrapper_get_mime_type(content);
        assert!(!mime_type.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(mime_type).to_str().unwrap(),
            "application/pdf"
        );

        gobject_sys::g_object_unref(part.cast());
    }
}

/// Tests `CamelMultipart` container creation, boundary management, part addition,
/// indexed retrieval, part counting, and preface/postface text in EDS 3.52.
#[test]
fn camel_multipart_container_and_part_management_in_eds() {
    unsafe {
        let mp = camel_multipart_new();
        assert!(!mp.is_null());
        assert_eq!(camel_multipart_get_number(mp), 0);

        // Boundary
        camel_multipart_set_boundary(mp, c"==_boundary_section_42_==".as_ptr());
        let boundary = camel_multipart_get_boundary(mp);
        assert!(!boundary.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(boundary).to_str().unwrap(),
            "==_boundary_section_42_=="
        );

        // Preface & Postface
        camel_multipart_set_preface(
            mp,
            c"This is a multi-part message in MIME format.\n".as_ptr(),
        );
        let preface = camel_multipart_get_preface(mp);
        assert!(!preface.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(preface).to_str().unwrap(),
            "This is a multi-part message in MIME format.\n"
        );

        camel_multipart_set_postface(mp, c"-- End of multi-part message --\n".as_ptr());
        let postface = camel_multipart_get_postface(mp);
        assert!(!postface.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(postface).to_str().unwrap(),
            "-- End of multi-part message --\n"
        );

        // Add 2 parts
        let part1 = camel_mime_part_new();
        camel_mime_part_set_content(
            part1,
            c"Plain text body content".as_ptr(),
            23,
            c"text/plain".as_ptr(),
        );
        camel_multipart_add_part(mp, part1);

        let part2 = camel_mime_part_new();
        camel_mime_part_set_content(
            part2,
            c"<html><body><p>HTML formatted body</p></body></html>".as_ptr(),
            52,
            c"text/html".as_ptr(),
        );
        camel_multipart_add_part(mp, part2);

        assert_eq!(camel_multipart_get_number(mp), 2);

        // Inspect parts by index
        let got_part1 = camel_multipart_get_part(mp, 0);
        assert_eq!(got_part1, part1);
        let got_part2 = camel_multipart_get_part(mp, 1);
        assert_eq!(got_part2, part2);

        // Attach multipart to a CamelMimeMessage
        let msg = camel_mime_message_new();
        camel_medium_set_content(msg.cast(), mp.cast());
        let root_content = camel_medium_get_content(msg.cast());
        assert_eq!(root_content, mp.cast());

        gobject_sys::g_object_unref(part2.cast());
        gobject_sys::g_object_unref(part1.cast());
        gobject_sys::g_object_unref(msg.cast());
    }
}

/// Tests `CamelTransferEncoding` constants and `CAMEL_MAX_PREVIEW_LENGTH` in EDS 3.52.
#[test]
fn camel_transfer_encoding_constants_in_eds() {
    assert_eq!(CAMEL_TRANSFER_ENCODING_DEFAULT, 0);
    assert_eq!(CAMEL_TRANSFER_ENCODING_7BIT, 1);
    assert_eq!(CAMEL_TRANSFER_ENCODING_8BIT, 2);
    assert_eq!(CAMEL_TRANSFER_ENCODING_BASE64, 3);
    assert_eq!(CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE, 4);
    assert_eq!(CAMEL_TRANSFER_ENCODING_BINARY, 5);
    assert_eq!(CAMEL_TRANSFER_ENCODING_UUENCODE, 6);
    assert_eq!(CAMEL_TRANSFER_NUM_ENCODINGS, 7);

    assert_eq!(CAMEL_MAX_PREVIEW_LENGTH, 256);
}

/// Tests `CamelContentType` creation, parameter setting, decoding, matching, and formatting
/// in EDS 3.52.
#[test]
fn camel_content_type_creation_parameters_and_matching_in_eds() {
    unsafe {
        // Construct structured content type
        let ct = camel_content_type_new(c"text".as_ptr(), c"plain".as_ptr());
        assert!(!ct.is_null());
        assert_eq!(
            camel_content_type_is(ct, c"text".as_ptr(), c"plain".as_ptr()),
            glib_sys::GTRUE
        );
        assert_eq!(
            camel_content_type_is(ct, c"text".as_ptr(), c"*".as_ptr()),
            glib_sys::GTRUE
        );
        assert_eq!(
            camel_content_type_is(ct, c"image".as_ptr(), c"*".as_ptr()),
            glib_sys::GFALSE
        );

        // Parameters
        camel_content_type_set_param(ct, c"charset".as_ptr(), c"utf-8".as_ptr());
        camel_content_type_set_param(ct, c"format".as_ptr(), c"flowed".as_ptr());
        let charset = camel_content_type_param(ct, c"charset".as_ptr());
        assert!(!charset.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(charset).to_str().unwrap(), "utf-8");

        // Format
        let formatted = camel_content_type_format(ct);
        assert!(!formatted.is_null());
        let formatted_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(formatted_str.starts_with("text/plain"));
        assert!(formatted_str.contains("charset=\"utf-8\""));
        assert!(formatted_str.contains("format=\"flowed\""));
        glib_sys::g_free(formatted.cast());

        let simple = camel_content_type_simple(ct);
        assert!(!simple.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(simple).to_str().unwrap(),
            "text/plain"
        );
        glib_sys::g_free(simple.cast());

        camel_content_type_unref(ct);

        // Decode from header string
        let decoded = camel_content_type_decode(
            c"text/html; charset=\"ISO-8859-1\"; name=\"document.html\"".as_ptr(),
        );
        assert!(!decoded.is_null());
        assert_eq!(
            camel_content_type_is(decoded, c"text".as_ptr(), c"html".as_ptr()),
            glib_sys::GTRUE
        );
        let name_param = camel_content_type_param(decoded, c"name".as_ptr());
        assert!(!name_param.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(name_param).to_str().unwrap(),
            "document.html"
        );

        camel_content_type_unref(decoded);
    }
}

/// Tests `CamelContentDisposition` decoding, formatting, and attachment checking in EDS 3.52.
#[test]
fn camel_content_disposition_attachment_parsing_in_eds() {
    unsafe {
        let disp = camel_content_disposition_decode(
            c"attachment; filename=\"statement-2026.pdf\"".as_ptr(),
        );
        assert!(!disp.is_null());

        let ct = camel_content_type_new(c"application".as_ptr(), c"pdf".as_ptr());
        let is_att = camel_content_disposition_is_attachment(disp, ct);
        assert_eq!(is_att, glib_sys::GTRUE);

        let formatted = camel_content_disposition_format(disp);
        assert!(!formatted.is_null());
        let form_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(form_str.contains("attachment"));
        assert!(form_str.contains("statement-2026.pdf"));
        glib_sys::g_free(formatted.cast());

        camel_content_type_unref(ct);
        camel_content_disposition_unref(disp);
    }
}

/// Tests `CamelHeaderAddress` address list decoding, formatting, and encoding in EDS 3.52.
#[test]
fn camel_header_address_mailbox_decoding_and_formatting_in_eds() {
    unsafe {
        let raw = c"Alice Smith <alice@example.com>, Bob Jones <bob@example.com>";
        let mut addrs: *mut CamelHeaderAddress =
            camel_header_address_decode(raw.as_ptr(), std::ptr::null());
        assert!(!addrs.is_null());

        // Check first address node
        assert_eq!((*addrs).type_, CAMEL_HEADER_ADDRESS_NAME);
        let name1 = (*addrs).name;
        let addr1 = (*addrs).v.addr;
        assert!(!name1.is_null());
        assert!(!addr1.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(name1).to_str().unwrap(),
            "Alice Smith"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(addr1).to_str().unwrap(),
            "alice@example.com"
        );

        // Check second address node
        let next_node = (*addrs).next;
        assert!(!next_node.is_null());
        assert_eq!((*next_node).type_, CAMEL_HEADER_ADDRESS_NAME);
        let name2 = (*next_node).name;
        let addr2 = (*next_node).v.addr;
        assert_eq!(
            std::ffi::CStr::from_ptr(name2).to_str().unwrap(),
            "Bob Jones"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(addr2).to_str().unwrap(),
            "bob@example.com"
        );

        // Format and Encode
        let formatted = camel_header_address_list_format(addrs);
        assert!(!formatted.is_null());
        let f_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(f_str.contains("Alice Smith <alice@example.com>"));
        assert!(f_str.contains("Bob Jones <bob@example.com>"));
        glib_sys::g_free(formatted.cast());

        let encoded = camel_header_address_list_encode(addrs);
        assert!(!encoded.is_null());
        let enc_str = std::ffi::CStr::from_ptr(encoded).to_str().unwrap();
        assert!(enc_str.contains("alice@example.com"));
        glib_sys::g_free(encoded.cast());

        camel_header_address_list_clear(&mut addrs);
        assert!(addrs.is_null());
    }
}

/// Tests Camel header date decoding, date formatting, and message ID utilities in EDS 3.52.
#[test]
fn camel_header_date_and_msgid_utilities_in_eds() {
    unsafe {
        // Date parse
        let mut tz_offset: glib_sys::gint = 0;
        let date_sec =
            camel_header_decode_date(c"Thu, 15 Jan 2026 09:30:00 +0000".as_ptr(), &mut tz_offset);
        assert_eq!(date_sec, 1_768_469_400);
        assert_eq!(tz_offset, 0);

        // Date format
        let formatted_date = camel_header_format_date(date_sec, 0);
        assert!(!formatted_date.is_null());
        let date_str = std::ffi::CStr::from_ptr(formatted_date).to_str().unwrap();
        assert!(date_str.contains("Jan 2026 09:30:00"));
        assert!(date_str.contains("+0000"));
        glib_sys::g_free(formatted_date.cast());

        // Message-ID decode (unwraps angle brackets)
        let mid = camel_header_msgid_decode(c"<msg-alpha-99@example.com>".as_ptr());
        assert!(!mid.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(mid).to_str().unwrap(),
            "msg-alpha-99@example.com"
        );
        glib_sys::g_free(mid.cast());

        // Transfer encoding from string
        assert_eq!(
            camel_transfer_encoding_from_string(c"base64".as_ptr()),
            CAMEL_TRANSFER_ENCODING_BASE64
        );
        assert_eq!(
            camel_transfer_encoding_from_string(c"quoted-printable".as_ptr()),
            CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE
        );
        assert_eq!(
            camel_transfer_encoding_from_string(c"7bit".as_ptr()),
            CAMEL_TRANSFER_ENCODING_7BIT
        );
    }
}

/// Tests `CamelMimeMessage` Message-ID, Reply-To, attachment detection, and part lookup by
/// Content-ID in EDS 3.52.
#[test]
fn camel_mime_message_attachment_and_reply_to_in_eds() {
    unsafe {
        let msg = camel_mime_message_new();
        assert!(!msg.is_null());

        // Message-ID
        camel_mime_message_set_message_id(msg, c"<auto-reply-777@example.com>".as_ptr());
        let mid = camel_mime_message_get_message_id(msg);
        assert!(!mid.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(mid).to_str().unwrap(),
            "<auto-reply-777@example.com>"
        );

        // Reply-To
        let reply_to = camel_internet_address_new();
        camel_internet_address_add(
            reply_to,
            c"Support Team".as_ptr(),
            c"support@example.com".as_ptr(),
        );
        camel_mime_message_set_reply_to(msg, reply_to);

        let got_reply_to = camel_mime_message_get_reply_to(msg);
        assert!(!got_reply_to.is_null());
        assert_eq!(camel_address_length(got_reply_to.cast()), 1);

        // Attachment and Content-ID lookup
        assert_eq!(camel_mime_message_has_attachment(msg), glib_sys::GFALSE);

        let mp = camel_multipart_new();
        camel_multipart_set_boundary(mp, c"==_att_bound_==".as_ptr());

        let text_part = camel_mime_part_new();
        camel_mime_part_set_content(
            text_part,
            c"Please see attached invoice.".as_ptr(),
            28,
            c"text/plain".as_ptr(),
        );
        camel_multipart_add_part(mp, text_part);

        let att_part = camel_mime_part_new();
        camel_mime_part_set_disposition(att_part, c"attachment".as_ptr());
        camel_mime_part_set_filename(att_part, c"invoice.pdf".as_ptr());
        camel_mime_part_set_content_id(att_part, c"invoice-100@example.com".as_ptr());
        camel_mime_part_set_content(
            att_part,
            c"%PDF-invoice-content".as_ptr(),
            20,
            c"application/pdf".as_ptr(),
        );
        camel_multipart_add_part(mp, att_part);

        camel_medium_set_content(msg.cast(), mp.cast());

        assert_eq!(camel_mime_message_has_attachment(msg), glib_sys::GTRUE);

        let found_part =
            camel_mime_message_get_part_by_content_id(msg, c"invoice-100@example.com".as_ptr());
        assert_eq!(found_part, att_part);

        let not_found =
            camel_mime_message_get_part_by_content_id(msg, c"non-existent-id@example.com".as_ptr());
        assert!(not_found.is_null());

        gobject_sys::g_object_unref(att_part.cast());
        gobject_sys::g_object_unref(text_part.cast());
        gobject_sys::g_object_unref(reply_to.cast());
        gobject_sys::g_object_unref(msg.cast());
    }
}

/// Tests `CamelStreamMem` memory stream and `CamelStreamNull` null sink in EDS 3.52.
#[test]
fn camel_stream_mem_and_null_stream_operations_in_eds() {
    let test_data = b"Stream payload data for JMAP unit tests\r\n";

    unsafe {
        // 1. CamelStreamMem
        let mem_stream = camel_stream_mem_new();
        assert!(!mem_stream.is_null());
        // A fresh memory stream has 0 bytes, so EOS is TRUE
        assert_eq!(camel_stream_eos(mem_stream), glib_sys::GTRUE);

        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        let written = camel_stream_write(
            mem_stream,
            test_data.as_ptr().cast(),
            test_data.len(),
            std::ptr::null_mut(),
            &mut error,
        );
        assert_eq!(written as usize, test_data.len());
        assert!(error.is_null());

        let flush_res = camel_stream_flush(mem_stream, std::ptr::null_mut(), &mut error);
        assert_eq!(flush_res, 0);
        assert!(error.is_null());

        let byte_array = camel_stream_mem_get_byte_array(mem_stream.cast());
        assert!(!byte_array.is_null());
        assert_eq!((*byte_array).len as usize, test_data.len());

        // 2. CamelStreamNull
        let null_stream = camel_stream_null_new();
        assert!(!null_stream.is_null());
        assert_eq!(camel_stream_null_get_bytes_written(null_stream.cast()), 0);

        let null_written = camel_stream_write(
            null_stream,
            test_data.as_ptr().cast(),
            test_data.len(),
            std::ptr::null_mut(),
            &mut error,
        );
        assert_eq!(null_written as usize, test_data.len());
        assert_eq!(
            camel_stream_null_get_bytes_written(null_stream.cast()),
            test_data.len()
        );
        assert_eq!(
            camel_stream_null_get_ends_with_crlf(null_stream.cast()),
            glib_sys::GTRUE
        );

        gobject_sys::g_object_unref(null_stream.cast());
        gobject_sys::g_object_unref(mem_stream.cast());
    }
}

/// Tests `CamelStreamFs` filesystem stream operations in EDS 3.52.
#[test]
fn camel_stream_fs_file_operations_in_eds() {
    unsafe {
        let mut tmp_error: *mut glib_sys::GError = std::ptr::null_mut();
        let tmp_dir_raw =
            glib_sys::g_dir_make_tmp(c"eds-fs-stream-XXXXXX".as_ptr(), &mut tmp_error);
        assert!(!tmp_dir_raw.is_null());
        assert!(tmp_error.is_null());

        let file_path = format!(
            "{}/test_file.bin\0",
            std::ffi::CStr::from_ptr(tmp_dir_raw).to_str().unwrap()
        );

        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        // O_RDWR (2) | O_CREAT (64) | O_TRUNC (512) = 578
        let fs_stream =
            camel_stream_fs_new_with_name(file_path.as_ptr().cast(), 578, 0o600, &mut error);
        assert!(!fs_stream.is_null(), "camel_stream_fs_new_with_name failed");
        assert!(error.is_null());

        let fd = camel_stream_fs_get_fd(fs_stream.cast());
        assert!(fd >= 0);

        let test_str = c"Filesystem stream write test content\r\n";
        let written = camel_stream_write_string(
            fs_stream,
            test_str.as_ptr(),
            std::ptr::null_mut(),
            &mut error,
        );
        assert!(written > 0);
        assert!(error.is_null());

        camel_stream_close(fs_stream, std::ptr::null_mut(), &mut error);
        assert!(error.is_null());
        gobject_sys::g_object_unref(fs_stream.cast());

        // Re-open for read (O_RDONLY = 0)
        let read_stream =
            camel_stream_fs_new_with_name(file_path.as_ptr().cast(), 0, 0, &mut error);
        assert!(!read_stream.is_null());
        assert!(error.is_null());

        let mut buf = [0u8; 64];
        let n_read = camel_stream_read(
            read_stream,
            buf.as_mut_ptr().cast(),
            buf.len(),
            std::ptr::null_mut(),
            &mut error,
        );
        assert_eq!(n_read, written);
        assert_eq!(&buf[..n_read as usize], test_str.to_bytes());

        camel_stream_close(read_stream, std::ptr::null_mut(), &mut error);
        gobject_sys::g_object_unref(read_stream.cast());
        glib_sys::g_free(tmp_dir_raw.cast());
    }
}

/// Tests `CamelMimeFilterBasic` Base64 and Quoted-Printable filtering in EDS 3.52.
#[test]
fn camel_mime_filter_basic_base64_and_qp_in_eds() {
    let plain = b"Hello, JMAP MIME stream filtering!";

    unsafe {
        // Base64 Encode
        let enc_filter = camel_mime_filter_basic_new(CAMEL_MIME_FILTER_BASIC_BASE64_ENC);
        assert!(!enc_filter.is_null());

        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            enc_filter,
            plain.as_ptr().cast(),
            plain.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut encoded = Vec::new();
        if out_len > 0 && !out_ptr.is_null() {
            encoded.extend_from_slice(std::slice::from_raw_parts(out_ptr as *const u8, out_len));
        }

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;

        let empty = c"";
        camel_mime_filter_complete(
            enc_filter,
            empty.as_ptr(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );

        if comp_len > 0 && !comp_ptr.is_null() {
            encoded.extend_from_slice(std::slice::from_raw_parts(comp_ptr as *const u8, comp_len));
        }

        assert!(!encoded.is_empty());
        let enc_str = String::from_utf8_lossy(&encoded);
        assert!(enc_str.contains("SGVsbG8sIEpNQVAgTUlNR"));

        // Base64 Decode
        let dec_filter = camel_mime_filter_basic_new(CAMEL_MIME_FILTER_BASIC_BASE64_DEC);
        assert!(!dec_filter.is_null());

        let mut dec_out_ptr: *mut gchar = std::ptr::null_mut();
        let mut dec_out_len: gsize = 0;
        let mut dec_out_pre: gsize = 0;

        camel_mime_filter_filter(
            dec_filter,
            encoded.as_ptr().cast(),
            encoded.len(),
            0,
            &mut dec_out_ptr,
            &mut dec_out_len,
            &mut dec_out_pre,
        );

        let mut decoded = Vec::new();
        if dec_out_len > 0 && !dec_out_ptr.is_null() {
            decoded.extend_from_slice(std::slice::from_raw_parts(
                dec_out_ptr as *const u8,
                dec_out_len,
            ));
        }

        let mut dec_comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut dec_comp_len: gsize = 0;
        let mut dec_comp_pre: gsize = 0;

        camel_mime_filter_complete(
            dec_filter,
            empty.as_ptr(),
            0,
            0,
            &mut dec_comp_ptr,
            &mut dec_comp_len,
            &mut dec_comp_pre,
        );

        if dec_comp_len > 0 && !dec_comp_ptr.is_null() {
            decoded.extend_from_slice(std::slice::from_raw_parts(
                dec_comp_ptr as *const u8,
                dec_comp_len,
            ));
        }

        assert_eq!(decoded.as_slice(), plain);

        // QP Filter creation
        let qp_enc = camel_mime_filter_basic_new(CAMEL_MIME_FILTER_BASIC_QP_ENC);
        assert!(!qp_enc.is_null());
        let qp_dec = camel_mime_filter_basic_new(CAMEL_MIME_FILTER_BASIC_QP_DEC);
        assert!(!qp_dec.is_null());

        gobject_sys::g_object_unref(qp_dec.cast());
        gobject_sys::g_object_unref(qp_enc.cast());
        gobject_sys::g_object_unref(dec_filter.cast());
        gobject_sys::g_object_unref(enc_filter.cast());
    }
}

/// Tests `CamelMimeFilterCRLF` and `CamelMimeFilterLinewrap` in EDS 3.52.
#[test]
fn camel_mime_filter_crlf_and_linewrap_in_eds() {
    unsafe {
        let crlf_filter = camel_mime_filter_crlf_new(
            CAMEL_MIME_FILTER_CRLF_ENCODE,
            CAMEL_MIME_FILTER_CRLF_MODE_CRLF_ONLY,
        );
        assert!(!crlf_filter.is_null());

        camel_mime_filter_crlf_set_ensure_crlf_end(crlf_filter.cast(), glib_sys::GTRUE);
        assert_eq!(
            camel_mime_filter_crlf_get_ensure_crlf_end(crlf_filter.cast()),
            glib_sys::GTRUE
        );

        let linewrap_filter =
            camel_mime_filter_linewrap_new(72, 80, b' ' as gchar, CAMEL_MIME_FILTER_LINEWRAP_WORD);
        assert!(!linewrap_filter.is_null());

        gobject_sys::g_object_unref(linewrap_filter.cast());
        gobject_sys::g_object_unref(crlf_filter.cast());
    }
}

/// Tests `CamelStreamFilter` stream pipeline with `CamelMimeFilterBasic` in EDS 3.52.
#[test]
fn camel_stream_filter_pipeline_in_eds() {
    let base64_input = b"SGVsbG8sIEpNQVAgc3RyZWFtIGZpbHRlciBwaXBlbGluZSE=";

    unsafe {
        let mem_stream =
            camel_stream_mem_new_with_buffer(base64_input.as_ptr().cast(), base64_input.len());
        assert!(!mem_stream.is_null());

        let filter_stream = camel_stream_filter_new(mem_stream);
        assert!(!filter_stream.is_null());

        let source = camel_stream_filter_get_source(filter_stream.cast());
        assert_eq!(source, mem_stream);

        let dec_filter = camel_mime_filter_basic_new(CAMEL_MIME_FILTER_BASIC_BASE64_DEC);
        let filter_id = camel_stream_filter_add(filter_stream.cast(), dec_filter);
        assert!(filter_id >= 0);

        let mut buf = [0u8; 128];
        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        let n_read = camel_stream_read(
            filter_stream,
            buf.as_mut_ptr().cast(),
            buf.len(),
            std::ptr::null_mut(),
            &mut error,
        );
        assert!(n_read > 0);
        assert!(error.is_null());

        let decoded_text = std::str::from_utf8(&buf[..n_read as usize]).unwrap();
        assert_eq!(decoded_text, "Hello, JMAP stream filter pipeline!");

        camel_stream_filter_remove(filter_stream.cast(), filter_id);

        gobject_sys::g_object_unref(dec_filter.cast());
        gobject_sys::g_object_unref(filter_stream.cast());
        gobject_sys::g_object_unref(mem_stream.cast());
    }
}

/// Tests `CamelMimeParser` streaming, step states, header extraction, and content-type
/// parsing in EDS 3.52.
#[test]
fn camel_mime_parser_streaming_and_header_scanning_in_eds() {
    let raw_email = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>\r\n\
Subject: Parser Step Verification\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Body content line 1\r\n\
Body content line 2\r\n";

    unsafe {
        let parser = camel_mime_parser_new();
        assert!(!parser.is_null());

        let gbytes = glib_sys::g_bytes_new_static(raw_email.as_ptr().cast(), raw_email.len());
        assert!(!gbytes.is_null());

        camel_mime_parser_init_with_bytes(parser, gbytes);

        // Step 1: initial -> header
        let mut buf_ptr: *mut gchar = std::ptr::null_mut();
        let mut buf_len: gsize = 0;
        let state1 = camel_mime_parser_step(parser, &mut buf_ptr, &mut buf_len);
        assert_eq!(state1, CAMEL_MIME_PARSER_STATE_HEADER);

        // Inspect the header (the value includes the space after the colon)
        let mut offset: glib_sys::gint = 0;
        let subj = camel_mime_parser_header(parser, c"Subject".as_ptr(), &mut offset);
        assert!(!subj.is_null());
        let subj_str = std::ffi::CStr::from_ptr(subj).to_str().unwrap();
        assert_eq!(subj_str.trim(), "Parser Step Verification");

        let ct = camel_mime_parser_content_type(parser);
        assert!(!ct.is_null());
        assert_eq!(
            camel_content_type_is(ct, c"text".as_ptr(), c"plain".as_ptr()),
            glib_sys::GTRUE
        );

        // Step 2: header -> body
        let state2 = camel_mime_parser_step(parser, &mut buf_ptr, &mut buf_len);
        assert_eq!(state2, CAMEL_MIME_PARSER_STATE_BODY);

        // Step 3: body -> body end
        let state3 = camel_mime_parser_step(parser, &mut buf_ptr, &mut buf_len);
        assert_eq!(state3, CAMEL_MIME_PARSER_STATE_BODY_END);

        // Step 4: body end -> EOF
        let state4 = camel_mime_parser_step(parser, &mut buf_ptr, &mut buf_len);
        assert_eq!(state4, CAMEL_MIME_PARSER_STATE_EOF);

        glib_sys::g_bytes_unref(gbytes);
        gobject_sys::g_object_unref(parser.cast());
    }
}

/// Tests `CamelTrie` case-insensitive and case-sensitive multi-pattern search in EDS 3.52.
#[test]
fn camel_trie_multi_pattern_search_in_eds() {
    unsafe {
        let trie = camel_trie_new(glib_sys::GTRUE);
        assert!(!trie.is_null());

        camel_trie_add(trie, c"$seen".as_ptr(), 101);
        camel_trie_add(trie, c"$flagged".as_ptr(), 102);
        camel_trie_add(trie, c"$answered".as_ptr(), 103);
        camel_trie_add(trie, c"urgent".as_ptr(), 104);

        let text = b"Headers include: $SEEN, and urgent review flag";
        let mut matched_id: glib_sys::gint = 0;

        // Search first match ($SEEN -> 101)
        let match1 = camel_trie_search(trie, text.as_ptr().cast(), text.len(), &mut matched_id);
        assert!(!match1.is_null());
        assert_eq!(matched_id, 101);

        // Move past the matched pattern and search for the next match (urgent -> 104)
        let offset = match1.offset_from(text.as_ptr().cast()) as usize + 5;
        let remaining = &text[offset..];
        let match2 = camel_trie_search(
            trie,
            remaining.as_ptr().cast(),
            remaining.len(),
            &mut matched_id,
        );
        assert!(!match2.is_null());
        assert_eq!(matched_id, 104);

        camel_trie_free(trie);
    }
}

/// Tests `CamelUIDCache` creation, caching, querying, and serialization in EDS 3.52.
#[test]
fn camel_uid_cache_operations_in_eds() {
    unsafe {
        let mut tmp_error: *mut glib_sys::GError = std::ptr::null_mut();
        let tmp_dir_raw =
            glib_sys::g_dir_make_tmp(c"eds-uid-cache-XXXXXX".as_ptr(), &mut tmp_error);
        assert!(!tmp_dir_raw.is_null());
        assert!(tmp_error.is_null());

        let cache_path = format!(
            "{}/uids.cache\0",
            std::ffi::CStr::from_ptr(tmp_dir_raw).to_str().unwrap()
        );

        let cache = camel_uid_cache_new(cache_path.as_ptr().cast());
        assert!(!cache.is_null());

        camel_uid_cache_save_uid(cache, c"UID-1001".as_ptr());
        camel_uid_cache_save_uid(cache, c"UID-1002".as_ptr());

        // The uids of a listing that the cache has not seen. EDS 3.60 replaced
        // `camel_uid_cache_get_new_uids`/`_free_uids` with
        // `camel_uid_cache_dup_new_uids`. Its result is a usual reference-counted
        // `GPtrArray`: a different ownership contract, not a rename. No code in
        // this tree uses the uid cache. Thus the test examines the pre-3.60 API
        // only where it exists and does not abstract it. The creation, save and
        // destroy around this block run on both versions.
        #[cfg(camel_uid_cache_get_new_uids)]
        {
            // Make a query array with both old and new UIDs
            let query_array = glib_sys::g_ptr_array_new();
            glib_sys::g_ptr_array_add(query_array, glib_sys::g_strdup(c"UID-1001".as_ptr()).cast());
            glib_sys::g_ptr_array_add(query_array, glib_sys::g_strdup(c"UID-1002".as_ptr()).cast());
            glib_sys::g_ptr_array_add(query_array, glib_sys::g_strdup(c"UID-1003".as_ptr()).cast());
            glib_sys::g_ptr_array_add(query_array, glib_sys::g_strdup(c"UID-1004".as_ptr()).cast());

            let new_uids = camel_uid_cache_get_new_uids(cache, query_array.cast());
            assert!(!new_uids.is_null());
            assert_eq!((*new_uids).len, 2);

            let u1 = *(*new_uids).pdata as *const gchar;
            let u2 = *(*new_uids).pdata.add(1) as *const gchar;
            let s1 = std::ffi::CStr::from_ptr(u1).to_str().unwrap();
            let s2 = std::ffi::CStr::from_ptr(u2).to_str().unwrap();
            assert_eq!(s1, "UID-1003");
            assert_eq!(s2, "UID-1004");

            camel_uid_cache_free_uids(new_uids);
            glib_sys::g_ptr_array_free(query_array, glib_sys::GTRUE);
        }

        assert_eq!(camel_uid_cache_save(cache), glib_sys::GTRUE);
        camel_uid_cache_destroy(cache);
        glib_sys::g_free(tmp_dir_raw.cast());
    }
}

/// Tests `CamelCharset` character set resolution, stepwise detection, and ISO mapping in EDS 3.52.
#[test]
fn camel_charset_detection_and_iso_to_windows_in_eds() {
    unsafe {
        // For pure ASCII text, `camel_charset_best` returns NULL (no non-ASCII
        // encoding is necessary)
        let ascii_text = c"Simple ASCII content for testing";
        let best_ascii = camel_charset_best(ascii_text.as_ptr(), 32);
        assert!(best_ascii.is_null());

        // A UTF-8 multi-byte sequence gives UTF-8 or a compatible charset
        let utf8_sample = c"Umlaut \xc3\xa4\xc3\xb6\xc3\xbc detection";
        let best_utf8 = camel_charset_best(utf8_sample.as_ptr(), 22);
        assert!(!best_utf8.is_null());
        assert!(
            std::ffi::CStr::from_ptr(best_utf8)
                .to_str()
                .unwrap()
                .contains("UTF-8")
                || std::ffi::CStr::from_ptr(best_utf8)
                    .to_str()
                    .unwrap()
                    .contains("ISO-8859")
        );

        let win_charset = camel_charset_iso_to_windows(c"iso-8859-1".as_ptr());
        assert!(!win_charset.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(win_charset).to_str().unwrap(),
            "windows-cp1252"
        );

        let mut cc = std::mem::zeroed::<CamelCharset>();
        camel_charset_init(&mut cc);
        camel_charset_step(&mut cc, utf8_sample.as_ptr(), 22);
        let best_name = camel_charset_best_name(&mut cc);
        assert!(!best_name.is_null());
        assert!(
            std::ffi::CStr::from_ptr(best_name)
                .to_str()
                .unwrap()
                .contains("UTF-8")
                || std::ffi::CStr::from_ptr(best_name)
                    .to_str()
                    .unwrap()
                    .contains("ISO-8859")
        );
    }
}

/// Tests `CamelMimeFilterPreview`, `CamelMimeFilterCanon`, and `camel_text_to_html` in EDS 3.52.
#[test]
fn camel_mime_filter_preview_and_html_conversions_in_eds() {
    unsafe {
        // 1. Preview filter limit getter / setter
        let prev = camel_mime_filter_preview_new(100);
        assert!(!prev.is_null());
        assert_eq!(camel_mime_filter_preview_get_limit(prev.cast()), 100);
        camel_mime_filter_preview_set_limit(prev.cast(), 200);
        assert_eq!(camel_mime_filter_preview_get_limit(prev.cast()), 200);
        gobject_sys::g_object_unref(prev.cast());

        // 2. Canon filter
        let canon = camel_mime_filter_canon_new(
            CAMEL_MIME_FILTER_CANON_CRLF | CAMEL_MIME_FILTER_CANON_STRIP,
        );
        assert!(!canon.is_null());
        gobject_sys::g_object_unref(canon.cast());

        // 3. Text to HTML utility conversion
        let text_input = c"Visit https://example.com/portal for update\nLine 2";
        let html_out = camel_text_to_html(
            text_input.as_ptr(),
            CAMEL_MIME_FILTER_TOHTML_CONVERT_NL
                | CAMEL_MIME_FILTER_TOHTML_CONVERT_URLS
                | CAMEL_MIME_FILTER_TOHTML_CONVERT_SPACES,
            0,
        );
        assert!(!html_out.is_null());
        let html_str = std::ffi::CStr::from_ptr(html_out).to_str().unwrap();
        assert!(
            html_str
                .contains("<a href=\"https://example.com/portal\">https://example.com/portal</a>")
        );
        assert!(html_str.contains("<br>"));
        glib_sys::g_free(html_out.cast());
    }
}

/// Tests `CamelHTMLParser` data loading, stepwise tokenization, tag and attribute extraction
/// in EDS 3.52.
#[test]
fn camel_html_parser_tokenization_and_attributes_in_eds() {
    unsafe {
        let parser = camel_html_parser_new();
        assert!(!parser.is_null());

        let html_content =
            c"<p class=\"greeting\" id=\"p1\">Hello <strong>world</strong> &amp; welcome</p>";
        let raw_bytes = html_content.to_bytes();
        camel_html_parser_set_data(
            parser,
            html_content.as_ptr(),
            raw_bytes.len() as glib_sys::gint,
            1,
        );

        let mut data_ptr: *const gchar = std::ptr::null();
        let mut data_len: glib_sys::gint = 0;
        let mut states = Vec::new();
        let mut tags = Vec::new();

        loop {
            let st = camel_html_parser_step(parser, &mut data_ptr, &mut data_len);
            states.push(st);
            if st == CAMEL_HTML_PARSER_EOF || st == CAMEL_HTML_PARSER_EOD {
                break;
            }
            if st == CAMEL_HTML_PARSER_TAG || st == CAMEL_HTML_PARSER_ELEMENT {
                let tag = camel_html_parser_tag(parser);
                if !tag.is_null() {
                    let tag_name = std::ffi::CStr::from_ptr(tag)
                        .to_str()
                        .unwrap()
                        .to_lowercase();
                    tags.push(tag_name);
                }
                let class_attr = camel_html_parser_attr(parser, c"class".as_ptr());
                if !class_attr.is_null() {
                    assert_eq!(
                        std::ffi::CStr::from_ptr(class_attr).to_str().unwrap(),
                        "greeting"
                    );
                }
            }
        }

        assert!(
            states.contains(&CAMEL_HTML_PARSER_TAG) || states.contains(&CAMEL_HTML_PARSER_ELEMENT)
        );
        assert!(tags.iter().any(|t| t == "p" || t == "strong"));

        gobject_sys::g_object_unref(parser.cast());
    }
}

/// Tests `CamelUrlScanner` pattern registration and URL detection in EDS 3.52.
#[test]
fn camel_url_scanner_detection_and_matching_in_eds() {
    unsafe {
        let scanner = camel_url_scanner_new();
        assert!(!scanner.is_null());

        let mut pattern = CamelUrlPattern {
            pattern: c"https://".as_ptr(),
            prefix: std::ptr::null(),
            start: Some(camel_url_web_start),
            end: Some(camel_url_web_end),
        };
        camel_url_scanner_add(scanner, &mut pattern);

        let text = b"Read specification at https://jmap.io/spec.html for details";
        let mut match_info = std::mem::zeroed::<CamelUrlMatch>();

        let found =
            camel_url_scanner_scan(scanner, text.as_ptr().cast(), text.len(), &mut match_info);
        assert_eq!(found, glib_sys::GTRUE);
        assert!(match_info.um_so >= 0);
        assert!(match_info.um_eo > match_info.um_so);

        let matched_slice = &text[match_info.um_so as usize..match_info.um_eo as usize];
        let matched_str = std::str::from_utf8(matched_slice).unwrap();
        assert_eq!(matched_str, "https://jmap.io/spec.html");

        camel_url_scanner_free(scanner);
    }
}

/// Tests `CamelMimeFilterEnriched` and `camel_enriched_to_html` in EDS 3.52.
#[test]
fn camel_mime_filter_enriched_and_conversion_in_eds() {
    unsafe {
        let filter = camel_mime_filter_enriched_new(CAMEL_MIME_FILTER_ENRICHED_NONE);
        assert!(!filter.is_null());
        gobject_sys::g_object_unref(filter.cast());

        let enriched_input = c"<bold>Meeting Agenda</bold><italic>Review</italic>";
        let html_output = camel_enriched_to_html(enriched_input.as_ptr(), 0);
        assert!(!html_output.is_null());
        let converted = std::ffi::CStr::from_ptr(html_output).to_str().unwrap();
        assert!(
            converted.contains("<bold>")
                || converted.contains("<b>")
                || converted.contains("<italic>")
                || converted.contains("<i>")
        );
        assert!(converted.contains("Meeting Agenda"));
        assert!(converted.contains("Review"));
        glib_sys::g_free(html_output.cast());
    }
}

/// Tests `CamelMimeFilterHTML` HTML-to-plaintext conversion in EDS 3.52.
#[test]
fn camel_mime_filter_html_to_plaintext_in_eds() {
    unsafe {
        let filter = camel_mime_filter_html_new();
        assert!(!filter.is_null());

        let html_input = b"<html><head><title>Test</title></head><body><h1>Heading</h1><p>Paragraph text.</p></body></html>";

        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            filter,
            html_input.as_ptr().cast(),
            html_input.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut extracted = Vec::new();
        if out_len > 0 && !out_ptr.is_null() {
            extracted.extend_from_slice(std::slice::from_raw_parts(out_ptr as *const u8, out_len));
        }

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;

        let empty = c"";
        camel_mime_filter_complete(
            filter,
            empty.as_ptr(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );

        if comp_len > 0 && !comp_ptr.is_null() {
            extracted
                .extend_from_slice(std::slice::from_raw_parts(comp_ptr as *const u8, comp_len));
        }

        let extracted_text = String::from_utf8_lossy(&extracted);
        assert!(extracted_text.contains("Heading") || extracted_text.contains("Paragraph text."));
        assert!(!extracted_text.contains("<html>"));
        assert!(!extracted_text.contains("<body>"));

        gobject_sys::g_object_unref(filter.cast());
    }
}

/// Tests `CamelMimeFilterGZip` compression and decompression in EDS 3.52.
#[test]
fn camel_mime_filter_gzip_compression_and_decompression_in_eds() {
    let payload = b"Compression test payload for CamelMimeFilterGZip in Evolution Data Server.";

    unsafe {
        // Compress
        let zip_filter = camel_mime_filter_gzip_new(CAMEL_MIME_FILTER_GZIP_MODE_ZIP, 6);
        assert!(!zip_filter.is_null());

        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            zip_filter,
            payload.as_ptr().cast(),
            payload.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut compressed = Vec::new();
        if out_len > 0 && !out_ptr.is_null() {
            compressed.extend_from_slice(std::slice::from_raw_parts(out_ptr as *const u8, out_len));
        }

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;
        let empty = c"";

        camel_mime_filter_complete(
            zip_filter,
            empty.as_ptr(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );

        if comp_len > 0 && !comp_ptr.is_null() {
            compressed
                .extend_from_slice(std::slice::from_raw_parts(comp_ptr as *const u8, comp_len));
        }

        assert!(compressed.len() >= 10);
        // Gzip header magic bytes: 0x1f, 0x8b
        assert_eq!(compressed[0], 0x1f);
        assert_eq!(compressed[1], 0x8b);

        gobject_sys::g_object_unref(zip_filter.cast());

        // Decompress
        let unzip_filter = camel_mime_filter_gzip_new(CAMEL_MIME_FILTER_GZIP_MODE_UNZIP, 0);
        assert!(!unzip_filter.is_null());

        let mut dec_out_ptr: *mut gchar = std::ptr::null_mut();
        let mut dec_out_len: gsize = 0;
        let mut dec_out_pre: gsize = 0;

        camel_mime_filter_filter(
            unzip_filter,
            compressed.as_ptr().cast(),
            compressed.len(),
            0,
            &mut dec_out_ptr,
            &mut dec_out_len,
            &mut dec_out_pre,
        );

        let mut decompressed = Vec::new();
        if dec_out_len > 0 && !dec_out_ptr.is_null() {
            decompressed.extend_from_slice(std::slice::from_raw_parts(
                dec_out_ptr as *const u8,
                dec_out_len,
            ));
        }

        let mut dec_comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut dec_comp_len: gsize = 0;
        let mut dec_comp_pre: gsize = 0;

        camel_mime_filter_complete(
            unzip_filter,
            empty.as_ptr(),
            0,
            0,
            &mut dec_comp_ptr,
            &mut dec_comp_len,
            &mut dec_comp_pre,
        );

        if dec_comp_len > 0 && !dec_comp_ptr.is_null() {
            decompressed.extend_from_slice(std::slice::from_raw_parts(
                dec_comp_ptr as *const u8,
                dec_comp_len,
            ));
        }

        assert_eq!(decompressed.as_slice(), payload);

        gobject_sys::g_object_unref(unzip_filter.cast());
    }
}

/// Tests `CamelMimeFilterWindows` Windows charset detection and alias resolution in EDS 3.52.
#[test]
fn camel_mime_filter_windows_charset_aliases_in_eds() {
    unsafe {
        let win_filter = camel_mime_filter_windows_new(c"iso-8859-1".as_ptr());
        assert!(!win_filter.is_null());

        // At first, the filter has seen no Windows characters
        assert_eq!(
            camel_mime_filter_windows_is_windows_charset(win_filter.cast()),
            glib_sys::GFALSE
        );

        // Feed bytes with a byte specific to Windows-1252 (0x93: left curly double quote)
        let win1252_bytes = [b'A', b'B', 0x93, b'C'];
        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            win_filter,
            win1252_bytes.as_ptr().cast(),
            win1252_bytes.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        // Now the filter detects a Windows charset
        assert_eq!(
            camel_mime_filter_windows_is_windows_charset(win_filter.cast()),
            glib_sys::GTRUE
        );
        let real_cs = camel_mime_filter_windows_real_charset(win_filter.cast());
        assert!(!real_cs.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(real_cs).to_str().unwrap(),
            "windows-cp1252"
        );

        gobject_sys::g_object_unref(win_filter.cast());

        // For pure ASCII input, the filter does not detect a Windows charset
        let ascii_filter = camel_mime_filter_windows_new(c"iso-8859-1".as_ptr());
        assert!(!ascii_filter.is_null());
        let ascii_bytes = b"Pure ASCII content";
        camel_mime_filter_filter(
            ascii_filter,
            ascii_bytes.as_ptr().cast(),
            ascii_bytes.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );
        assert_eq!(
            camel_mime_filter_windows_is_windows_charset(ascii_filter.cast()),
            glib_sys::GFALSE
        );
        let ascii_cs = camel_mime_filter_windows_real_charset(ascii_filter.cast());
        assert!(!ascii_cs.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(ascii_cs).to_str().unwrap(),
            "iso-8859-1"
        );
        gobject_sys::g_object_unref(ascii_filter.cast());
    }
}

/// Tests `CamelMimeFilterBestenc` encoding and charset detection in EDS 3.52.
#[test]
fn camel_mime_filter_bestenc_detection_in_eds() {
    unsafe {
        // 1. Text with 7-bit ASCII
        let flags =
            CAMEL_BESTENC_GET_ENCODING | CAMEL_BESTENC_GET_CHARSET | CAMEL_BESTENC_LF_IS_CRLF;
        let filter = camel_mime_filter_bestenc_new(flags as glib_sys::guint);
        assert!(!filter.is_null());

        let text = b"Hello, this is pure ASCII content.\r\nLine 2.\r\n";
        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            filter,
            text.as_ptr().cast(),
            text.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;
        let empty = [0u8; 0];
        camel_mime_filter_complete(
            filter,
            empty.as_ptr().cast(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );

        let best_enc = camel_mime_filter_bestenc_get_best_encoding(
            filter.cast(),
            CAMEL_BESTENC_7BIT | CAMEL_BESTENC_TEXT,
        );
        assert!(
            best_enc == CAMEL_TRANSFER_ENCODING_7BIT
                || best_enc == CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE
        );

        let best_cs = camel_mime_filter_bestenc_get_best_charset(filter.cast());
        // For pure ASCII, Camel returns NULL (the standard default) or "US-ASCII"
        if !best_cs.is_null() {
            assert_eq!(
                std::ffi::CStr::from_ptr(best_cs).to_str().unwrap(),
                "US-ASCII"
            );
        }

        gobject_sys::g_object_unref(filter.cast());

        // 2. Text with 8-bit characters (e.g. UTF-8 ä ö ü)
        let filter8 = camel_mime_filter_bestenc_new(flags as glib_sys::guint);
        assert!(!filter8.is_null());
        let text8 = "Grüße aus Berlin! Überprüfung.\r\n".as_bytes();
        camel_mime_filter_filter(
            filter8,
            text8.as_ptr().cast(),
            text8.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );
        camel_mime_filter_complete(
            filter8,
            empty.as_ptr().cast(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );

        let best_enc8 = camel_mime_filter_bestenc_get_best_encoding(
            filter8.cast(),
            CAMEL_BESTENC_7BIT | CAMEL_BESTENC_TEXT,
        );
        assert!(
            best_enc8 == CAMEL_TRANSFER_ENCODING_QUOTEDPRINTABLE
                || best_enc8 == CAMEL_TRANSFER_ENCODING_BASE64
                || best_enc8 == CAMEL_TRANSFER_ENCODING_8BIT
        );

        let best_cs8 = camel_mime_filter_bestenc_get_best_charset(filter8.cast());
        assert!(!best_cs8.is_null());
        assert!(
            std::ffi::CStr::from_ptr(best_cs8)
                .to_str()
                .unwrap()
                .contains("UTF-8")
                || std::ffi::CStr::from_ptr(best_cs8)
                    .to_str()
                    .unwrap()
                    .contains("ISO-8859")
        );

        gobject_sys::g_object_unref(filter8.cast());
    }
}

/// Tests `CamelMimeFilterCharset` UTF-8 to ISO-8859-1 conversion in EDS 3.52.
#[test]
fn camel_mime_filter_charset_conversion_in_eds() {
    unsafe {
        let filter = camel_mime_filter_charset_new(c"UTF-8".as_ptr(), c"ISO-8859-1".as_ptr());
        assert!(!filter.is_null());

        let utf8_input = "Café".as_bytes();
        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            filter,
            utf8_input.as_ptr().cast(),
            utf8_input.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut result = Vec::new();
        if out_len > 0 && !out_ptr.is_null() {
            result.extend_from_slice(std::slice::from_raw_parts(out_ptr as *const u8, out_len));
        }

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;
        let empty = [0u8; 0];
        camel_mime_filter_complete(
            filter,
            empty.as_ptr().cast(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );
        if comp_len > 0 && !comp_ptr.is_null() {
            result.extend_from_slice(std::slice::from_raw_parts(comp_ptr as *const u8, comp_len));
        }

        assert_eq!(result.as_slice(), &[b'C', b'a', b'f', 0xE9]);

        gobject_sys::g_object_unref(filter.cast());
    }
}

/// Tests `CamelMimeFilterFrom` mbox "From " line escaping in EDS 3.52.
#[test]
fn camel_mime_filter_from_escaping_in_eds() {
    unsafe {
        let filter = camel_mime_filter_from_new();
        assert!(!filter.is_null());

        let mbox_content = b"From sender@example.com Sat Jan 15 12:00:00 2026\nSubject: Test\n\nBody line.\nFrom inside text\n";
        let mut out_ptr: *mut gchar = std::ptr::null_mut();
        let mut out_len: gsize = 0;
        let mut out_pre: gsize = 0;

        camel_mime_filter_filter(
            filter,
            mbox_content.as_ptr().cast(),
            mbox_content.len(),
            0,
            &mut out_ptr,
            &mut out_len,
            &mut out_pre,
        );

        let mut escaped = Vec::new();
        if out_len > 0 && !out_ptr.is_null() {
            escaped.extend_from_slice(std::slice::from_raw_parts(out_ptr as *const u8, out_len));
        }

        let mut comp_ptr: *mut gchar = std::ptr::null_mut();
        let mut comp_len: gsize = 0;
        let mut comp_pre: gsize = 0;
        let empty = [0u8; 0];
        camel_mime_filter_complete(
            filter,
            empty.as_ptr().cast(),
            0,
            0,
            &mut comp_ptr,
            &mut comp_len,
            &mut comp_pre,
        );
        if comp_len > 0 && !comp_ptr.is_null() {
            escaped.extend_from_slice(std::slice::from_raw_parts(comp_ptr as *const u8, comp_len));
        }

        let escaped_str = String::from_utf8_lossy(&escaped);
        assert!(escaped_str.contains(">From sender@example.com"));
        assert!(escaped_str.contains(">From inside text"));

        gobject_sys::g_object_unref(filter.cast());
    }
}

/// Tests `CamelMimeFilterYenc` and yEnc step encoding/decoding in EDS 3.52.
#[test]
fn camel_mime_filter_yenc_encode_and_decode_in_eds() {
    unsafe {
        let input_data = b"Testing yEnc binary payload 0123456789 \x00\xFF\x0D\x0A.";
        let mut encoded_buf = vec![0u8; input_data.len() * 3 + 128];
        let mut state: glib_sys::gint = CAMEL_MIME_YENCODE_STATE_INIT as glib_sys::gint;
        let mut pcrc: u32 = 0;
        let mut crc: u32 = CAMEL_MIME_YENCODE_CRC_INIT as u32;

        let enc_len = camel_yencode_step(
            input_data.as_ptr(),
            input_data.len(),
            encoded_buf.as_mut_ptr(),
            &mut state,
            &mut pcrc,
            &mut crc,
        );

        let close_len = camel_yencode_close(
            input_data.as_ptr(),
            0,
            encoded_buf.as_mut_ptr().add(enc_len),
            &mut state,
            &mut pcrc,
            &mut crc,
        );
        let total_enc_len = enc_len + close_len;
        let final_crc = !crc;
        assert!(total_enc_len > 0);

        let mut decoded_buf = vec![0u8; input_data.len() + 128];
        let mut dec_state: glib_sys::gint = CAMEL_MIME_YDECODE_STATE_INIT as glib_sys::gint;
        let mut dec_pcrc: u32 = 0;
        let mut dec_crc: u32 = CAMEL_MIME_YENCODE_CRC_INIT as u32;

        let dec_len = camel_ydecode_step(
            encoded_buf.as_ptr(),
            total_enc_len,
            decoded_buf.as_mut_ptr(),
            &mut dec_state,
            &mut dec_pcrc,
            &mut dec_crc,
        );

        assert_eq!(&decoded_buf[..dec_len], input_data);
        assert_eq!(!dec_crc, final_crc);
    }
}

/// Tests `CamelMimeFilterProgress` and `CamelStreamBuffer` line reading in EDS 3.52.
#[test]
fn camel_mime_filter_progress_and_stream_buffer_in_eds() {
    unsafe {
        let prog_filter = camel_mime_filter_progress_new(std::ptr::null_mut(), 1024);
        assert!(!prog_filter.is_null());
        gobject_sys::g_object_unref(prog_filter.cast());

        let payload = b"First Line\nSecond Line\nThird Line\n";
        let mem_stream = camel_stream_mem_new_with_buffer(payload.as_ptr().cast(), payload.len());
        assert!(!mem_stream.is_null());

        let buf_stream = camel_stream_buffer_new(mem_stream, CAMEL_STREAM_BUFFER_READ);
        assert!(!buf_stream.is_null());

        let mut error: *mut glib_sys::GError = std::ptr::null_mut();
        let line1 =
            camel_stream_buffer_read_line(buf_stream.cast(), std::ptr::null_mut(), &mut error);
        assert!(!line1.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(line1).to_str().unwrap(),
            "First Line"
        );
        glib_sys::g_free(line1.cast());

        let line2 =
            camel_stream_buffer_read_line(buf_stream.cast(), std::ptr::null_mut(), &mut error);
        assert!(!line2.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(line2).to_str().unwrap(),
            "Second Line"
        );
        glib_sys::g_free(line2.cast());

        camel_stream_buffer_discard_cache(buf_stream.cast());

        gobject_sys::g_object_unref(buf_stream.cast());
        gobject_sys::g_object_unref(mem_stream.cast());
    }
}

/// Tests `CamelUTF8`, `CamelStringUtils`, and `CamelHostnameUtils` in EDS 3.52.
#[test]
fn camel_utf8_and_string_utilities_in_eds() {
    unsafe {
        // 1. UTF-8 Validation
        let valid_text = c"Valid UTF-8 string: Grüß Gott!";
        let checked = camel_utf8_make_valid(valid_text.as_ptr());
        assert!(!checked.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(checked).to_str().unwrap(),
            "Valid UTF-8 string: Grüß Gott!"
        );
        glib_sys::g_free(checked.cast());

        let invalid_bytes = [b'A', b'B', 0xFF, 0xFE, b'C', 0];
        let sanitized = camel_utf8_make_valid(invalid_bytes.as_ptr().cast());
        assert!(!sanitized.is_null());
        let san_str = std::ffi::CStr::from_ptr(sanitized).to_str().unwrap();
        assert!(san_str.starts_with("AB"));
        assert!(san_str.ends_with("C"));
        glib_sys::g_free(sanitized.cast());

        // 2. UTF-7 / UTF-8 conversion (Modified IMAP UTF-7)
        let utf8_folder = c"Entw&APw-rfe";
        let converted_utf8 = camel_utf7_utf8(utf8_folder.as_ptr());
        assert!(!converted_utf8.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(converted_utf8).to_str().unwrap(),
            "Entwürfe"
        );

        let roundtrip_utf7 = camel_utf8_utf7(converted_utf8);
        assert!(!roundtrip_utf7.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(roundtrip_utf7).to_str().unwrap(),
            "Entw&APw-rfe"
        );
        glib_sys::g_free(converted_utf8.cast());
        glib_sys::g_free(roundtrip_utf7.cast());

        // 3. String Utilities
        assert_ne!(
            camel_string_is_all_ascii(c"pure ascii".as_ptr()),
            glib_sys::GFALSE
        );
        assert_eq!(
            camel_string_is_all_ascii(c"Müller".as_ptr()),
            glib_sys::GFALSE
        );

        assert_ne!(
            camel_strcase_equal(
                c"Hello World".as_ptr().cast(),
                c"hello world".as_ptr().cast()
            ),
            0
        );
        assert_eq!(
            camel_strcase_hash(c"ABC".as_ptr().cast()),
            camel_strcase_hash(c"abc".as_ptr().cast())
        );

        let found_sub =
            camel_strstrcase(c"Header: Content-Type".as_ptr(), c"content-type".as_ptr());
        assert!(!found_sub.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(found_sub).to_str().unwrap(),
            "Content-Type"
        );

        // 4. Hostname Utils
        assert_eq!(
            camel_hostname_utils_requires_ascii(c"mail.example.com".as_ptr()),
            glib_sys::GFALSE
        );
        assert_ne!(
            camel_hostname_utils_requires_ascii(c"münchen.example.com".as_ptr()),
            glib_sys::GFALSE
        );
    }
}

/// Tests `CamelSExp` S-expression parsing and evaluation in EDS 3.52.
#[test]
fn camel_sexp_parsing_and_evaluation_in_eds() {
    unsafe {
        let sexp = camel_sexp_new();
        assert!(!sexp.is_null());

        let expr = c"(and (= 1 1) (< 2 5))";
        camel_sexp_input_text(sexp, expr.as_ptr(), expr.to_bytes().len() as glib_sys::gint);

        let parsed = camel_sexp_parse(sexp);
        assert_eq!(parsed, 0, "S-expression failed to parse");

        let result = camel_sexp_eval(sexp);
        assert!(!result.is_null());
        assert_eq!((*result).type_, CAMEL_SEXP_RES_BOOL);
        assert_ne!(*(*result).value.boolean.as_ref(), 0);

        camel_sexp_result_free(sexp, result);
        gobject_sys::g_object_unref(sexp.cast());
    }
}

/// Tests `CamelFolderSearch` in EDS 3.52.
///
/// 3.60 removed it: `CamelStoreSearch` replaced the whole object, and only
/// `camel_folder_search_sync` and its two siblings stay on `CamelFolder`. The
/// folder search of `jmap-mail` calls the removed API, so this also marks a port,
/// not only a missing probe.
#[cfg(camel_folder_search_object)]
#[test]
fn camel_folder_search_utilities_in_eds() {
    unsafe {
        let search = camel_folder_search_new();
        assert!(!search.is_null());

        // 1. Message ID hashing
        let msg_id = c"<msg-12345@example.com>";
        let hash1 = camel_folder_search_util_hash_message_id(msg_id.as_ptr(), glib_sys::GFALSE);
        let hash2 = camel_folder_search_util_hash_message_id(msg_id.as_ptr(), glib_sys::GFALSE);
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, 0);

        // 2. Date comparison helper (compares dates only). `compare_date` uses
        // local time, so these values are at UTC midday. No local timezone in the
        // -12..+14 range can move dt1/dt2 across a day boundary, or keep dt3 on
        // the same day.
        let dt1 = 1700049600i64; // 2023-11-15 12:00:00 UTC
        let dt2 = 1700053200i64; // 2023-11-15 13:00:00 UTC, same day
        assert_eq!(camel_folder_search_util_compare_date(dt1, dt2), 0);
        let dt3 = 1700136000i64; // 2023-11-16 12:00:00 UTC, next day
        assert!(camel_folder_search_util_compare_date(dt1, dt3) < 0);

        // 3. Month addition helper
        let now = 1700000000 as eds_sys::time_t;
        let future = camel_folder_search_util_add_months(now, 2);
        assert!(future > now);

        // 4. Folder search cached messages configuration
        camel_folder_search_set_only_cached_messages(search, glib_sys::GTRUE);
        assert_ne!(
            camel_folder_search_get_only_cached_messages(search),
            glib_sys::GFALSE
        );

        gobject_sys::g_object_unref(search.cast());
    }
}

/// Tests `CamelFolderThread` boxed type in EDS 3.52.
///
/// 3.60 has no boxed `CamelFolderThread`: threading moved behind
/// `CamelStoreSearch`. The name is now on `camel_folder_thread_flags_get_type`,
/// which is the `GType` of the enum, a different thing. No code in this tree
/// threads messages, so on 3.60 this probe is absent and no port is necessary.
#[cfg(camel_folder_thread_boxed)]
#[test]
fn camel_folder_thread_messages_in_eds() {
    unsafe {
        let gtype = camel_folder_thread_messages_get_type();
        assert_ne!(gtype, G_TYPE_INVALID);
        assert_eq!(g_type_fundamental(gtype), gobject_sys::G_TYPE_BOXED);
    }
}

/// Tests `CamelOperation` in EDS 3.52.
#[test]
fn camel_operation_and_cancellable_in_eds() {
    unsafe {
        let cancellable = camel_operation_new();
        assert!(!cancellable.is_null());

        let msg = c"Syncing JMAP mailbox";
        camel_operation_push_message(cancellable, msg.as_ptr());

        let dup = camel_operation_dup_message(cancellable);
        assert!(!dup.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(dup).to_str().unwrap(),
            "Syncing JMAP mailbox"
        );
        glib_sys::g_free(dup.cast());

        camel_operation_progress(cancellable, 50);

        camel_operation_pop_message(cancellable);

        let dup_after_pop = camel_operation_dup_message(cancellable);
        assert!(dup_after_pop.is_null());

        gobject_sys::g_object_unref(cancellable.cast());
    }
}

/// Tests `CamelNNTPAddress` in EDS 3.52.
#[test]
fn camel_nntp_address_parsing_and_formatting_in_eds() {
    unsafe {
        let addr = camel_nntp_address_new();
        assert!(!addr.is_null());

        assert_eq!(camel_nntp_address_add(addr, c"comp.lang.rust".as_ptr()), 0);
        assert_eq!(camel_nntp_address_add(addr, c"gnome.evolution".as_ptr()), 1);

        let mut name: *const glib_sys::gchar = std::ptr::null();
        assert_ne!(camel_nntp_address_get(addr, 0, &mut name), glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(name).to_str().unwrap(),
            "comp.lang.rust"
        );

        assert_ne!(camel_nntp_address_get(addr, 1, &mut name), glib_sys::GFALSE);
        assert_eq!(
            std::ffi::CStr::from_ptr(name).to_str().unwrap(),
            "gnome.evolution"
        );

        let formatted = camel_address_format(addr.cast());
        assert!(!formatted.is_null());
        let fmt_str = std::ffi::CStr::from_ptr(formatted).to_str().unwrap();
        assert!(fmt_str.contains("comp.lang.rust"));
        assert!(fmt_str.contains("gnome.evolution"));
        glib_sys::g_free(formatted.cast());

        gobject_sys::g_object_unref(addr.cast());
    }
}

/// Tests `CamelFileUtils` in EDS 3.52.
#[test]
fn camel_file_utilities_in_eds() {
    unsafe {
        let unsafe_name = c"report/subfolder/test_filename.eml";
        let safe_name = camel_file_util_safe_filename(unsafe_name.as_ptr());
        assert!(!safe_name.is_null());
        let safe_str = std::ffi::CStr::from_ptr(safe_name).to_str().unwrap();
        assert!(!safe_str.contains('/'));
        glib_sys::g_free(safe_name.cast());

        let savename = camel_file_util_savename(c"/var/mail/inbox.dat".as_ptr());
        assert!(!savename.is_null());
        let save_str = std::ffi::CStr::from_ptr(savename).to_str().unwrap();
        assert!(save_str.contains("inbox"));
        glib_sys::g_free(savename.cast());
    }
}

/// Tests `CamelLocalSettings` and `CamelStoreSettings` in EDS 3.52.
#[test]
fn camel_local_and_store_settings_in_eds() {
    unsafe {
        let settings = gobject_sys::g_object_new(
            camel_local_settings_get_type(),
            std::ptr::null::<std::os::raw::c_char>(),
        ) as *mut CamelLocalSettings;
        assert!(!settings.is_null());

        camel_local_settings_set_path(settings, c"/tmp/jmap-test-cache".as_ptr());
        let path = camel_local_settings_get_path(settings);
        assert_eq!(
            std::ffi::CStr::from_ptr(path).to_str().unwrap(),
            "/tmp/jmap-test-cache"
        );

        camel_local_settings_set_filter_all(settings, glib_sys::GTRUE);
        assert_ne!(
            camel_local_settings_get_filter_all(settings),
            glib_sys::GFALSE
        );

        camel_local_settings_set_filter_junk(settings, glib_sys::GTRUE);
        assert_ne!(
            camel_local_settings_get_filter_junk(settings),
            glib_sys::GFALSE
        );

        camel_local_settings_set_maildir_alt_flag_sep(settings, glib_sys::GTRUE);
        assert_ne!(
            camel_local_settings_get_maildir_alt_flag_sep(settings),
            glib_sys::GFALSE
        );

        // Store settings properties that CamelLocalSettings inherits
        let store_settings: *mut CamelStoreSettings = settings.cast();
        camel_store_settings_set_filter_inbox(store_settings, glib_sys::GTRUE);
        assert_ne!(
            camel_store_settings_get_filter_inbox(store_settings),
            glib_sys::GFALSE
        );

        camel_store_settings_set_store_changes_interval(store_settings, 120);
        assert_eq!(
            camel_store_settings_get_store_changes_interval(store_settings),
            120
        );

        gobject_sys::g_object_unref(settings.cast());
    }
}

/// Tests `CamelCertDB` and `CamelCert` certificate database operations in EDS 3.52.
#[test]
fn camel_certdb_operations_and_host_certificates_in_eds() {
    unsafe {
        // 1. CamelCert creation, field manipulation, ref and unref
        let cert = camel_cert_new();
        assert!(!cert.is_null());
        assert_eq!((*cert).refcount, 1);
        (*cert).hostname = glib_sys::g_strdup(c"mail.example.com".as_ptr());
        (*cert).fingerprint = glib_sys::g_strdup(c"01:23:45:67:89:AB:CD:EF".as_ptr());
        (*cert).trust = CAMEL_CERT_TRUST_FULLY;

        assert_eq!(
            std::ffi::CStr::from_ptr((*cert).hostname).to_str().unwrap(),
            "mail.example.com"
        );
        assert_eq!(
            std::ffi::CStr::from_ptr((*cert).fingerprint)
                .to_str()
                .unwrap(),
            "01:23:45:67:89:AB:CD:EF"
        );
        assert_eq!((*cert).trust, CAMEL_CERT_TRUST_FULLY);

        let cert_ref = camel_cert_ref(cert);
        assert_eq!(cert_ref, cert);
        assert_eq!((*cert).refcount, 2);
        camel_cert_unref(cert_ref);
        assert_eq!((*cert).refcount, 1);
        camel_cert_unref(cert);

        // 2. CamelCertTrust enum values
        assert_eq!(CAMEL_CERT_TRUST_UNKNOWN, 0);
        assert_eq!(CAMEL_CERT_TRUST_NEVER, 1);
        assert_eq!(CAMEL_CERT_TRUST_MARGINAL, 2);
        assert_eq!(CAMEL_CERT_TRUST_FULLY, 3);
        assert_eq!(CAMEL_CERT_TRUST_ULTIMATE, 4);
        assert_eq!(CAMEL_CERT_TRUST_TEMPORARY, 5);

        // 3. CamelCertDB creation, filename setting, and saving
        let certdb = camel_certdb_new();
        assert!(!certdb.is_null());

        let mut tmp_error: *mut glib_sys::GError = std::ptr::null_mut();
        let tmp_dir = glib_sys::g_dir_make_tmp(c"eds-certdb-XXXXXX".as_ptr(), &mut tmp_error);
        assert!(!tmp_dir.is_null());
        assert!(tmp_error.is_null());

        let db_path = format!(
            "{}/certs.db\0",
            std::ffi::CStr::from_ptr(tmp_dir).to_str().unwrap()
        );
        camel_certdb_set_filename(certdb, db_path.as_ptr().cast());
        assert_eq!(camel_certdb_save(certdb), 0);

        camel_certdb_clear(certdb);
        gobject_sys::g_object_unref(certdb.cast());
        glib_sys::g_free(tmp_dir.cast());
    }
}

/// Tests `CamelJunkFilter` interface and status constants in EDS 3.52.
#[test]
fn camel_junk_filter_interface_and_status_in_eds() {
    unsafe {
        let gtype = camel_junk_filter_get_type();
        assert_ne!(gtype, 0);
        assert_ne!(
            gobject_sys::g_type_is_a(gtype, gobject_sys::G_TYPE_INTERFACE),
            glib_sys::GFALSE
        );

        // Enum constants
        assert_eq!(CAMEL_JUNK_STATUS_ERROR, 0);
        assert_eq!(CAMEL_JUNK_STATUS_INCONCLUSIVE, 1);
        assert_eq!(CAMEL_JUNK_STATUS_MESSAGE_IS_JUNK, 2);
        assert_eq!(CAMEL_JUNK_STATUS_MESSAGE_IS_NOT_JUNK, 3);
    }
}

/// Tests `CamelNetworkService` interface in EDS 3.52.
#[test]
fn camel_network_service_interface_in_eds() {
    unsafe {
        let gtype = camel_network_service_get_type();
        assert_ne!(gtype, 0);
        assert_ne!(
            gobject_sys::g_type_is_a(gtype, gobject_sys::G_TYPE_INTERFACE),
            glib_sys::GFALSE
        );
    }
}

/// Tests `CamelMemPool` allocation and lifecycle in EDS 3.52.
#[test]
fn camel_mempool_allocation_and_flush_in_eds() {
    unsafe {
        let pool = camel_mempool_new(1024, 256, CAMEL_MEMPOOL_ALIGN_STRUCT);
        assert!(!pool.is_null());

        let mem = camel_mempool_alloc(pool, 128);
        assert!(!mem.is_null());
        std::ptr::write_bytes(mem as *mut u8, 0xAB, 128);

        let dup = camel_mempool_strdup(pool, c"JMAP memory pool test string".as_ptr());
        assert!(!dup.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(dup).to_str().unwrap(),
            "JMAP memory pool test string"
        );

        camel_mempool_flush(pool, 0);
        camel_mempool_destroy(pool);
    }
}

/// Tests `CamelTextIndex` creation and word additions in EDS 3.52.
#[test]
fn camel_text_index_creation_and_words_in_eds() {
    unsafe {
        let mut tmp_error: *mut glib_sys::GError = std::ptr::null_mut();
        let tmp_dir = glib_sys::g_dir_make_tmp(c"eds-index-XXXXXX".as_ptr(), &mut tmp_error);
        assert!(!tmp_dir.is_null());
        assert!(tmp_error.is_null());

        let idx_path = format!(
            "{}/mail.index\0",
            std::ffi::CStr::from_ptr(tmp_dir).to_str().unwrap()
        );

        // O_RDWR (2) | O_CREAT (64) | O_TRUNC (512) = 578
        let idx = camel_text_index_new(idx_path.as_ptr().cast(), 578);
        assert!(!idx.is_null());

        let name = camel_index_add_name(idx.cast(), c"subject_field".as_ptr());
        assert!(!name.is_null());

        camel_index_name_add_word(name, c"roadmap".as_ptr());
        camel_index_name_add_word(name, c"milestone".as_ptr());

        let buf = c"Search keywords and token stream";
        // add_buffer returns the number of bytes *remaining* (not processed).
        // CamelTextIndex processes all input incrementally, so it always returns 0.
        let remaining = camel_index_name_add_buffer(name, buf.as_ptr(), 32);
        assert_eq!(remaining, 0);

        // Commit the indexed words to the index, then flush to disk.
        assert_eq!(camel_index_write_name(idx.cast(), name), 0);
        assert_eq!(camel_index_sync(idx.cast()), 0);

        gobject_sys::g_object_unref(name.cast());
        gobject_sys::g_object_unref(idx.cast());

        assert_eq!(camel_text_index_check(idx_path.as_ptr().cast()), 0);

        glib_sys::g_free(tmp_dir.cast());
    }
}

/// Tests `CamelFilterSearch` search constants in EDS 3.52.
#[test]
fn camel_filter_search_constants_in_eds() {
    assert_eq!(CAMEL_SEARCH_ERROR, -1);
    assert_eq!(CAMEL_SEARCH_NOMATCH, 0);
    assert_eq!(CAMEL_SEARCH_MATCHED, 1);
}
