// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Generates the EDS bindings at build time. The struct layouts differ between
// EDS releases, and a stale committed binding is a silently wrong ABI, not a
// build failure. tests/layout.rs compares the result with the GObject type
// system at run time.

use std::env;
use std::fs;
use std::path::PathBuf;

mod feature_marker;

/// The oldest supported EDS (Ubuntu 24.04, Fedora 40). The design rests on
/// `e_book_meta_backend_*`, which EDS has since 3.26.
const MIN_EDS: &str = "3.52";

/// All the types the backends use, and no other types. A bare `libebackend.h`
/// pulls in libsoup, libxml2 and json-glib: minutes of generation for a
/// binding surface that nobody reads.
const ALLOWED_TYPES: &[&str] = &[
    "EBackend.*",
    "E(Book|Cal)Backend.*",
    "E(Book|Cal)MetaBackend.*",
    "EData(Book|Cal).*",
    "E(Book|Cal)Cache.*",
    "ECache.*",
    "ESource.*",
    // Token authentication, in three shapes. `EOAuth2Service` is the interface
    // a service implements, and `EOAuth2ServiceInterface` is its vtable, which
    // the plugin fills. `EOAuth2Services` is the registry of known services.
    // `EOAuth2ServiceBase` is the `EExtension` that a service module subclasses
    // so that the registry finds it.
    //
    // The vtable uses `SoupMessage` as an opaque forward declaration. The
    // headers already reach libsoup through `libedataserver.h`, and
    // `libebackend-1.2` already adds `-I/usr/include/libsoup-3.0` and
    // `-lsoup-3.0`. Thus no new probe is necessary and no libsoup layout is
    // claimed.
    //
    // Exact names, not an `EOAuth2Service.*` prefix (see `CamelFolder` below).
    // The prefix also matches `EOAuth2ServiceGoogle`, `-Outlook` and `-Yahoo`,
    // the three built-in services of EDS. Their class structs would claim a
    // layout check for three types that nothing here subclasses. Their
    // `get_type` accessors come in through the function prefix, which claims
    // nothing: a `GType` is an integer.
    //
    // `g_type_query` gives no data about an interface, so tests/layout.rs
    // cannot check it. tests/oauth2.rs pins the vtable slot offsets: it
    // dispatches through each slot.
    "EOAuth2Service",
    "EOAuth2ServiceInterface",
    "EOAuth2ServiceFlags",
    "EOAuth2ServiceNavigationPolicy",
    "EOAuth2ServiceRefSourceFunc",
    "EOAuth2ServiceBase",
    "EOAuth2ServiceBaseClass",
    "EOAuth2Services",
    "EOAuth2ServicesClass",
    "ECollectionBackend.*",
    "EModule.*",
    "EExtension.*",
    "EContact.*",
    "EVCard.*",
    "EComponent.*",
    // A calendar object, in the two shapes that the `ECalMetaBackend` vfuncs
    // use: the libical-glib `ICalComponent` that `load_component_sync` returns,
    // and the libecal `ECalComponent` wrapper that `save_component_sync` gets
    // a list of. Both come in already as field and argument types. These names
    // also bring their class structs, which tests/layout.rs checks with
    // `g_type_query`.
    "ICal.*",
    "ECalComponent.*",
    // The zones that a calendar knows and no zone database knows. An
    // `ECalBackend` and its `ECalCache` implement this interface. EDS removes a
    // `VTIMEZONE` that another client wrote from the object and keeps it here,
    // not in the component. Thus the builtin table of libical does not find
    // such a zone. tests/layout.rs cannot check an interface, so tests/ical.rs
    // does (as tests/camel.rs does for `CamelSubscribable`).
    "ETimezoneCache.*",
    // The error domain of all EDS clients. Only the enum, not the full EClient
    // class: the backends make these codes, but they never use an EClient.
    "EClientError",
    // The address book part of it. `EBookMetaBackend` matches on
    // `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND`: a load that fails with it means
    // "remove this contact from the cache", not "the sync failed". Any other
    // report of a missing contact leaves a stuck cache entry.
    "EBookClientError",
    // The calendar part, for the same reason: a component that is gone reports
    // `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND`. The two domains are different
    // quarks, so the book code cannot replace it (tests/errors.rs pins this).
    "ECalClientError",
    // The mail provider. `CamelProvider` is the static struct that
    // `libcameljmap.so` registers on load. The other types are the objects it
    // names: an offline store (so the summary cache works without a
    // connection), a transport, their `CamelService` parent, the settings that
    // configure a service, and the session that owns it.
    "CamelProvider.*",
    "CamelService.*",
    "CamelStore.*",
    "CamelOfflineStore.*",
    // The interface a store implements so that the user can subscribe to a
    // folder. Camel gives no default for its three vfuncs (tests/camel.rs pins
    // both facts). It is the first interface whose vtable our type must fill.
    "CamelSubscribable.*",
    // The authentication mechanism of a provider. The class of the abstract
    // `CamelSasl` holds the `CamelServiceAuthType` that `camel_sasl_authtype`
    // uses as its table key. A bearer-token mechanism derives from
    // `CamelSaslXOAuth2`. The provider mechanism is under these two, so
    // `camel_sasl_is_xoauth2_alias` returns true for it.
    //
    // Exact names, not a `CamelSasl.*` prefix (see `EOAuth2Service` above). The
    // prefix also matches `CamelSaslAnonymous`, `-CramMd5`, `-DigestMd5`,
    // `-Gssapi`, `-Login`, `-Ntlm`, `-Plain`, `-Popb4smtp` and the three
    // built-in XOAUTH2 types. Their class structs would claim a layout check
    // for eleven types that nothing here subclasses.
    "CamelSasl",
    "CamelSaslClass",
    "CamelSaslXOAuth2",
    "CamelSaslXOAuth2Class",
    "CamelTransport.*",
    "CamelSession.*",
    "CamelSettings.*",
    "CamelNetworkSettings.*",
    // The settings of a JMAP service. The Camel classes stop at
    // `CamelOfflineSettings`, and none implements `CamelNetworkSettings`
    // (tests/camel.rs pins this). Thus the provider subclasses the first and
    // implements the second. This needs the parent structs and the enum of the
    // `security-method` property.
    "CamelOfflineSettings.*",
    "CamelNetworkSecurityMethod",
    "CamelURL",
    // The folder part of a store. `get_folder_info_sync` returns
    // `CamelFolderInfo`, a plain struct (not an object): the name brings its
    // flags enum. The provider folder derives from `CamelFolder` through
    // `CamelOfflineFolder`, which keeps the offline copy of the content.
    // `CamelFolderSummary` holds the rows of the folder: the message count, the
    // uid list and each `CamelMessageInfo` come from it. Exact names, because
    // a `CamelFolder.*` prefix also pulls in `CamelFolderSearch` and
    // `CamelFolderThread`: two more class structs that this layer would claim
    // to check.
    "CamelFolder",
    "CamelFolderClass",
    // How a folder tells Camel what changed: four uid lists, and the argument
    // of the `changed` signal on which Evolution redraws a message list. A
    // plain struct behind a boxed type, like `CamelProvider`, so
    // `g_type_query` knows nothing of it and tests/camel.rs checks it.
    "CamelFolderChangeInfo",
    // The result of `get_quota_info_sync`: a `name`/`used`/`total` node and a
    // `next` pointer to the next quota, all public fields with no accessors.
    // A boxed type like `CamelFolderChangeInfo`, so tests/layout.rs does not
    // check it.
    "CamelFolderQuotaInfo",
    // A third error domain, after `CamelServiceError` and `CamelStoreError`,
    // for failures of neither the account nor the store. For example,
    // `CAMEL_FOLDER_ERROR_INVALID_UID` reports a message that was in the last
    // listing and is now gone, without a report that the account is broken.
    "CamelFolderError",
    "CamelFolderInfo",
    "CamelFolderInfoFlags",
    "CamelFolderSummary",
    "CamelFolderSummaryClass",
    // The flags of the summary. `CAMEL_FOLDER_SUMMARY_DIRTY` tells
    // `camel_folder_summary_save` that the header and rows need a write.
    "CamelFolderSummaryFlags",
    // The flags of the folder object, different from `CamelFolderInfoFlags`
    // and with different bits. The info flags give the kind of folder. The
    // folder flags give how Camel treats it: for example, whether Camel
    // filters new mail in it, or whether it is the trash of the account. Name
    // both: only then are two such similar enums not mixed up.
    "CamelFolderFlags",
    "CamelOfflineFolder.*",
    // The contents of a folder, one row at a time. `CamelMessageInfo` holds one
    // summary row. Its subclass `CamelMessageInfoBase` stores the columns; a
    // summary with no message-info type of its own (as here) makes one.
    // `CamelMessageFlags` is the row flags word (a third flags enum with a third
    // set of bits). The `CamelSummaryMessageID` union gives the width of a
    // stored message id. The user flags and the headers come back as
    // `CamelNamedFlags` and `CamelNameValueArray`.
    "CamelMessageInfo.*",
    // One row as the summary database stores it: the struct with one column
    // per field that the Camel `load` and `save` vfuncs get. A message info
    // subclass keeps its own column in the `bdata` field. This provider keeps
    // the keywords of the last listing there.
    "CamelMIRecord",
    // The full folder as the same database stores it: the header row beside the
    // message rows, also with a `bdata` field. This provider keeps there the
    // `Email` state of its last listing. Thus a folder that opens again after a
    // restart can ask for a delta, not a full listing of the mailbox.
    "CamelFIRecord",
    "CamelMessageFlags",
    "CamelSummaryMessageID",
    "CamelNamedFlags",
    "CamelNameValueArray",
    // How an address list becomes the string that a summary row stores.
    // `CamelInternetAddress` is the RFC 5322 type. Its abstract parent
    // `CamelAddress` declares the format entry point.
    "CamelAddress.*",
    "CamelInternetAddress.*",
    // A message: what `get_message_sync` returns and the Evolution preview pane
    // shows. The four types are one chain: `CamelMimeMessage` is a
    // `CamelMimePart` is a `CamelMedium` is a `CamelDataWrapper`. The last
    // declares the parse entry point, so the provider crosses the ABI at each
    // level. Not `CamelMimeParser` and not the stream classes: a blob download
    // gives the bytes as one buffer, and `construct_from_data_sync` takes one.
    "CamelDataWrapper.*",
    "CamelMedium.*",
    "CamelMimePart.*",
    "CamelMultipart.*",
    "CamelMimeMessage.*",
    "CamelTransferEncoding",
    // The cache of fetched messages, so a second open is not a second download:
    // a directory of files with a path and a name as key, as the IMAPX message
    // cache. A `GObject`, so layout-checked, although nothing subclasses it:
    // the provider holds only one.
    "CamelDataCache.*",
    // Structured content types, content dispositions, and address headers
    "CamelContentType.*",
    "CamelContentDisposition.*",
    "CamelHeaderAddress.*",
    // Streams and MIME filter pipeline
    "CamelStream.*",
    "CamelMimeFilter.*",
    "CamelMimeFilterBasicType",
    "CamelMimeFilterCRLFDirection",
    "CamelMimeFilterCRLFMode",
    // MIME parser, trie, UID cache, and character set mapping
    "CamelMimeParser.*",
    "CamelMimeParserState",
    "CamelTrie.*",
    "CamelUIDCache.*",
    "CamelCharset.*",
    // Additional MIME filters and flags
    "CamelMimeFilterPreview.*",
    "CamelMimeFilterCanon.*",
    "CamelMimeFilterCanonFlags",
    "CamelMimeFilterToHTML.*",
    "CamelMimeFilterToHTMLFlags",
    // HTML parser, URL scanner, enriched, gzip, and windows filters
    "CamelHTMLParser.*",
    "_camel_html_parser_.*",
    "CamelHTMLParserState",
    "CamelUrlScanner.*",
    "CamelUrlPattern",
    "CamelUrlMatch",
    "CamelUrlScanFunc",
    "CamelMimeFilterHTML.*",
    "CamelMimeFilterEnriched.*",
    "CamelMimeFilterEnrichedFlags",
    "CamelMimeFilterGZip.*",
    "CamelMimeFilterGZipMode",
    "CamelMimeFilterWindows.*",
    // Bestenc, charset, From, yEnc and progress filters, stream buffer, S-expressions
    "CamelMimeFilterBestenc.*",
    "CamelBestencRequired",
    "CamelBestencEncoding",
    "CamelMimeFilterCharset.*",
    "CamelMimeFilterFrom.*",
    "CamelMimeFilterYenc.*",
    "CamelMimeFilterYencDirection",
    "CamelMimeFilterProgress.*",
    "CamelStreamBuffer.*",
    "CamelStreamBufferMode",
    "CamelSExp.*",
    "CamelSExpResult.*",
    "CamelSExpResultType",
    "CamelSExpTerm.*",
    "CamelSExpTermType",
    "CamelSExpSymbol",
    // Folder search, threads, memchunk, operation, NNTP address, local settings
    "CamelFolderSearch.*",
    "CamelFolderThread.*",
    "CamelMemChunk.*",
    "CamelOperation.*",
    "CamelNNTPAddress.*",
    "CamelLocalSettings.*",
    // Certificate database, junk filter, network service, memory pool, index, filter search
    "CamelCertDB.*",
    "CamelCert.*",
    "CamelCertTrust",
    "CamelJunkFilter.*",
    "CamelJunkStatus",
    "CamelNetworkService.*",
    "CamelMemPool.*",
    "CamelMemPoolFlags",
    "CamelIndex.*",
    "CamelIndexName.*",
    "CamelIndexCursor.*",
    "CamelTextIndex.*",
    "CamelFilterSearch.*",
];

