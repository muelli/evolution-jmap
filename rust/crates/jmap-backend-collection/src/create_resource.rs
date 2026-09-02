// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creating an address book or a calendar *on the server* — the half of
//! evolution-ews parity that a backend which only mirrors what it finds cannot
//! do.
//!
//! Everything else in this crate reads the server and writes Evolution. This is
//! the one request that goes the other way: Evolution's "New Address Book" and
//! "New Calendar" dialogs hand the registry a scratch `ESource` describing a
//! collection that does not exist yet, and the collection backend is asked to
//! make it exist. `ECollectionBackendClass::create_resource_sync` is that
//! request, and [`crate::backend`] is where the slot is filled.
//!
//! What is here is the EDS ends of it. The decision in the middle — which JMAP
//! account, which `/set` call, and what [`Child`] the created collection becomes
//! — is [`jmap_collection_sync::create`]'s, where it needs no headers and is
//! tested against a running `jmap-mockd`.
//!
//! ## The two obligations, taken from EDS's own documentation of the vfunc
//!
//! `e_collection_backend_create_resource_sync()` states them, and neither is
//! visible in the header:
//!
//! - *"It is the implementor's responsibility to examine @source and determine
//!   what the equivalent server-side resource would be. If this cannot be
//!   determined without ambiguity, the function must return an error."* — hence
//!   [`requested_of`], and hence its `None` for a scratch source that names no
//!   kind.
//! - *"After the server-side resource is successfully created, the implementor
//!   must also add an #ESource to @backend's #ECollectionBackend:server."* —
//!   hence [`adopt_created`] followed by the publish. Creating the collection on
//!   the server and stopping there would leave Evolution with a dialog that
//!   closed and no address book: the child would appear only after the next
//!   populate, so the create would look as if it had silently failed.
//!
//! ## The parent's implementation is not chained up to
//!
//! The other way round from [`crate::child_added`], and for a reason that is in
//! EDS's source rather than its documentation:
//! `collection_backend_create_resource()` — which the default
//! `create_resource_sync` drives through a closure — does exactly one thing,
//! `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
//! creating remote resources")`. So the parent *is* the refusal this override
//! exists to replace, and chaining up would turn a create that worked into a
//! create that reported failure.
//!
//! ## The scratch source is EDS's, and this backend finishes it
//!
//! `server_side_source_remote_create_cb()` builds the scratch source with
//! `e_server_side_source_new_user_file()` — the registry's own source directory,
//! not this collection's cache — sets the keyfile Evolution sent onto it, and
//! deliberately does **not** add it to the registry: the comment there says it is
//! "up to the ECollectionBackend whether to use source as given or create its own
//! equivalent". This backend uses it as given, which is what evolution-ews does,
//! and finishes it into exactly the child a populate would have written:
//!
//! - every setting of the [`Child`] the created collection became, through the
//!   same [`crate::child_source::apply`] a fan-out uses — so a created child and
//!   a discovered one differ in no field, least of all in `[Resource] Identity`,
//!   whose absence would have EDS delete its cache file;
//! - `parent`, `writable` and `write_directory`, which are precisely what EDS's
//!   own `collection_backend_new_source()` sets on a child it mints and which the
//!   scratch source therefore lacks. `removable` is the fourth thing that
//!   function sets and is deliberately **not** set here: EDS's
//!   `collection_backend_child_added()` sets it `FALSE` for every child and
//!   [`crate::child_added`] chains up to that, so the child gets it on publish.
//!
//! ## What is deliberately not set here: `remote-deletable`
//!
//! evolution-ews sets it at each of the three sites that mint a child, this one
//! included. This backend sets it in one place instead —
//! [`crate::delete_resource::offer_deletion`], called from `child_added`, which
//! is EDS's own funnel for every child of a collection and therefore covers a
//! created child on its publish as well as a discovered or cached one. So its
//! absence here is where the flag is written, not whether. Unlike a
//! discovered child, a created one has nothing to correct that default
//! afterwards — [`crate::fan_out::adopt`]'s per-resource `myRights.mayDelete`
//! answer is a fan-out's, and a create never runs one — so a freshly created
//! collection stays deletable until the next populate reads its rights.
//!
//! ## The credentials are looked up, not remembered
//!
//! evolution-ews keeps the `ENamedParameters` its `authenticate_sync` was handed
//! in the backend instance and rebuilds a connection from them whenever it needs
//! one. This backend does not, because it does not have to:
//! `e_source_registry_server_ref_credentials_provider()` reaches the very store
//! `authenticate_sync`'s credentials came out of, and [`stored_password_of`] asks
//! it at the moment the password is needed. So no secret is held for the life of
//! the account, there is no instance state to initialise and finalize, and a
//! create works in a process where no `authenticate_sync` has run for this
//! account yet. An OAuth 2.0 account goes on through
//! [`jmap_backend_core::oauth2::access_token`] —
//! [`crate::authenticate::login_of`] is what decides which — so its token is
//! always obtained fresh rather than being a remembered one that has since
//! expired.
//!
//! [`Child`]: jmap_collection_sync::Child

