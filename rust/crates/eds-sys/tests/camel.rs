// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The one struct M5's mail provider hands to C by value: `CamelProvider`.
// Every other type crossed so far is a GObject, whose layout `tests/layout.rs`
// checks against `g_type_query`. A provider is not — it is a plain struct
// behind a boxed GType, so GObject knows nothing about its size and there is
// nothing to query. What stands in for that check here is a round trip: build
// one in Rust, register it, and ask Camel to hand it back. If a single field
// were at the wrong offset, the values that come back would be someone else's.

use eds_sys::*;
use std::ptr;

/// Why this file exists rather than three more lines in `tests/layout.rs`:
/// `camel_provider_get_type()` registers a *boxed* type, so `g_type_query()`
/// reports zero for both sizes and the `assert_layout!` macro would compare
/// `size_of` against nothing at all and pass no matter what.
#[test]
fn a_provider_is_a_boxed_type_so_gtype_knows_nothing_of_its_size() {
    // SAFETY: plain type accessor, and `g_type_query` fills a struct we own.
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

/// The provider struct is a static description, so the test builds it the way
/// the real module will: `'static` C string literals, no allocation, no
/// ownership handed over. `camel_provider_register` takes the pointer and
/// keeps it, which is why the module's provider has to outlive the module.
#[test]
fn a_provider_built_in_rust_reads_back_out_of_camels_registry() {
    // `camel_provider_init()` is what scans the provider directory, so this is
    // the state a loaded `libcameljmap.so` will find itself in — Camel calls
    // `camel_provider_module_init` from inside it. Registering works without it
    // (the table is created lazily, checked), but a test of the module's path
    // should stand where the module stands.
    // SAFETY: no arguments, and repeated calls are harmless (checked).
    let store_type = unsafe {
        camel_provider_init();
        camel_offline_store_get_type()
    };

    // A protocol nobody else registers, so the test cannot collide with a
    // provider Camel loaded from disk.
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
        auto_detect: None,
        // The array is indexed by CamelProviderType; a store and no transport.
        object_types: [store_type, G_TYPE_INVALID],
        authtypes: ptr::null_mut(),
        url_hash: None,
        url_equal: None,
        translation_domain: ptr::null(),
        priv_: ptr::null_mut(),
    };

    // SAFETY: `provider` outlives the call and everything it points at is
    // 'static. `camel_provider_get` returns the registered pointer or NULL
    // with `error` set; on the happy path it is the very struct above.
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

/// `object_types` is a fixed-size array in the struct, and its length is an
/// enum value in a different header. If EDS ever grew a third provider type
/// the array would move `authtypes` and everything after it, so the length is
/// worth pinning rather than trusting bindgen to have followed the `#define`.
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

/// How the manual test recipe will point Camel at an uninstalled build, and a
/// string Camel compares against `g_getenv`'s key — so a typo is a provider
/// that is simply never found. Take it from the header.
#[test]
fn the_provider_directory_override_is_the_variable_camel_reads() {
    assert_eq!(EDS_CAMEL_PROVIDER_DIR, c"EDS_CAMEL_PROVIDER_DIR");
}

/// `CamelFolderInfo` is the second struct this layer hands to C by value, and
/// like `CamelProvider` it is not a GObject — `camel_folder_info_get_type` is a
/// boxed type, so there is no instance size to compare against. What the
/// builder in `jmap-mail` relies on instead is the allocator's contract:
/// `camel_folder_info_new` hands back a struct with every field zeroed, so the
/// builder can fill in only the fields it has something to say about and trust
/// that `next`, `parent` and `child` are NULL rather than garbage. A
/// `g_slice_new` instead of a `g_slice_new0` upstream would turn that into a
/// chain that walks into freed memory the first time Camel follows it.
#[test]
fn a_fresh_folder_info_is_zeroed() {
    // SAFETY: no arguments; the returned struct is ours until we free it, and
    // `camel_folder_info_free` walks the (empty) chain and frees the names.
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

/// The names in a `CamelFolderInfo` are freed with `g_free`, so they have to be
/// allocated with `g_malloc` and not with a Rust allocator. Nothing in the type
/// system says so — both are `*mut gchar` — which makes this the assumption most
/// worth writing down: the round trip below is what a leak checker looks at, and
/// a `CString::into_raw` in its place is a heap corruption that usually survives
/// long enough to corrupt something else.
#[test]
fn folder_info_names_survive_a_g_strdup_and_a_free() {
    // SAFETY: `g_strdup` allocates with g_malloc, which is what
    // `camel_folder_info_free` releases the two name fields with.
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

/// A folder's *type* is a small integer packed into the flags word, not a bit
/// per type: `CAMEL_FOLDER_TYPE_MASK` isolates it and the ordinary bit flags
/// live outside the mask. Setting a type by OR-ing it in — which is what the
/// role mapping does — is only correct while those two facts hold, and both are
/// `#define`s in a header rather than anything the compiler checks.
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

    // ...and the flags a JMAP folder also carries do not land in the type
    // field, so OR-ing them together cannot change the type.
    let flags = CAMEL_FOLDER_SUBSCRIBED
        | CAMEL_FOLDER_SYSTEM
        | CAMEL_FOLDER_CHILDREN
        | CAMEL_FOLDER_NOCHILDREN;
    assert_eq!(flags & CAMEL_FOLDER_TYPE_MASK as CamelFolderInfoFlags, 0);
}

/// Why the provider has to declare a settings type of its own rather than
/// reuse `CamelOfflineSettings`, which is what `CamelOfflineStore` would
/// otherwise instantiate: none of Camel's stock settings classes implements
/// `CamelNetworkSettings`. Host, port, user and security method live on that
/// interface, and every provider in the tree — IMAPx, POP, SMTP — implements it
/// on its own settings subclass. A store whose settings do not is a store with
/// no server to talk to, and the symptom is a NULL host at connect time rather
/// than anything at build time.
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

        // ...and it is the *offline* one a `CamelOfflineStore` subclass would
        // inherit, which is the default the provider's settings type has to
        // replace and the parent it has to keep.
        assert_ne!(
            g_type_is_a(
                camel_offline_settings_get_type(),
                camel_store_settings_get_type()
            ),
            glib_sys::GFALSE
        );
    }
}