const ALLOWED_FUNCTIONS: &[&str] = &[
    // EDS tells if the loaded library can serve code compiled for the build
    // version. Compatibility is the EDS rule, so this is more durable than a
    // version comparison here.
    "eds_check_version",
    "e_backend_.*",
    "e_(book|cal)_backend_.*",
    "e_(book|cal)_meta_backend_.*",
    "e_data_(book|cal)_.*",
    "e_(book|cal)_cache_.*",
    "e_cache_.*",
    "e_source_.*",
    // The token calls, in two prefixes: `e_oauth2_services_*` does not start
    // with `e_oauth2_service_`. The first brings the dispatch wrappers (which
    // tests/oauth2.rs calls to prove the vtable offsets), the `util_` helpers
    // that make the form bodies, and the four libsecret `*_sync` calls. The
    // second brings the service registry.
    "e_oauth2_service_.*",
    "e_oauth2_services_.*",
    // The EDS `g_object_bind_property`, with which each collection backend
    // makes a child connection follow its account. Not the gobject-sys call:
    // the EDS variant locks around making and removing the binding, and more
    // than one thread touches the sources of a registry.
    "e_binding_.*",
    // The registry side of a `.source` file. No backend calls these (a backend
    // gets a finished `ESource`). They make an `ESource` from a keyfile without
    // a running `evolution-source-registry`, so a test can check the manual
    // test recipe. The collection backend meets server-side sources for real.
    "e_server_side_source_.*",
    "e_collection_backend_.*",
    "e_contact_.*",
    "e_vcard_.*",
    // Make, read and render a calendar object. Not all of `i_cal_.*`:
    // `jmap-ical` does the property and value work in Rust on the text, so only
    // the component crosses the boundary.
    "i_cal_component_.*",
    // The zones that a calendar object refers to, which a saved object must
    // define: RFC 5545 §3.2.19 says a `TZID` parameter names a `VTIMEZONE` in
    // the same object. The libical identifier for a builtin zone is
    // `/freeassociation.sourceforge.net/Europe/Berlin`, which only libical can
    // resolve. `_from_tzid` resolves that form, `get_builtin_timezone` a plain
    // IANA name. `get_component` returns the `VTIMEZONE`, whose
    // `X-LIC-LOCATION` `jmap-ical` uses to translate the identifier. Exact
    // names: `i_cal_timezone_.*` adds about thirty functions (zone directory,
    // UTC offsets) that nothing here uses.
    "i_cal_timezone_get_builtin_timezone",
    "i_cal_timezone_get_builtin_timezone_from_tzid",
    "i_cal_timezone_get_component",
    // The zones that the calendar holds and the builtin table does not.
    // `e_timezone_cache_get_timezone` is the second place where a save finds
    // the definition of a `TZID`. The prefix is only four functions.
    // `add_timezone` lets a test add a zone as the EDS `add_timezone` D-Bus
    // method does.
    "e_timezone_cache_.*",
    // Read the `TZID` parameter of a property: only there does a component name
    // its zone. Set the `TZID` property of the copied `VTIMEZONE`, so the
    // definition has the identifier that the properties use, not the libical one.
    "i_cal_attach_.*",
    "i_cal_geo_.*",
    "i_cal_duration_.*",
    "i_cal_time_.*",
    "i_cal_recurrence_.*",
    "i_cal_recur_.*",
    "i_cal_property_.*",
    "i_cal_parameter_.*",
    "e_cal_component_.*",
    "e_client_error_.*",
    "e_(book|cal)_client_error_.*",
    // A backend gets its credentials as an ENamedParameters in connect_sync,
    // after EDS reads them from libsecret.
    "e_named_parameters_.*",
    // The rule that decides if a child source asks for its own password or
    // uses the one of its collection. EDS applies it in
    // `e_source_credentials_provider_ref_credentials_source`, and it compares
    // the `[Authentication] Host` of the two sources. Exact name: `e_util_.*`
    // is a hundred unrelated helpers. No backend calls it, but the account
    // setup must write a mail source that passes it. Without a test, two hosts
    // written in different places can silently differ.
    "e_util_can_use_collection_as_credential_source",
    // How to record a dismissed reminder, and how to free the component lists
    // that `modify_objects_sync` returns. `discard_alarm_sync` of the calendar
    // backend uses both, as the EDS CalDAV backend does. Exact names, for the
    // reason above.
    "e_cal_util_set_alarm_acknowledged",
    "e_util_free_nullable_object_slist",
    // Not an EDS symbol, but each loadable EDS module must export it. The
    // signature in scope lets the compiler check the module trampoline.
    "e_module_.*",
    // The base class of all extensions in this ecosystem: a collection backend
    // factory is one, and so is `EMailConfigServiceBackend`, which is in
    // Evolution and bound in `evo-sys`. A test there compares its parent with
    // the type accessor, which proves that the `EExtension` in the parent slot
    // is the one of this crate, not a second one generated from the same header.
    "e_extension_.*",
    // The other half: the interface of an extended object, and the call that
    // makes all extensions for it. `jmap-ui` has no extensible of its own, but
    // its tests use one to pin what `EExtensionClass::extensible_type` accepts.
    "e_extensible_.*",
    // The mail provider side. Camel declares `camel_provider_module_init` but
    // does not define it, as with `e_module_load`. The declaration lets the
    // compiler check the `extern "C"` definition of the module.
    "camel_provider_.*",
    "camel_service_.*",
    "camel_store_.*",
    "camel_offline_store_.*",
    // The interface above: its type accessor, the three wrappers that the
    // Evolution subscription editor calls, and the two that emit the signals
    // of a store when one of those answers changes.
    "camel_subscribable_.*",
    // The two type accessors that the provider mechanism registers under, and
    // the two lookups that are its purpose. `mail_ui_session_authenticate_sync`
    // asks `camel_sasl_authtype` to decide between one silent try and a
    // password prompt. The Evolution account editor uses
    // `camel_sasl_is_xoauth2_alias` to identify a bearer mechanism with its own
    // name. Exact names: `camel_sasl_.*` also brings `camel_sasl_new` and the
    // challenge and try-empty-password calls, for a mechanism instance that
    // this provider never makes.
    "camel_sasl_get_type",
    "camel_sasl_xoauth2_get_type",
    // The upstream XOAUTH2 mechanism with dynamic configuration (SASL name
    // "OAuth2Dynamic", e-d-s `bf6e074`): a built-in alternative to
    // `CamelSaslXOAuth2Jmap` where EDS has it. Only the type accessor: as with
    // `CamelSaslXOAuth2Google` and its two siblings, nothing here derives from it.
    "camel_sasl_xoauth2_dynamic_get_type",
    "camel_sasl_authtype",
    "camel_sasl_is_xoauth2_alias",
    // `camel_folder_info_new` and `_free` make and free the folder tree.
    // tests/layout.rs queries the type accessor.
    "camel_folder_info_.*",
    "camel_folder_get_type",
    // The quark of the domain above. Its siblings come in with
    // `camel_service_.*` and `camel_store_.*`. The folder prefix is not a
    // wildcard, so this one needs its name.
    "camel_folder_error_quark",
    // What a folder tells about itself: the path that Camel uses as key, the
    // name that the user sees, its store, and the flags that tell how Camel
    // treats it. Not all of `camel_folder_.*`: that matches each
    // `camel_folder_search_*` function and pulls in its class struct.
    "camel_folder_(get|set)_(full_name|display_name|flags)",
    "camel_folder_get_parent_store",
    // The summary of a folder. `take_folder_summary` puts one on a folder (it
    // takes the reference, it does not add one). `get_folder_summary` reads it.
    // Camel tests `has_summary_capability` before it asks for a message count.
    "camel_folder_(get|take)_folder_summary",
    "camel_folder_has_summary_capability",
    // `camel_folder_refresh_info_sync` wraps the vfunc that this provider
    // overrides, so a test can call it as Camel does. The base class answers
    // `get_message_count` and `get_uids` from the summary.
    // `camel_folder_changed` sends the changes of a refresh to an open window.
    "camel_folder_refresh_info_sync",
    // The wrapper of the vfunc that writes to the server. Evolution calls it
    // when it closes a folder or saves changes. A test uses it to put a flag
    // change on the server as Evolution does.
    "camel_folder_synchronize_sync",
    // The wrapper of the vfunc that removes the messages marked deleted:
    // Evolution "Expunge" and "Empty Trash" use it. A test calls it as
    // Evolution does: the wrapper locks the folder and skips a folder that has
    // the `CAMEL_FOLDER_HAS_BEEN_DELETED` flag.
    "camel_folder_expunge_sync",
    // The wrapper of the vfunc that moves or copies messages to another folder.
    // Before dispatch, this wrapper itself completes a transfer to the same
    // folder or of no messages, with no provider call. A test that skips it
    // tests a call that Evolution cannot make.
    "camel_folder_transfer_messages_to_sync",
    // The per-folder recursive lock. The refresh, synchronize and expunge
    // wrappers take it around dispatch; the transfer wrapper does not (from
    // camel-folder.c of EDS 3.52.3, not from the header). Thus the provider
    // `transfer_messages_to_sync` takes it, as each provider must that changes
    // a summary that a concurrent refresh, synchronize or expunge can change.
    "camel_folder_(lock|unlock)",
    // The wrapper of the vfunc that returns one message. A test calls it as
    // the Evolution preview pane does.
    "camel_folder_get_message_sync",
    // The wrapper of the vfunc that adds one message. The Camel transfer
    // between stores calls it on the destination folder, and so does "save to
    // Drafts" in the composer. A test calls it through the wrapper, which locks
    // the folder around the dispatch.
    "camel_folder_append_message_sync",
    "camel_folder_get_message_count",
    // Both names of the folder uid list: `compat::folder_dup_uids` resolves
    // which one exists. Up to 3.52, `get_uids` with `free_uids`; from 3.58,
    // only `dup_uids`. A name that this EDS does not declare costs nothing
    // (bindgen emits only what the headers have), so one allowlist serves both.
    "camel_folder_(get|free|dup)_uids",
    // The wrappers of the `cmp_uids` vfunc that this provider overrides and of
    // `sort_uids`, which calls it, so a test sorts uids as the Camel callers
    // do. Both are the same from 3.52 through master.
    "camel_folder_(cmp|sort)_uids",
    // The wrapper of the `get_permanent_flags` vfunc that this provider
    // overrides, so a test asks as the Camel callers do. The same from 3.52
    // through master.
    "camel_folder_get_permanent_flags",
    "camel_folder_changed",
    // The wrapper of the `get_quota_info_sync` vfunc that this provider
    // overrides, so a test calls it as the Evolution folder properties dialog
    // does. The vfunc makes one `camel_folder_quota_info_new` node per quota
    // and chains them through the public `next` field. A test frees the chain
    // with `_free`, as it frees a `GError`. Not `_clone`: nothing here needs a
    // second reference to a chain it owns.
    "camel_folder_get_quota_info_sync",
    "camel_folder_quota_info_(new|free)",
    "camel_folder_change_info_.*",
    "camel_folder_summary_.*",
    "camel_offline_folder_get_type",
    // Write and read one summary row. This prefix also matches
    // `camel_message_info_new_from_headers` from camel-folder-summary.h, and
    // that is necessary. A locally parsed message uses it, so it is the only
    // reference for the 64-bit digests that Camel uses for threads and that a
    // provider must calculate. Different digests split a conversation in two.
    "camel_message_info_.*",
    // The encoding of the one summary database field that Camel keeps for a
    // subclass column: `CamelMIRecord.bdata`, one string that the class chain
    // appends to on save and reads in the same order on load. These four
    // functions make sure that a keyword with a space or a digit comes back as
    // one keyword, not two.
    "camel_util_bdata_.*",
    // The set of user flags (Evolution labels) of a row. The provider replaces
    // the full set, not flag by flag: JMAP `keywords` is the full set of labels
    // of a message, and a missing keyword is one that the user removed elsewhere.
    "camel_named_flags_.*",
    "camel_name_value_array_.*",
    // Format an address list as a summary row stores it: make a
    // `CamelInternetAddress`, then call `camel_address_format` on it.
    "camel_address_.*",
    "camel_internet_address_.*",
    // Make the Camel message object from a download. `camel_mime_message_new`
    // allocates it and `camel_data_wrapper_construct_from_data_sync` parses the
    // bytes into it: one call, not streams, because a blob download gives the
    // full message as one buffer. A test asks the parsed message through the
    // accessors that the same prefixes bring. `camel_medium_.*` brings
    // `camel_medium_get_content`, which gets the body with no `CamelMimePart`
    // function. The part type accessor is here only for tests/layout.rs.
    "camel_data_wrapper_.*",
    // Keep the message, so the next open is not a download. `_new` also makes
    // the directory. `_add` and `_get` give a `GIOStream` on one entry.
    // `_remove` removes an entry that is not complete. A test uses
    // `_get_filename` to find where an entry is.
    "camel_data_cache_.*",
    "camel_medium_.*",
    "camel_mime_part_.*",
    "camel_multipart_.*",
    "camel_mime_message_.*",
    "camel_transport_.*",
    "camel_session_.*",
    "camel_settings_.*",
    "camel_network_settings_.*",
    "camel_offline_settings_.*",
    "camel_url_.*",
    "camel_content_type_.*",
    "camel_content_disposition_.*",
    "camel_header_address_.*",
    "camel_header_mailbox_decode",
    "camel_header_.*",
    "camel_transfer_encoding_.*",
    "camel_stream_.*",
    "camel_mime_filter_.*",
    "camel_mime_parser_.*",
    "camel_trie_.*",
    "camel_uid_cache_.*",
    "camel_charset_.*",
    "camel_text_to_html",
    "camel_html_parser_.*",
    "camel_url_scanner_.*",
    "camel_enriched_to_html",
    "camel_mime_filter_bestenc_.*",
    "camel_mime_filter_charset_.*",
    "camel_mime_filter_from_.*",
    "camel_mime_filter_yenc_.*",
    "camel_ydecode_step",
    "camel_yencode_step",
    "camel_yencode_close",
    "camel_mime_filter_progress_.*",
    "camel_stream_buffer_.*",
    "camel_utf8_.*",
    "camel_utf7_.*",
    "camel_ucs2_.*",
    "camel_strcase_.*",
    "camel_strstrcase",
    "camel_strdown",
    "camel_pstring_.*",
    "camel_string_is_all_ascii",
    "camel_hostname_utils_.*",
    "camel_sexp_.*",
    "camel_folder_search_.*",
    // The same helpers with their 3.58 names (`camel-search-utils.h`), after
    // 3.58 removed `camel-folder-search.h`. Both prefixes, as for the folder uid
    // list: bindgen emits only what the headers declare.
    "camel_search_util_.*",
    "camel_folder_thread_.*",
    "camel_folder_threaded_messages_dump",
    "camel_memchunk_.*",
    "camel_operation_.*",
    "camel_nntp_address_.*",
    "camel_local_settings_.*",
    "camel_file_util_.*",
    "camel_certdb_.*",
    "camel_cert_.*",
    "camel_junk_filter_.*",
    "camel_network_service_.*",
    "camel_mempool_.*",
    "camel_index_.*",
    "camel_text_index_.*",
    "camel_filter_search_.*",
];