use std::fmt;
use std::ptr;

use eds_sys::{
    ENamedParameters, ESource, ESourceCredentialsProvider, ESourceRegistryServer,
    e_named_parameters_free, e_server_side_source_set_writable,
    e_server_side_source_set_write_directory, e_source_credentials_provider_lookup_sync,
    e_source_get_display_name, e_source_get_uid, e_source_registry_server_ref_credentials_provider,
    e_source_set_parent,
};
use gio_sys::{
    G_IO_ERROR_CANCELLED, G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED, GCancellable,
    g_io_error_quark,
};
use glib_sys::{GError, GFALSE, GTRUE, g_error_free};
use jmap_backend_core::connect::Collection;
use jmap_backend_core::error::{cstring_lossy, invalid_arg_gerror, to_gerror};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{password as password_of, read_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::source::{self, ConnectTarget};
use jmap_backend_core::trampoline::{log_critical, log_critical_for_account};
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, CreateFailure, Requested, create_collection};

use crate::child_source::{UnwritableSetting, apply};
use crate::resource_id::kind_of;

/// Why a create could not be done.
#[derive(Debug)]
pub enum CreateError {
    /// The scratch source names neither an address book nor a calendar, which is
    /// the ambiguity EDS's own documentation of the vfunc says must be an error.
    UnknownKind,
    /// The account could not be turned into a login — a server it may not
    /// contact, or no credentials for it.
    Login(crate::authenticate::LoginError),
    /// The connection or the create itself failed, or the account's server holds
    /// no place to create this kind of collection.
    Server(CreateFailure),
    /// The collection was created and a setting could not be written onto the
    /// source for it. Reported rather than swallowed, and the reason the create
    /// still counts as failed — see [`adopt_created`]. The kind is carried so
    /// the message can name what was made and is now un-described.
    Unwritable(ChildKind, UnwritableSetting),
}

impl From<crate::authenticate::LoginError> for CreateError {
    fn from(failure: crate::authenticate::LoginError) -> Self {
        Self::Login(failure)
    }
}

impl From<CreateFailure> for CreateError {
    fn from(failure: CreateFailure) -> Self {
        Self::Server(failure)
    }
}

impl From<jmap_client::Error> for CreateError {
    fn from(error: jmap_client::Error) -> Self {
        Self::Server(CreateFailure::Client(error))
    }
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKind => f.write_str(&translate(
                // TRANSLATORS: shown when Evolution asks a JMAP account to
                // create a collection whose kind the request does not state.
                c"the new collection is neither an address book nor a calendar",
            )),
            Self::Login(failure) => failure.fmt(f),
            // `Unserved` is `jmap-collection-sync`'s, and that crate builds
            // without gettext, so its own `Display` is the developer-facing
            // spelling and the user-facing one is written here — with the same
            // translated nouns every other account error uses, rather than a
            // second pair of msgids saying "address book" and "calendar".
            Self::Server(CreateFailure::Unserved(kind)) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar". Shown when
                // the JMAP server behind an account offers no account of that
                // kind at all, so there is nowhere to create one.
                c"this account's JMAP server has no place to create %1$s in",
                &[collection_of(*kind).noun().as_str()],
            )),
            Self::Server(failure) => failure.fmt(f),
            Self::Unwritable(kind, setting) => f.write_str(&translate_with(
                // TRANSLATORS: %1$s is "address book" or "calendar"; %2$s is a
                // developer-facing description of the setting that could not be
                // written. Shown when the collection was made on the server and
                // the local source describing it could not be completed.
                c"the %1$s was created on the server, but the source for it could not be written: %2$s",
                &[collection_of(*kind).noun().as_str(), &setting.to_string()],
            )),
        }
    }
}