/// The three values `security-method` can take, and the only one that means
/// "no encryption". `STARTTLS_ON_STANDARD_PORT` is Camel's recommended value
/// and its name is about a protocol JMAP does not have — JMAP is HTTP, so both
/// non-`NONE` values mean the same thing here, TLS, and only `NONE` is a
/// decision to send credentials in the clear.
///
/// That `NONE` is *zero* is worth pinning because it is what a settings object
/// reads back as when nobody has set the property: an implementer that claims
/// `CamelNetworkSettings` without overriding its properties never receives the
/// interface's own default, and so silently starts out insecure. The default
/// that applies once the overrides are in place is pinned where the overrides
/// are, in `jmap-mail`'s `tests/settings.rs`.
#[test]
fn the_security_method_that_means_plaintext_is_the_zero_one() {
    assert_eq!(CAMEL_NETWORK_SECURITY_METHOD_NONE, 0);
    assert_ne!(CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT, 0);
    assert_ne!(CAMEL_NETWORK_SECURITY_METHOD_STARTTLS_ON_STANDARD_PORT, 0);
}

/// How wide a stored message id is, straight from the union Camel declares it
/// with. `CamelSummaryMessageID` overlays a `guint64` on eight bytes, and the
/// digest a provider computes is those eight bytes read back as the integer —
/// so the two views have to be the same size, and the size has to be eight.
/// `jmap-mail`'s `message_id_digest` takes exactly that many bytes off the
/// front of an MD5; a union that had grown would leave it filling half a field
/// and every message threading on a truncated id.
#[test]
fn a_stored_message_id_is_eight_bytes_of_a_digest() {
    let id = unsafe { std::mem::zeroed::<CamelSummaryMessageID>() };
    assert_eq!(size_of_val(&id), size_of::<u64>());
    // SAFETY: reading either arm of a fully-zeroed union is defined, and both
    // arms are plain data.
    unsafe {
        assert_eq!(size_of_val(id.id.hash.as_ref()), size_of::<u64>());
        assert_eq!(*id.id.id.as_ref(), 0);
    }
}