/// The `ESource` extension names are `#define`d strings, not exported symbols.
/// A typo is not a link error but an address book that silently has no host,
/// so take them from the headers. `E_SOURCE_CREDENTIAL_*` are the same for the
/// `ENamedParameters` of a backend: a typo is a password that reads as absent.
/// `EDS_CAMEL_PROVIDER_DIR` is the `#define`d environment variable that points
/// the Camel provider loader at a build that is not installed (the manual test
/// recipe uses it). A typo is a provider directory that Camel silently ignores.
/// `CAMEL_FOLDER_TYPE_BIT` and `_MASK` are `#define`s for the shift and mask of
/// the folder type in the flags word. A typo gives a folder of a different type.
const ALLOWED_VARS: &[&str] = &[
    // The EDS version, in two forms that can differ: the `#define`s give the
    // header version that bindgen read, and the `extern const guint`s give the
    // version of the loaded `libedataserver`. The version matrix reads both
    // from a failing job. tests/layout.rs fails when they are not the same ABI
    // (see docs/eds-versions.md).
    "EDS_M(AJOR|INOR|ICRO)_VERSION",
    "eds_m(ajor|inor|icro)_version",
    "E_SOURCE_EXTENSION_.*",
    "E_SOURCE_CREDENTIAL_.*",
    // The three keys of a token in libsecret. A typo writes a refresh token
    // where nothing reads it, so the account asks for consent at each start.
    "E_OAUTH2_SECRET_.*",
    "EDS_CAMEL_PROVIDER_DIR",
    "CAMEL_FOLDER_TYPE_BIT",
    "CAMEL_FOLDER_TYPE_MASK",
    "CAMEL_MAX_PREVIEW_LENGTH",
    // The keys under which `initial_setup_sync` puts a role folder in the
    // `save_setup` hash table that `camel_store_initial_setup_sync` gives the
    // account wizard. A typo is a folder that the wizard silently does not fill.
    "CAMEL_STORE_SETUP_.*",
    "CAMEL_MIME_FILTER_LINEWRAP_.*",
    "CAMEL_MIME_FILTER_TOHTML_.*",
    "CAMEL_MIME_FILTER_CANON_.*",
    "CAMEL_MIME_FILTER_ENRICHED_.*",
    "CAMEL_HTML_PARSER_.*",
    "CAMEL_BESTENC_.*",
    "CAMEL_MIME_YDECODE_STATE_.*",
    "CAMEL_MIME_YENCODE_.*",
    "CAMEL_SEARCH_.*",
    "CAMEL_MEMPOOL_.*",
    "CAMEL_CERT_TRUST_.*",
    "CAMEL_JUNK_STATUS_.*",
];