/// The [`Collection`] a [`ChildKind`] is, so the translated noun is the one
/// [`Collection::noun`] already carries rather than a second pair of msgids.
///
/// Shared with [`crate::delete_resource`], which names the same two things in
/// its own messages.
pub(crate) fn collection_of(kind: ChildKind) -> Collection {
    match kind {
        ChildKind::AddressBook => Collection::AddressBook,
        ChildKind::Calendar => Collection::Calendar,
    }
}

impl std::error::Error for CreateError {}

impl CreateError {
    /// The `GError` the vfunc reports this through.
    ///
    /// A `jmap_client::Error` goes through
    /// [`jmap_backend_core::error::to_gerror`], the same mapping every other
    /// operation in these backends uses, so a 401 on a create becomes the
    /// Evolution error code a 401 always becomes. Everything else is
    /// `E_CLIENT_ERROR_INVALID_ARG`: each is either something EDS handed us that
    /// cannot be acted on or a fault in this backend, and neither is a reason to
    /// report the account itself as broken.
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Login(failure) => failure.to_gerror(),
            Self::Server(CreateFailure::Client(error)) => to_gerror(error),
            other => invalid_arg_gerror(&other.to_string()),
        }
    }
}

/// What the scratch source `create_resource_sync` was handed asks for, or `None`
/// if it does not say.
///
/// `None` is EDS's documented "cannot be determined without ambiguity": a source
/// with neither the `[Address Book]` nor the `[Calendar]` extension is one this
/// backend has no server-side object for, and guessing a kind would create the
/// wrong sort of collection under a name the user chose for the other.
///
/// Reached through [`kind_of`], which tests for the extensions rather than
/// fetching them — a scratch source is a real `ESource` that gets written to
/// disk, and `e_source_get_extension` would add the group it was asked about.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` that outlives the call. It is only
/// read from.
pub unsafe fn requested_of(source: *mut ESource) -> Option<Requested> {
    // SAFETY: the caller's contract is `kind_of`'s.
    let kind = unsafe { kind_of(source) }?;
    // SAFETY: a valid source, since `kind_of` answered `Some` for it; the getter
    // returns NULL or a string the source owns.
    let display_name = unsafe { read_string(e_source_get_display_name(source)) };
    Some(Requested {
        kind,
        display_name: display_name.unwrap_or_default(),
    })
}

/// Connects as `target`/`credentials` say and creates `requested` there.
///
/// One call so that the vfunc has one thing to do with a
/// [`Login`](crate::authenticate::Login) and one error type to report. Both
/// halves are somebody else's decision: the connect is
/// [`jmap_backend_core::source::connect`]'s — so a bare-domain account gets the
/// same `_jmap._tcp` autodiscovery a fan-out gets — and the create is
/// [`create_collection`]'s.
pub fn create_on_server(
    target: &ConnectTarget,
    rebase_urls: bool,
    credentials: Credentials,
    requested: &Requested,
) -> Result<Child, CreateError> {
    let client = source::connect(target, rebase_urls, credentials)?;
    let child = create_collection(&client, requested)?;
    tracing::debug!(
        account_id = child.account_id.as_str(),
        kind = ?child.kind,
        display_name = child.display_name.as_str(),
        resource_id = child.resource_id.as_str(),
        collection_id = child.collection_id.as_str(),
        "created collection resource on server"
    );
    Ok(child)
}