/// The other plain struct this provider hands across the boundary:
/// `CamelFolderChangeInfo`, which is what a folder tells Camel it has changed.
/// Like `CamelProvider` it sits behind a boxed type, so `g_type_query` knows
/// nothing of its size and `tests/layout.rs` has nothing to check — and unlike
/// `CamelProvider` it has both public fields *and* accessors for the same four
/// arrays, which is the layout check standing in for the one GObject would
/// have given us. A field at the wrong offset is an added uid read out of the
/// removed list.
///
/// The emptiness half is the contract the refresh vfunc is built on: a fresh
/// change info reports no changes, so "nothing came back different" and
/// "nothing to tell Camel about" are the same test — and a folder that emitted
/// its `changed` signal regardless would redraw the user's message list on
/// every poll.
#[test]
fn a_change_info_is_empty_until_something_is_put_in_it() {
    // SAFETY: the change info is allocated, filled and freed here; the uid is
    // a NUL-terminated literal the call copies.
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

/// The interface the store implements so a user can tick a folder off the
/// account: `CamelSubscribable`.
///
/// It is here rather than in `tests/layout.rs` for the same reason
/// `CamelProvider` is — `g_type_query` reports nothing about an interface, so
/// there is no size to compare `size_of` against. What is checkable is the
/// shape of the contract the provider signs, and each of the three assertions
/// below is a decision the store's implementation rests on.
#[test]
fn subscribing_is_an_interface_a_store_has_to_implement_itself() {
    // SAFETY: plain type-system reads; the default vtable ref is dropped
    // again, and the prerequisite array is g_malloc'd for the caller to free.
    unsafe {
        let subscribable = camel_subscribable_get_type();

        // An interface, so `tests/layout.rs` cannot cover it.
        assert_eq!(
            g_type_fundamental(subscribable),
            gobject_sys::G_TYPE_INTERFACE
        );
        let mut query = std::mem::zeroed::<GTypeQuery>();
        g_type_query(subscribable, &mut query);
        assert_eq!(query.type_, 0, "g_type_query knows an interface after all");

        // Only a store can implement it, which is why the provider's own
        // `CamelJmapStore` is the implementer and not some object beside it.
        let mut n = 0;
        let prerequisites = gobject_sys::g_type_interface_prerequisites(subscribable, &mut n);
        let required = std::slice::from_raw_parts(prerequisites, n as usize).to_vec();
        glib_sys::g_free(prerequisites.cast());
        assert!(
            required.contains(&camel_store_get_type()),
            "a CamelStore is not what CamelSubscribable requires"
        );

        // And Camel's own offline store — the class the provider derives from
        // — does not implement it, so everything the interface promises has to
        // come from the provider.
        assert_eq!(
            gobject_sys::g_type_is_a(camel_offline_store_get_type(), subscribable),
            glib_sys::GFALSE,
            "CamelOfflineStore implements CamelSubscribable already"
        );
    }
}

/// Camel installs no default behind the three methods, so an implementer that
/// leaves one NULL is not a store that answers conservatively — it is a call
/// through a NULL pointer from inside `camel_subscribable_folder_is_subscribed`.
/// That is what makes filling the vtable, rather than merely declaring the
/// interface, the whole of implementing it.
#[test]
fn the_interface_has_no_default_behind_any_of_its_three_methods() {
    // SAFETY: the default vtable is ref'd for the length of the reads and
    // unref'd again.
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

/// Whether `camel_provider_register` writes back into the struct it is given.
///
/// It matters because `jmap-mail`'s `provider::register` hands the registered
/// struct out as a `&'static CamelProvider` — a *shared* Rust reference to
/// memory Camel keeps a mutable pointer to for the life of the process. That is
/// only a claim Rust's model allows while nothing on the C side ever writes
/// there. Camel's documented in-place work is on `extra_conf`, which a JMAP
/// account leaves NULL, so the expectation is that the bytes come back
/// unchanged — and if a Camel release ever starts filling in `priv_` or
/// defaulting `url_hash`, this is where it surfaces rather than as a
/// mysteriously mutated `&'static`.
///
/// Originally `docs/AUDIT-FFI.md`'s F8, on the branch that check was written
/// on; the squash that brought F1–F4 to master left it behind, which is
/// `docs/AUDIT-FFI-20260810.md`'s F12.
#[test]
fn registering_a_provider_does_not_write_back_into_the_struct() {
    // SAFETY: as in the round-trip test above; `provider` outlives the call and
    // everything it points at is 'static.
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
        auto_detect: None,
        object_types: [store_type, G_TYPE_INVALID],
        authtypes: ptr::null_mut(),
        url_hash: None,
        url_equal: None,
        translation_domain: ptr::null(),
        priv_: ptr::null_mut(),
    };

    // SAFETY: reading the struct's own bytes, which are initialised above.
    let before = unsafe {
        std::slice::from_raw_parts(
            (&raw const provider).cast::<u8>(),
            std::mem::size_of::<CamelProvider>(),
        )
        .to_vec()
    };

    // SAFETY: the pointer stays valid for the rest of the process — the struct
    // is a `static`-lifetime local of a test that never returns it, and Camel
    // is not asked for it again after this test.
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

/// Probing `CamelInternetAddress` creation, multiple recipient formatting, index inspection,
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

        // Add 3 addresses (camel_internet_address_add returns 0-based insertion index)
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

/// Probing `CamelMimeMessage` parsing, header inspection (Subject, From, Message-ID, Date),
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

        // Inspect Message-ID (Camel strips surrounding angle brackets)
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

        // In-place modification of subject
        camel_mime_message_set_subject(msg, c"Revised Project Update".as_ptr());
        let updated_subj = camel_mime_message_get_subject(msg);
        assert_eq!(
            std::ffi::CStr::from_ptr(updated_subj).to_str().unwrap(),
            "Revised Project Update"
        );

        gobject_sys::g_object_unref(msg.cast());
    }
}