/// `GType` and related types come from the gtk-rs sys crates, so eds-sys works
/// with the glib ecosystem and does not make its own incompatible `GObject`.
/// bindgen emits a bare name for a match, which the glob re-exports resolve.
///
/// Both forms are necessary. If only the `GObject` typedef is blocked, bindgen
/// emits the `_GObject` tag struct of the header: a second `GObject` in the
/// parent slot of each EDS instance struct, with the correct layout and the
/// wrong identity. With the tag also blocked, bindgen uses the gtk-rs typedef.
const BLOCKED_TYPES: &[&str] = &[
    "G[A-Z].*",
    "_G[A-Z].*",
    "g[a-z]+",
    "va_list",
    "__va_list_tag",
];

/// The parts of the EDS API that only some releases have, each with a name
/// whose presence in the generated bindings gives the answer:
/// `(cfg name, marker identifier)`.
///
/// The test reads the output of bindgen, not `EDS_HEADER_VERSION`. A version
/// comparison is a guess about which release changed what, and a wrong guess
/// is silent. Here clang reads the installed headers: if the symbol is in
/// `bindings.rs`, this EDS has it. Each marker is a full identifier found
/// nowhere else, so a longer name cannot cause a false substring match. A
/// struct field has no identifier of its own, so its marker is its declaration.
///
/// The version matrix keeps this list correct. `docs/eds-version-matrix.md`
/// tells what each entry is, and which ones the shipped code (not only a
/// probe test) depends on.
const EDS_FEATURES: &[(&str, &str)] = &[
    // 3.60 changed vCard versions: `EVCardFormat` and `EVC_FORMAT_VCARD_30` are
    // gone, `e_vcard_to_string` has no format argument, and the caller gives an
    // `EVCardVersion` to `e_vcard_convert_to_string`. `e_contact_date_to_string`
    // has a second parameter of the same enum, so one probe covers both.
    // `src/compat.rs` is the only place with either form.
    ("eds_vcard_version_enum", "EVCardVersion"),
    // 3.58 removed the `auto_detect` field of `CamelProvider`. The marker is
    // the function-pointer typedef of that field, which 3.58 also removed.
    ("camel_provider_auto_detect", "CamelProviderAutoDetectFunc"),
    // 3.60 removed the `url_hash` and `url_equal` fields of `CamelProvider`.
    // The marker is the field declaration, because no typedef goes with them
    // and `camel_url_hash` contains the bare name.
    ("camel_provider_url_helpers", "pub url_hash: "),
    // 3.58 removed `camel-folder-search.h`: the `CamelFolderSearch` object, its
    // class and the `camel_folder_search_util_*` helpers, for a
    // `CamelStoreSearch` API. Only `camel_folder_search_sync` and its two
    // siblings stay, on `CamelFolder`.
    ("camel_folder_search_object", "camel_folder_search_new"),
    // 3.58 added the `search_body_sync` and `search_header_sync` slots of
    // `CamelFolderClass`. A provider fills them to answer a body or header term
    // from the server, so `CamelStoreSearch` does not open each message. A
    // separate probe, not the negation of the one above: the two are
    // independent changes.
    (
        "camel_folder_search_body_sync",
        "camel_folder_search_body_sync",
    ),
    // 3.58 renamed the summary database row structs `CamelMIRecord` (a message)
    // and `CamelFIRecord` (the folder header). The provider keeps its column in
    // them.
    ("camel_summary_records", "CamelMIRecord"),
    // 3.58 replaced the `get_uids`/`free_uids` pair of `CamelFolder` (an array
    // lent by the folder and given back) with `dup_uids`, a reference-counted
    // `GPtrArray` that the caller owns. A separate probe from the records
    // above: the two are independent renames in the same release, and one
    // probe would be silently wrong for an EDS that changes only one.
    ("camel_folder_get_uids", "camel_folder_free_uids"),
    // 3.58 replaced `camel_uid_cache_get_new_uids`/`_free_uids` with
    // `camel_uid_cache_dup_new_uids`, which returns a reference-counted
    // `GPtrArray`.
    (
        "camel_uid_cache_get_new_uids",
        "camel_uid_cache_get_new_uids",
    ),
    // From 3.58, `CamelFolderThread` is a final `GObject`, not a boxed type.
    (
        "camel_folder_thread_boxed",
        "camel_folder_thread_messages_get_type",
    ),
    // 3.60 added `E_CONTACT_DEATHDATE`. The 3.52 `EContactField` enum does not
    // have it, so an unconditional reference does not compile there. Three
    // `EContact` behavior changes with no header signal arrive with it:
    // `X-JABBER`/`X-AIM`/`X-GADUGADU` map to the `_HOME_1` slot, not the
    // attribute-list field; `ANNIVERSARY` and `X-EVOLUTION-ANNIVERSARY` swap
    // which maps to `E_CONTACT_ANNIVERSARY`; and `E_CONTACT_NAME_OR_ORG`
    // derives differently with no explicit `FILE_AS`. All four are measured on
    // 3.52 and 3.60.2 in `docs/eds-version-matrix.md` (B).
    // `eds-sys/tests/contacts.rs` uses this one cfg for all four, because they
    // change together. The matrix exists to show a future EDS that separates them.
    ("eds_death_date_field", "E_CONTACT_DEATHDATE"),
    // eds#663 (MR 247, merged upstream 2026-09-29 as `592d95d`; `Since: 3.64`,
    // not released, only in master headers): `ESourceAuthentication` gets
    // `credential-store-id`. The credentials engine now uses it, not
    // `credential-name`, as the key of the stored OAuth 2.0 token. This key is
    // stable and unique per account, but `credential-name` comes from the host
    // and can be shared. Detected by its setter, not a version comparison. No
    // installed headers where this crate was built declare it, so this cfg has
    // never been set.
    (
        "eds_credential_store_id",
        "e_source_authentication_set_credential_store_id",
    ),
    // e-d-s `bf6e074` (merged upstream 2026-10-07, with `efe25dc` for
    // `e_util_fill_random_bytes`; `Since: 3.64`, not released, only in master
    // headers): `ESourceOAuth2Client` is the keyfile group ("OAuth2 Client")
    // where an account with the upstream dynamic OAuth 2.0 support (method
    // "OAuth2Dynamic") writes its client-id, client-secret,
    // authorization-endpoint, token-endpoint, redirect-uri, scope, resource and
    // issuer. This provider uses its own "JMAP OAuth2" group. Detected by the
    // type accessor, as `eds_credential_store_id` is. The `ESource.*` and
    // `e_source_.*` wildcards above already match both names, so bindgen emits
    // them when a header declares them.
    ("eds_oauth2_dynamic", "e_source_oauth2_client_get_type"),
    // 3.62 removed `camel_data_cache_add` for
    // `camel_data_cache_add_atomic`/`commit_atomic`/`discard_atomic`: a write
    // to a temporary name in the cache directory, renamed into place when it is
    // complete, not a stream on the final path. `jmap-mail/src/cache.rs` is the
    // only caller of either form.
    ("camel_data_cache_atomic", "camel_data_cache_add_atomic"),
];