/// Finishes the scratch source into the child of this collection that `child`
/// describes.
///
/// Everything but the publish, which needs the registry server and so stays in
/// [`crate::backend`]. In order:
///
/// 1. every [`Setting`](jmap_collection_sync::Setting) of the child, through the
///    same [`apply`] a fan-out writes a discovered child with. An error here
///    fails the whole create even though the collection is already on the
///    server, and that is the honest answer rather than the tidy one: the
///    alternative is exporting a half-written child, which is exactly what
///    [`crate::child_source`] exists to prevent. The collection is left behind on
///    the server, where the next populate finds it and writes a child for it
///    properly — a leftover the user can see beats a source that looks right and
///    reaches no server.
/// 2. `parent`, so the source is a child of this account rather than a top-level
///    one — and so `child_added` fires for it when it is published;
/// 3. `write_directory`, so the source's `.source` file is written into this
///    collection's cache directory, where a fan-out-created child's lives and
///    where removing the account takes it along, rather than staying in the
///    registry's user directory where EDS minted it;
/// 4. `writable`, which is what lets the user rename the address book afterwards.
///
/// Steps 2–4 are `collection_backend_new_source()`'s own, minus the `removable =
/// FALSE` that `collection_backend_child_added()` applies to every child on
/// publish; see the module comment.
///
/// `cache_dir` is `None` only for a collection backend EDS gave no cache
/// directory, which should not happen — it derives one from the account uid — and
/// is treated as "do not redirect the writes" rather than as an error: the source
/// still works, it is merely written where EDS put it.
///
/// # Safety
///
/// `scratch` must be a valid `EServerSideSource` — the source EDS handed
/// `create_resource_sync`, which is one by construction
/// (`server_side_source_remote_create_cb` builds it with
/// `e_server_side_source_new`). No reference is taken and nothing outlives the
/// call.
pub unsafe fn adopt_created(
    scratch: *mut ESource,
    child: &Child,
    connection: &Connection,
    account_uid: &str,
    cache_dir: Option<&str>,
) -> Result<(), UnwritableSetting> {
    tracing::debug!(
        account_uid,
        resource_id = child.resource_id.as_str(),
        kind = ?child.kind,
        "adopting created collection child source"
    );

    // SAFETY: a valid source by this function's contract.
    unsafe { apply(scratch, &child.settings(connection)) }?;

    let account_uid = cstring_lossy(account_uid);
    // SAFETY: as above, and a NUL-terminated string the setter copies.
    unsafe { e_source_set_parent(scratch, account_uid.as_ptr()) };

    if let Some(cache_dir) = cache_dir {
        let cache_dir = cstring_lossy(cache_dir);
        // SAFETY: as above; the source is an `EServerSideSource` by this
        // function's contract, and the setter copies the string.
        unsafe { e_server_side_source_set_write_directory(scratch.cast(), cache_dir.as_ptr()) };
    }

    // SAFETY: as above.
    unsafe { e_server_side_source_set_writable(scratch.cast(), GTRUE) };

    Ok(())
}

/// The name of the kind a create was refused for, for a log line.
///
/// Not user-facing — [`CreateError`] is what the user is shown — but a refused
/// create is worth a line on EDS's own channel, and "address book"/"calendar"
/// reads better in one than a `Debug` of the enum.
pub fn kind_noun(kind: ChildKind) -> &'static str {
    match kind {
        ChildKind::AddressBook => "address book",
        ChildKind::Calendar => "calendar",
    }
}