/// Probing `CamelNamedFlags` user labels / keywords and summary `bdata` encoding in EDS 3.52.
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

/// Probing `CamelDataCache` disk cache directory path, file naming, and removal in EDS 3.52.
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

/// Probing `CamelNameValueArray` construction, header insertion, named lookup, copying,
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

/// Probing `CamelMessageFlags` and `CamelFolderFlags` bitmask constants and bit independence in EDS 3.52.
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

/// Probing `CamelMIRecord` and `CamelFIRecord` summary database record structures in EDS 3.52.
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

/// Probing `CamelURL` construction, parameter setting/getting, cloning, equality, hash,
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

/// Probing `CamelFolderChangeInfo` multi-event batching (add, remove, change, recent),
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

/// Probing Camel error domain quarks (`camel_folder_error_quark`, `camel_service_error_quark`,
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

/// Probing `CamelMimePart` description, disposition, filename, content-ID, content-location,
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

        // Content-ID & Content-Location (Camel expects unbracketed ID and wraps it in Content-ID: <...>)
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

/// Probing `CamelMultipart` container creation, boundary management, part addition,
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

/// Probing `CamelTransferEncoding` constants and `CAMEL_MAX_PREVIEW_LENGTH` in EDS 3.52.
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

/// Probing `CamelContentType` creation, parameter setting, decoding, matching, and formatting in EDS 3.52.
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

/// Probing `CamelContentDisposition` decoding, formatting, and attachment checking in EDS 3.52.
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

/// Probing `CamelHeaderAddress` address list decoding, formatting, and encoding in EDS 3.52.
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

/// Probing Camel header date decoding, date formatting, and message ID utilities in EDS 3.52.
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

/// Probing `CamelMimeMessage` Message-ID, Reply-To, attachment detection, and part lookup by Content-ID in EDS 3.52.
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

/// Probing `CamelStreamMem` memory stream and `CamelStreamNull` null sink in EDS 3.52.
#[test]
fn camel_stream_mem_and_null_stream_operations_in_eds() {
    let test_data = b"Stream payload data for JMAP unit tests\r\n";

    unsafe {
        // 1. CamelStreamMem
        let mem_stream = camel_stream_mem_new();
        assert!(!mem_stream.is_null());
        // Fresh empty memory stream has 0 bytes, so EOS is TRUE
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

/// Probing `CamelStreamFs` filesystem stream operations in EDS 3.52.
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

/// Probing `CamelMimeFilterBasic` Base64 and Quoted-Printable filtering in EDS 3.52.
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

/// Probing `CamelMimeFilterCRLF` and `CamelMimeFilterLinewrap` in EDS 3.52.
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

/// Probing `CamelStreamFilter` stream pipeline with `CamelMimeFilterBasic` in EDS 3.52.
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

/// Probing `CamelMimeParser` streaming, step states, header extraction, and content-type parsing in EDS 3.52.
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

        // Inspect header (includes leading space after header colon)
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

/// Probing `CamelTrie` case-insensitive and case-sensitive multi-pattern search in EDS 3.52.
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

        // Advance past matched pattern and search next match (urgent -> 104)
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

/// Probing `CamelUIDCache` creation, caching, querying, and serialization in EDS 3.52.
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

        // Prepare query array containing both old and new UIDs
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

        assert_eq!(camel_uid_cache_save(cache), glib_sys::GTRUE);
        camel_uid_cache_destroy(cache);
        glib_sys::g_free(tmp_dir_raw.cast());
    }
}

/// Probing `CamelCharset` character set resolution, stepwise detection, and ISO mapping in EDS 3.52.
#[test]
fn camel_charset_detection_and_iso_to_windows_in_eds() {
    unsafe {
        // Pure ASCII text returns NULL from camel_charset_best (no non-ASCII encoding needed)
        let ascii_text = c"Simple ASCII content for testing";
        let best_ascii = camel_charset_best(ascii_text.as_ptr(), 32);
        assert!(best_ascii.is_null());

        // UTF-8 multi-byte sequence returns UTF-8 or compatible charset
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

/// Probing `CamelMimeFilterPreview`, `CamelMimeFilterCanon`, and `camel_text_to_html` in EDS 3.52.
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
