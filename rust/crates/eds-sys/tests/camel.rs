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