/// The password EDS has stored for `source`, asked of the registry's own
/// credentials provider.
///
/// The same store `authenticate_sync`'s `ENamedParameters` comes out of, reached
/// the one way a backend can reach it without being handed one: the provider
/// behind `e_source_registry_server_ref_credentials_provider()`. That provider
/// resolves the *credentials source* itself — a child that shares its
/// collection's password gets the collection's — so passing the account source is
/// asking exactly the question `authenticate_sync` was answered.
///
/// `None` is "there is no password for this account", which for an account that
/// names a user becomes [`ConnectError::CredentialsRequired`] and so a prompt. A
/// lookup that *failed* answers `None` too, and says why in a log line rather
/// than in the returned value: from the caller's side the two are the same
/// situation, this operation has no password, and the actionable message for
/// the user is the prompt either way.
///
/// **Which channel that line goes to is [`classify_lookup_failure`]'s**, and it
/// is not a detail. Most of that call's failures are not failures of anything:
/// an OAuth 2.0 account answers `G_IO_ERROR_NOT_SUPPORTED` on the path that then
/// works perfectly, and a keyring with nothing in it yet answers
/// `G_IO_ERROR_NOT_FOUND` on the path that becomes a prompt. Only a lookup that
/// failed for some other reason (no credentials provider at all, libsecret
/// unreachable) is the system fault GLib's critical channel exists for.
///
/// `context` names the vfunc for the critical channel — the same lookup serves
/// [`crate::delete_resource`]'s vfunc, and a log line that named the wrong one
/// would send a reader to the wrong module. Every failure here also carries
/// `source`'s own uid as a structured `account_id` field (there is no single
/// resource to blame — a credentials lookup runs on the account as a whole),
/// via [`log_critical_for_account`].
///
/// [`ConnectError::CredentialsRequired`]: jmap_backend_core::connect::ConnectError::CredentialsRequired
///
/// # Safety
///
/// `server` must be NULL or a valid `ESourceRegistryServer`, `source` a valid
/// `ESource`, and `cancellable` NULL or a valid `GCancellable`.
pub unsafe fn stored_password_of(
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
    cancellable: *mut GCancellable,
    context: &str,
) -> Option<String> {
    // SAFETY: a valid source by this function's own contract; the uid comes
    // back `(transfer none)`, borrowed for the length of this call.
    let account_id = unsafe { read_string(e_source_get_uid(source)) };
    let has_server = !server.is_null();
    tracing::debug!(
        account_id = account_id.as_deref(),
        has_server,
        context,
        "querying stored password for collection resource"
    );

    if server.is_null() {
        // The registry server is a weak reference on the backend, so NULL means
        // it is gone — during shutdown, say — and then there is nobody to ask.
        report_critical(
            account_id.as_deref(),
            format!("{context}: the registry server is gone; no credentials to look up"),
        );
        return None;
    }

    // SAFETY: a valid registry server; the provider comes back `(transfer
    // full)`.
    let provider = unsafe {
        Owned::<ESourceCredentialsProvider>::from_raw(
            e_source_registry_server_ref_credentials_provider(server),
        )
    };
    let Some(provider) = provider else {
        report_critical(
            account_id.as_deref(),
            format!("{context}: the registry server has no credentials provider"),
        );
        return None;
    };

    // SAFETY: a live provider, a valid source and a cancellable satisfying this
    // function's contract.
    unsafe {
        lookup_password(
            provider.as_ptr(),
            source,
            cancellable,
            context,
            account_id.as_deref(),
        )
    }
}

/// [`log_critical_for_account`] when `account_id` is known, else the plain
/// [`log_critical`] — the account's own uid is not always readable (an
/// `ESource` with none would be a first for EDS, but it is not this
/// function's contract to assume it).
fn report_critical(account_id: Option<&str>, message: String) {
    match account_id {
        Some(account_id) => log_critical_for_account(account_id, &message),
        None => log_critical(&message),
    }
}