/// Emit one `cargo::rustc-cfg` for each [`EDS_FEATURES`] entry that the
/// generated bindings contain, and a `rustc-check-cfg` for each entry, so that
/// `-D warnings` does not fail on the entries this EDS does not have.
///
/// The same answer also goes out as `cargo::metadata`, because a `rustc-cfg`
/// reaches only this crate: `#[cfg]` in a dependent crate is silently false on
/// each EDS. Cargo gives this metadata to the build scripts of direct
/// dependents as `DEP_EVOLUTION_DATA_SERVER_<KEY>` (the `links` key in
/// `Cargo.toml`, uppercase), and a dependent emits the cfgs it needs again
/// (see `jmap-mail/build.rs`). Thus the detection stays here, where clang
/// reads the installed headers, and no crate makes its own version test.
fn emit_feature_cfgs(bindings: &str) {
    for (cfg, marker) in EDS_FEATURES {
        println!("cargo::rustc-check-cfg=cfg({cfg})");
        if feature_marker::has_marker(bindings, marker) {
            println!("cargo::rustc-cfg={cfg}");
            println!("cargo::metadata={cfg}=1");
        }
    }
}

/// The one EDS version of the four probed packages.
///
/// The four `.pc` files come from one tarball, so a correct installation gives
/// one version. If they differ, the headers are of mixed ABIs, and the version
/// matrix must show this, not hide it. No binding is correct then, so emit none.
fn agreed(resolved: &[(&str, String)]) -> String {
    let (_, first) = resolved.first().expect("no packages were probed");
    if let Some((pkg, other)) = resolved.iter().find(|(_, v)| v != first) {
        let all = resolved
            .iter()
            .map(|(p, v)| format!("{p} {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        panic!(
            "the EDS development files disagree about their version \
             ({pkg} is {other}, not {first}) — these libraries ship as one \
             tarball, so a mixed installation would put headers of two ABIs \
             behind one set of bindings. Found: {all}"
        );
    }
    first.clone()
}

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    // The pkg_config crate emits the cargo:rustc-link-lib/-search lines; clang
    // needs only the include paths.
    let mut clang_args = vec!["-DE_CAL_DISABLE_DEPRECATED".to_string()];
    let mut resolved: Vec<(&str, String)> = Vec::new();
    for pkg in [
        "libebackend-1.2",
        "libedata-book-1.2",
        "libedata-cal-2.0",
        // Camel is in the same tarball with the same version, so the 3.52
        // minimum applies.
        "camel-1.2",
    ] {
        let lib = pkg_config::Config::new()
            .atleast_version(MIN_EDS)
            .probe(pkg)
            .unwrap_or_else(|e| panic!("{pkg} >= {MIN_EDS} not found: {e}"));
        clang_args.extend(
            lib.include_paths
                .iter()
                .map(|p| format!("-I{}", p.display())),
        );
        resolved.push((pkg, lib.version));
    }
    println!("cargo::rustc-env=EDS_HEADER_VERSION={}", agreed(&resolved));

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(clang_args)
        // The headers have many deprecations, and their warnings hide real
        // problems.
        .clang_arg("-Wno-deprecated-declarations")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Reproducible builds: no comment text (some clang versions put header
        // paths in it) and a stable item order.
        .generate_comments(false)
        .layout_tests(false)
        .derive_default(false)
        .prepend_enum_name(false)
        // Plain consts, not `ModuleConsts`: the blocked GLib enums are type
        // aliases in the gtk-rs sys crates, and with one module per enum bindgen
        // writes `GSomeFlags::Type`, which does not exist there.
        .default_enum_style(bindgen::EnumVariation::Consts)
        // String `#define`s become `&CStr`, so a `*const gchar` argument is
        // `.as_ptr()`, not a cast from a byte array with or without its NUL.
        .generate_cstr(true)
        // Edition 2024 denies unsafe operations in unsafe fns. Let bindgen
        // write the inner `unsafe` blocks; do not allow the lint everywhere.
        .wrap_unsafe_ops(true)
        .raw_line("pub use glib_sys::*;")
        .raw_line("pub use gobject_sys::*;")
        .raw_line("pub use gio_sys::*;")
        // glib-sys and gobject-sys both export these (same C symbols, two
        // declarations), so the globs above are ambiguous. An explicit
        // re-export has priority over a glob.
        .raw_line(
            "pub use glib_sys::{GIOCondition, g_io_condition_get_type, g_variant_get_gtype, \
             G_IO_ERR, G_IO_HUP, G_IO_IN, G_IO_NVAL, G_IO_OUT, G_IO_PRI};",
        );

    for t in ALLOWED_TYPES {
        builder = builder.allowlist_type(t);
    }
    for f in ALLOWED_FUNCTIONS {
        builder = builder.allowlist_function(f);
    }
    for v in ALLOWED_VARS {
        builder = builder.allowlist_var(v);
    }
    for t in BLOCKED_TYPES {
        builder = builder.blocklist_type(t);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed on the EDS headers");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR unset"));
    let written = out.join("bindings.rs");
    bindings
        .write_to_file(&written)
        .expect("could not write bindings.rs");

    // Detect features in the file that rustc compiles, not from the version
    // string, so the answer is what this EDS has.
    emit_feature_cfgs(&fs::read_to_string(&written).expect("could not read back bindings.rs"));
}