/// One `e_source_credentials_provider_lookup_sync`, with both things it hands
/// back released.
///
/// # Safety
///
/// `provider` must be a live `ESourceCredentialsProvider`, `source` a valid
/// `ESource`, and `cancellable` NULL or a valid `GCancellable`.
unsafe fn lookup_password(
    provider: *mut ESourceCredentialsProvider,
    source: *mut ESource,
    cancellable: *mut GCancellable,
    context: &str,
    account_id: Option<&str>,
) -> Option<String> {
    let mut credentials: *mut ENamedParameters = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: the contract above, and two writable out-parameters initialised to
    // NULL. The credentials come back `(transfer full)` and the `GError`
    // likewise.
    let looked_up = unsafe {
        e_source_credentials_provider_lookup_sync(
            provider,
            source,
            cancellable,
            &mut credentials,
            &mut error,
        )
    };

    if looked_up == GFALSE {
        // Classified before the message is taken, since taking it frees the
        // `GError` this reads the domain and code off.
        // SAFETY: the call failed, so `error` is NULL or a live `GError`.
        let failure = unsafe { classify_lookup_failure(error) };
        // SAFETY: the call failed, so `error` is NULL or a `GError` ownership of
        // which passed to us; its message is a string the struct owns.
        let message = unsafe { take_message(error) };
        match failure {
            // Not a fault, and the caller's `None` already says everything the
            // user needs: a prompt, or a token fetch that does not want a
            // password in the first place.
            LookupFailure::NoPassword => tracing::debug!(
                account_id,
                context,
                message,
                "EDS has no stored password for this account"
            ),
            LookupFailure::Fault => report_critical(
                account_id,
                format!("{context}: the account's credentials could not be looked up: {message}"),
            ),
        }
        return None;
    }

    // A success that also set an error would be a broken callee, but freeing it
    // costs nothing and leaking it costs a report — the same reasoning as
    // `crate::removal`'s.
    if !error.is_null() {
        // SAFETY: a `GError` this call owns and nothing else holds.
        unsafe { g_error_free(error) };
    }

    // SAFETY: the lookup succeeded, so `credentials` is NULL or an
    // `ENamedParameters` this call owns; `password_of` only reads through it.
    let password = unsafe { password_of(credentials) };
    if !credentials.is_null() {
        // SAFETY: ownership passed to us with the out-parameter, and the borrow
        // `password_of` took has ended — it answers an owned `String`.
        unsafe { e_named_parameters_free(credentials) };
    }

    password
}

/// What a `FALSE` from `e_source_credentials_provider_lookup_sync` means.
///
/// The distinction exists because a great many of that call's failures are not
/// failures of anything: see [`classify_lookup_failure`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LookupFailure {
    /// EDS has no password for this account, and that is an ordinary state
    /// rather than a fault. The caller's answer is `None` either way; what
    /// changes is which log channel says so.
    NoPassword,
    /// Something that should not have failed did, and GLib's critical channel
    /// is where a system fault belongs.
    Fault,
}

/// Which of the two a failed lookup is, by domain and code rather than by
/// message text.
///
/// Three `G_IO_ERROR` codes are ordinary, each for its own reason found in
/// EDS's source:
///
/// - `G_IO_ERROR_NOT_FOUND` is what
///   `e_source_credentials_provider_impl_password_lookup_sync` sets, verbatim,
///   when the keyring holds nothing for the source yet. That is the state
///   [`stored_password_of`] documents as becoming a credentials prompt.
/// - `G_IO_ERROR_NOT_SUPPORTED` is the abstract
///   `ESourceCredentialsProviderImpl` default, and it is what an **OAuth 2.0**
///   account answers: `e_source_credentials_provider_impl_oauth2` matches the
///   source through `can_process` and then overrides `can_store` and
///   `can_prompt` only, leaving `lookup_sync` at the base class's refusal. An
///   OAuth 2.0 account has no password by construction, and
///   [`crate::authenticate::login_of`] goes on to fetch a token and succeed, so
///   this arrives on the *success* path of every create and every delete such
///   an account performs.
/// - `G_IO_ERROR_CANCELLED` is the user pressing Stop, which reaches libsecret
///   because the vfunc's own `GCancellable` is passed straight through.
///
/// Everything else keeps its critical, including a `FALSE` carrying no `GError`
/// at all. That shape is what the provider's own `g_return_val_if_fail`
/// produces when it has no implementation to ask, not even the password
/// fallback.
///
/// # Safety
///
/// `error` must be NULL or a valid `GError` that outlives the call.
unsafe fn classify_lookup_failure(error: *const GError) -> LookupFailure {
    if error.is_null() {
        return LookupFailure::Fault;
    }

    // SAFETY: a live `GError` by the contract above; `g_io_error_quark` takes
    // no arguments.
    let (domain, code) = unsafe { ((*error).domain, (*error).code) };
    // SAFETY: as above.
    if domain != unsafe { g_io_error_quark() } {
        return LookupFailure::Fault;
    }

    match code {
        G_IO_ERROR_NOT_FOUND | G_IO_ERROR_NOT_SUPPORTED | G_IO_ERROR_CANCELLED => {
            LookupFailure::NoPassword
        }
        _ => LookupFailure::Fault,
    }
}

/// The message of a `GError` this call owns, and then frees.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        return "EDS gave no reason".to_owned();
    }

    // SAFETY: a live GError; its message is a NUL-terminated string it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: ownership passed to us with the out-parameter.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "EDS gave no message".to_owned())
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use gio_sys::{G_IO_ERROR_FAILED, g_dbus_error_quark};

    use super::*;

    /// A real `GError` rather than a hand-rolled struct, since
    /// [`classify_lookup_failure`] reads its `domain` and `code` fields
    /// directly. The same helper `jmap_backend_core::oauth2`'s tests use.
    fn error(domain: glib_sys::GQuark, code: i32) -> *mut GError {
        let message = CString::new("boom").unwrap();
        // SAFETY: a valid domain and a NUL-terminated message; every caller
        // below frees the result.
        unsafe { glib_sys::g_error_new_literal(domain, code, message.as_ptr()) }
    }

    fn classify(domain: glib_sys::GQuark, code: i32) -> LookupFailure {
        let error = error(domain, code);
        // SAFETY: a live `GError` built above and freed below.
        let failure = unsafe { classify_lookup_failure(error) };
        // SAFETY: as above; nothing else holds it.
        unsafe { g_error_free(error) };
        failure
    }

    /// The two shapes EDS answers for an account it simply has no password
    /// for. `NOT_FOUND` is what
    /// `e_source_credentials_provider_impl_password_lookup_sync` sets when
    /// the keyring holds nothing yet; `NOT_SUPPORTED` is the abstract
    /// `ESourceCredentialsProviderImpl` default, which is what an OAuth 2.0
    /// account gets, because the OAuth 2.0 impl matches `can_process` and
    /// then does not override `lookup_sync` at all.
    ///
    /// Neither is a fault: the first becomes a credentials prompt and the
    /// second is followed by a perfectly good token fetch. Reporting them on
    /// GLib's critical channel puts a `g_critical` in the log of every
    /// create and every delete that then succeeds, and aborts the registry
    /// outright under `G_DEBUG=fatal-criticals`.
    #[test]
    fn eds_having_no_password_for_an_account_is_not_a_fault() {
        for code in [G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED] {
            assert_eq!(
                classify(unsafe { g_io_error_quark() }, code),
                LookupFailure::NoPassword,
                "expected G_IO_ERROR code {code} to read as no password"
            );
        }
    }

    /// A lookup the user cancelled is not a fault either. Reachable because
    /// the vfunc's own `GCancellable` goes straight through to libsecret.
    #[test]
    fn a_cancelled_lookup_is_not_a_fault() {
        assert_eq!(
            classify(unsafe { g_io_error_quark() }, G_IO_ERROR_CANCELLED),
            LookupFailure::NoPassword
        );
    }

    /// Everything else stays on the critical channel, which is the whole
    /// point of narrowing it: a lookup that failed for a reason EDS did not
    /// name as one of the above is a system fault worth a report.
    #[test]
    fn any_other_failure_is_still_a_fault() {
        assert_eq!(
            classify(unsafe { g_io_error_quark() }, G_IO_ERROR_FAILED),
            LookupFailure::Fault
        );
    }

    /// The domain is compared as well as the code, and not as
    /// belt-and-braces: `G_IO_ERROR` and `G_DBUS_ERROR` are different
    /// enumerations, so a code alone would read some unrelated bus failure
    /// as "this account has no password" and silence it.
    #[test]
    fn the_same_code_in_another_domain_is_a_fault() {
        assert_eq!(
            classify(unsafe { g_dbus_error_quark() }, G_IO_ERROR_NOT_FOUND),
            LookupFailure::Fault
        );
    }

    /// A `FALSE` with no `GError` at all is what
    /// `e_source_credentials_provider_lookup_sync`'s own
    /// `g_return_val_if_fail` produces when the provider has no
    /// implementation to ask, including the password fallback. That really
    /// is broken, and it keeps its critical.
    #[test]
    fn a_failure_with_no_gerror_is_a_fault() {
        // SAFETY: NULL is this function's documented input.
        assert_eq!(
            unsafe { classify_lookup_failure(ptr::null()) },
            LookupFailure::Fault
        );
    }
}
