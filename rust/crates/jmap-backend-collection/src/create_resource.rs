// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creates an address book or a calendar *on the server*. This is the part of
//! parity with evolution-ews that a backend which only copies the server cannot
//! do.
//!
//! All other code in this crate reads the server and writes Evolution. This is
//! the only request in the opposite direction. The "New Address Book" and "New
//! Calendar" dialogs of Evolution give the registry a scratch `ESource` for a
//! collection that does not exist yet, and the collection backend must create
//! it. `ECollectionBackendClass::create_resource_sync` is that request, and
//! [`crate::backend`] sets the slot.
//!
//! This module contains the EDS parts of the request.
//! [`jmap_collection_sync::create`] makes the decision between these parts
//! (which JMAP account, which `/set` call, and which [`Child`] the new collection
//! becomes). That crate needs no headers, and its tests use a running
//! `jmap-mockd`.
//!
//! ## The two obligations, from the EDS documentation of the vfunc
//!
//! `e_collection_backend_create_resource_sync()` gives them, and the header shows
//! neither:
//!
//! - *"It is the implementor's responsibility to examine @source and determine
//!   what the equivalent server-side resource would be. If this cannot be
//!   determined without ambiguity, the function must return an error."* Thus
//!   [`requested_of`] exists, and it returns `None` for a scratch source with no
//!   kind.
//! - *"After the server-side resource is successfully created, the implementor
//!   must also add an #ESource to @backend's #ECollectionBackend:server."* Thus
//!   [`adopt_created`] runs, and then the publish. If the code only created the
//!   collection on the server, the dialog would close and no address book would
//!   appear. The child would appear only after the next populate, so the create
//!   would seem to fail silently.
//!
//! ## This override does not call the parent implementation
//!
//! This is the opposite of [`crate::child_added`]. The reason is in the EDS
//! source code, not in its documentation. `collection_backend_create_resource()`,
//! which the default `create_resource_sync` calls through a closure, does only one
//! thing: `g_task_return_new_error (G_IO_ERROR_NOT_SUPPORTED, "%s does not support
//! creating remote resources")`. Thus the parent implementation *is* the refusal
//! that this override replaces. A call to it would make a successful create
//! report a failure.
//!
//! ## EDS owns the scratch source, and this backend completes it
//!
//! `server_side_source_remote_create_cb()` makes the scratch source with
//! `e_server_side_source_new_user_file()`, in the source directory of the
//! registry and not in the cache of this collection. It sets the keyfile from
//! Evolution on the source. It intentionally does **not** add the source to the
//! registry: its comment says that it is "up to the ECollectionBackend whether to
//! use source as given or create its own equivalent". This backend uses the
//! source as given, as evolution-ews does. It completes the source into the same
//! child that a populate writes:
//!
//! - each setting of the [`Child`] of the new collection, through the same
//!   [`crate::child_source::apply`] that a fan-out uses. Thus a created child and
//!   a discovered child have the same fields. This is most important for
//!   `[Resource] Identity`: without it, EDS deletes the cache file;
//! - `parent`, `writable` and `write_directory`. EDS
//!   `collection_backend_new_source()` sets these on a child that it creates, so
//!   the scratch source does not have them. That function also sets `removable`,
//!   but this code intentionally does **not** set it. EDS
//!   `collection_backend_child_added()` sets it to `FALSE` for each child. The
//!   `child_added` vfunc of [`crate::backend`] calls that parent function, so the
//!   child gets the value when it is published.
//!
//! ## This module intentionally does not set `remote-deletable`
//!
//! evolution-ews sets it at each of the three sites that create a child, also
//! here. This backend sets it in one place:
//! [`crate::delete_resource::offer_deletion`], which `child_added` calls. EDS
//! calls `child_added` for each child of a collection. Thus it also covers a
//! created child when it is published, and a discovered or cached child. The
//! flag is thus set elsewhere, not omitted. A discovered child gets a correction
//! of that default later, but a created child does not.
//! [`crate::fan_out::adopt`] gives the per-resource `myRights.mayDelete` answer
//! only in a fan-out, and a create does not run a fan-out. Thus a new collection
//! stays deletable until the next populate reads its rights.
//!
//! ## The code gets the credentials when it needs them, and does not keep them
//!
//! evolution-ews keeps the `ENamedParameters` from its `authenticate_sync` in the
//! backend instance. It makes a new connection from them when necessary. This
//! backend does not need to do this.
//! `e_source_registry_server_ref_credentials_provider()` gives access to the store
//! that supplied the credentials of `authenticate_sync`. [`stored_password_of`]
//! reads it at the time when the password is necessary. Thus:
//!
//! - no secret stays in memory for the life of the account;
//! - no instance state needs initialization and finalization;
//! - a create works in a process where `authenticate_sync` did not run for this
//!   account yet.
//!
//! For an OAuth 2.0 account, the code continues through
//! [`jmap_backend_core::oauth2::access_token`].
//! [`crate::authenticate::login_of`] selects the path. Thus the code always gets a
//! new token, not a stored token that is possibly expired.
//!
//! [`Child`]: jmap_collection_sync::Child

use std::fmt;
use std::ptr;

use eds_sys::{
    ENamedParameters, ESource, ESourceCredentialsProvider, ESourceRegistryServer,
    e_named_parameters_free, e_server_side_source_set_writable,
    e_server_side_source_set_write_directory, e_source_credentials_provider_lookup_sync,
    e_source_dup_display_name, e_source_get_uid, e_source_registry_server_ref_credentials_provider,
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
use jmap_backend_core::marshal::{password as password_of, read_string, take_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::source::{self, ConnectTarget};
use jmap_backend_core::trampoline::{log_critical, log_critical_for_account};
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, CreateFailure, Requested, create_collection};

use crate::child_source::{UnwritableSetting, apply};
use crate::resource_id::kind_of;

/// The reason why a create failed.
#[derive(Debug)]
pub enum CreateError {
    /// The scratch source is not an address book or a calendar. The EDS
    /// documentation of the vfunc says that this ambiguity must be an error.
    UnknownKind,
    /// The account could not supply a login: it has a server that it must not
    /// contact, or it has no credentials.
    Login(crate::authenticate::LoginError),
    /// The connection or the create failed, or the server of the account has no
    /// account for this kind of collection.
    Server(CreateFailure),
    /// The server created the collection, but a setting could not be written to
    /// its source. This is reported, not ignored, and the create fails because of
    /// it (see [`adopt_created`]). The variant contains the kind, so the message
    /// can name the collection that now has no complete source.
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
                // create a collection, and the request does not give the kind.
                c"the new collection is neither an address book nor a calendar",
            )),
            Self::Login(failure) => failure.fmt(f),
            // `Unserved` comes from `jmap-collection-sync`, which builds without
            // gettext. Its `Display` is for developers, so this code writes the
            // text for the user. It uses the translated nouns of all other
            // account errors, not a second pair of msgids for "address book" and
            // "calendar".
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

/// Returns the [`Collection`] of a [`ChildKind`]. Thus the translated noun comes
/// from [`Collection::noun`], and no second pair of msgids is necessary.
///
/// [`crate::delete_resource`] also uses this function for its messages.
pub(crate) fn collection_of(kind: ChildKind) -> Collection {
    match kind {
        ChildKind::AddressBook => Collection::AddressBook,
        ChildKind::Calendar => Collection::Calendar,
    }
}

impl std::error::Error for CreateError {}

impl CreateError {
    /// Returns the `GError` that the vfunc uses to report this error.
    ///
    /// A `jmap_client::Error` goes through
    /// [`jmap_backend_core::error::to_gerror`], the mapping that all other
    /// operations in these backends use. Thus a 401 on a create becomes the usual
    /// Evolution error code for a 401. All other errors become
    /// `E_CLIENT_ERROR_INVALID_ARG`. Each of them is an EDS input that the code
    /// cannot use, or a fault in this backend. Neither is a reason to report the
    /// account as broken.
    pub fn to_gerror(&self) -> *mut GError {
        match self {
            Self::Login(failure) => failure.to_gerror(),
            Self::Server(CreateFailure::Client(error)) => to_gerror(error),
            other => invalid_arg_gerror(&other.to_string()),
        }
    }
}

/// Returns the request of the scratch source that `create_resource_sync` gets, or
/// `None` if the source does not give one.
///
/// `None` is the EDS "cannot be determined without ambiguity". This backend has
/// no server-side object for a source without the `[Address Book]` or the
/// `[Calendar]` extension. A guessed kind would create the incorrect type of
/// collection, with a name that the user selected for the other type.
///
/// The function uses [`kind_of`], which tests for the extensions and does not get
/// them. A scratch source is a real `ESource` that EDS writes to disk, and
/// `e_source_get_extension` adds the group that it gets.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource` that outlives the call. The
/// function only reads it.
pub unsafe fn requested_of(source: *mut ESource) -> Option<Requested> {
    // SAFETY: the contract of the caller is the contract of `kind_of`.
    let kind = unsafe { kind_of(source) }?;
    // SAFETY: a valid source, because `kind_of` returned `Some` for it. The
    // `dup_` getter gives a copy that a setter on another thread cannot free.
    let display_name = unsafe { take_string(e_source_dup_display_name(source)) };
    Some(Requested {
        kind,
        display_name: display_name.unwrap_or_default(),
    })
}

/// Connects with `target` and `credentials`, and creates `requested` on that
/// server.
///
/// This is one function, so the vfunc has one operation to do with a
/// [`Login`](crate::authenticate::Login), and one error type to report. Other
/// code makes both decisions. [`jmap_backend_core::source::connect`] makes the
/// connection, so an account with only a domain gets the same `_jmap._tcp`
/// autodiscovery as a fan-out. [`create_collection`] does the create.
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

/// Completes the scratch source into the child of this collection that `child`
/// describes.
///
/// This function does all steps except the publish. The publish needs the
/// registry server, so it stays in [`crate::backend`]. The steps, in order:
///
/// 1. Each [`Setting`](jmap_collection_sync::Setting) of the child, through the
///    same [`apply`] that a fan-out uses for a discovered child. An error here
///    makes the create fail, although the collection is already on the server.
///    This is the correct answer. The alternative is to export a child that is
///    only partly written, and [`crate::child_source`] exists to prevent that.
///    The collection stays on the server. The next populate finds it and writes
///    a correct child for it. A visible extra collection is better than a source
///    that looks correct but connects to no server.
/// 2. `parent`, so the source is a child of this account and not a top-level
///    source. Also, `child_added` then fires for it when it is published.
/// 3. `write_directory`, so EDS writes the `.source` file into the cache
///    directory of this collection. The children that a fan-out creates are also
///    there, and a removal of the account also removes them. Without this step,
///    the file stays in the user directory of the registry, where EDS created it.
/// 4. `writable`, which lets the user rename the address book later.
///
/// Steps 2 to 4 are the steps of `collection_backend_new_source()`, without
/// `removable = FALSE`. `collection_backend_child_added()` sets that value on
/// each child when it is published (see the module comment).
///
/// `cache_dir` is `None` only for a collection backend without a cache directory
/// from EDS. This must not occur, because EDS derives one from the account uid.
/// The function then does not change the write location, and does not return an
/// error. The source still works. EDS only writes it in its original location.
///
/// # Safety
///
/// `scratch` must be a valid `EServerSideSource`. The source that EDS gives
/// `create_resource_sync` is always one, because
/// `server_side_source_remote_create_cb` makes it with
/// `e_server_side_source_new`. The function takes no reference, and nothing
/// stays after the call.
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

    // SAFETY: a valid source because of the contract of this function.
    unsafe { apply(scratch, &child.settings(connection)) }?;

    let account_uid = cstring_lossy(account_uid);
    // SAFETY: as above. The string is NUL-terminated, and the setter copies it.
    unsafe { e_source_set_parent(scratch, account_uid.as_ptr()) };

    if let Some(cache_dir) = cache_dir {
        let cache_dir = cstring_lossy(cache_dir);
        // SAFETY: as above. The source is an `EServerSideSource` because of the
        // contract of this function, and the setter copies the string.
        unsafe { e_server_side_source_set_write_directory(scratch.cast(), cache_dir.as_ptr()) };
    }

    // SAFETY: as above.
    unsafe { e_server_side_source_set_writable(scratch.cast(), GTRUE) };

    Ok(())
}

/// Returns the name of the kind of a refused create, for a log line.
///
/// The user does not see this name; the user sees [`CreateError`]. But a refused
/// create must have a line on the EDS log channel, and "address book" or
/// "calendar" is easier to read there than a `Debug` of the enum.
pub fn kind_noun(kind: ChildKind) -> &'static str {
    match kind {
        ChildKind::AddressBook => "address book",
        ChildKind::Calendar => "calendar",
    }
}

/// Returns the password that EDS stores for `source`. The function gets it from
/// the credentials provider of the registry.
///
/// This is the same store that supplies the `ENamedParameters` of
/// `authenticate_sync`. A backend that does not get them can access the store
/// only through the provider of `e_source_registry_server_ref_credentials_provider()`.
/// That provider finds the *credentials source* itself: a child that uses the
/// password of its collection gets the password of the collection. Thus a lookup
/// for the account source asks the same question that `authenticate_sync` got an
/// answer to.
///
/// `None` means "this account has no password". For an account with a user name,
/// this becomes [`ConnectError::CredentialsRequired`], and thus a prompt. A
/// *failed* lookup also returns `None`, and a log line gives the reason. For the
/// caller, the two cases are the same: this operation has no password, and in
/// both cases the prompt is the useful message for the user.
///
/// **[`classify_lookup_failure`] selects the log channel for that line**, and this
/// is important. Most failures of that call are not real failures. An OAuth 2.0
/// account returns `G_IO_ERROR_NOT_SUPPORTED`, and then the token path works
/// correctly. A keyring that is still empty returns `G_IO_ERROR_NOT_FOUND`, and
/// then a prompt occurs. Only a lookup that fails for a different reason (no
/// credentials provider, or libsecret not reachable) is a system fault for the
/// GLib critical channel.
///
/// `context` names the vfunc for the critical channel. The vfunc of
/// [`crate::delete_resource`] also uses this lookup, and a log line with the
/// incorrect name would send a reader to the incorrect module. Each failure here
/// also has the uid of `source` as a structured `account_id` field, through
/// [`log_critical_for_account`]. No single resource is the cause, because a
/// credentials lookup applies to the full account.
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
    // SAFETY: a valid source because of the contract of this function. The uid
    // is `(transfer none)`, borrowed for the duration of this call.
    let account_id = unsafe { read_string(e_source_get_uid(source)) };
    let has_server = !server.is_null();
    tracing::debug!(
        account_id = account_id.as_deref(),
        has_server,
        context,
        "querying stored password for collection resource"
    );

    if server.is_null() {
        // The backend has a weak reference to the registry server. Thus NULL
        // means that the server is gone (for example, during shutdown), and no
        // provider is available.
        report_critical(
            account_id.as_deref(),
            format!("{context}: the registry server is gone; no credentials to look up"),
        );
        return None;
    }

    // SAFETY: a valid registry server. The provider is `(transfer full)`.
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

    // SAFETY: a live provider, a valid source and a cancellable that agrees with
    // the contract of this function.
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

/// Calls [`log_critical_for_account`] if `account_id` is known, else the plain
/// [`log_critical`]. The uid of the account is possibly not readable. EDS
/// probably never makes an `ESource` without a uid, but the contract of this
/// function does not assume it.
fn report_critical(account_id: Option<&str>, message: String) {
    match account_id {
        Some(account_id) => log_critical_for_account(account_id, &message),
        None => log_critical(&message),
    }
}

/// Calls `e_source_credentials_provider_lookup_sync` once, and releases the two
/// values that it returns.
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

    // SAFETY: the contract above, and two writable out-parameters set to NULL.
    // The credentials and the `GError` are both `(transfer full)`.
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
        // Classify the error before taking the message. `take_message` frees
        // the `GError`, and the classification reads its domain and code.
        // SAFETY: the call failed, so `error` is NULL or a live `GError`.
        let failure = unsafe { classify_lookup_failure(error) };
        // SAFETY: the call failed, so `error` is NULL or a `GError` that this
        // function now owns. Its message is a string that the struct owns.
        let message = unsafe { take_message(error) };
        match failure {
            // This is not a fault. The `None` of the caller already gives the
            // user all that is necessary: a prompt, or a token fetch that needs
            // no password.
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

    // Only a defective callee sets an error on success. Free the error, because
    // a leak causes a report. `crate::removal` does the same.
    if !error.is_null() {
        // SAFETY: a `GError` that this call owns and that nothing else holds.
        unsafe { g_error_free(error) };
    }

    // SAFETY: the lookup succeeded, so `credentials` is NULL or an
    // `ENamedParameters` that this call owns. `password_of` only reads it.
    let password = unsafe { password_of(credentials) };
    if !credentials.is_null() {
        // SAFETY: this function got ownership through the out-parameter. The
        // borrow of `password_of` has ended, because it returns an owned `String`.
        unsafe { e_named_parameters_free(credentials) };
    }

    password
}

/// The meaning of a `FALSE` from `e_source_credentials_provider_lookup_sync`.
///
/// This distinction is necessary because many failures of that call are not real
/// failures (see [`classify_lookup_failure`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LookupFailure {
    /// EDS has no password for this account. This is a normal state, not a
    /// fault. The caller returns `None` in both cases. Only the log channel is
    /// different.
    NoPassword,
    /// An operation that must not fail failed. A system fault goes to the GLib
    /// critical channel.
    Fault,
}

/// Returns the type of a failed lookup, from the domain and code and not from
/// the message text.
///
/// Three `G_IO_ERROR` codes are normal. The reason for each is in the EDS source
/// code:
///
/// - `e_source_credentials_provider_impl_password_lookup_sync` sets
///   `G_IO_ERROR_NOT_FOUND` when the keyring has no entry for the source yet.
///   [`stored_password_of`] documents that this state becomes a credentials
///   prompt.
/// - `G_IO_ERROR_NOT_SUPPORTED` is the default of the abstract
///   `ESourceCredentialsProviderImpl`. An **OAuth 2.0** account returns it:
///   `e_source_credentials_provider_impl_oauth2` accepts the source through
///   `can_process`, and overrides only `can_store` and `can_prompt`. Thus
///   `lookup_sync` keeps the refusal of the base class. An OAuth 2.0 account
///   never has a password, and [`crate::authenticate::login_of`] then gets a
///   token and succeeds. Thus this code occurs on the *success* path of each
///   create and each delete of such an account.
/// - `G_IO_ERROR_CANCELLED` occurs when the user selects Stop. It gets to
///   libsecret because the code gives the `GCancellable` of the vfunc directly to
///   the lookup.
///
/// All other failures stay critical, also a `FALSE` without a `GError`. The
/// `g_return_val_if_fail` of the provider gives that result when the provider has
/// no implementation to ask, not even the password fallback.
///
/// # Safety
///
/// `error` must be NULL or a valid `GError` that outlives the call.
unsafe fn classify_lookup_failure(error: *const GError) -> LookupFailure {
    if error.is_null() {
        return LookupFailure::Fault;
    }

    // SAFETY: a live `GError` because of the contract above. `g_io_error_quark`
    // has no arguments.
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

/// Returns the message of a `GError` that this call owns, then frees the error.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may consume.
unsafe fn take_message(error: *mut GError) -> String {
    if error.is_null() {
        return "EDS gave no reason".to_owned();
    }

    // SAFETY: a live GError. Its message is a NUL-terminated string that it owns.
    let message = unsafe { read_string((*error).message) };
    // SAFETY: this function got ownership through the out-parameter.
    unsafe { g_error_free(error) };

    message.unwrap_or_else(|| "EDS gave no message".to_owned())
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use gio_sys::{G_IO_ERROR_FAILED, g_dbus_error_quark};

    use super::*;

    /// Makes a real `GError`, not a struct made by hand, because
    /// [`classify_lookup_failure`] reads its `domain` and `code` fields
    /// directly. The tests of `jmap_backend_core::oauth2` use the same helper.
    fn error(domain: glib_sys::GQuark, code: i32) -> *mut GError {
        let message = CString::new("boom").unwrap();
        // SAFETY: a valid domain and a NUL-terminated message. Each caller below
        // frees the result.
        unsafe { glib_sys::g_error_new_literal(domain, code, message.as_ptr()) }
    }

    fn classify(domain: glib_sys::GQuark, code: i32) -> LookupFailure {
        let error = error(domain, code);
        // SAFETY: a live `GError` that the code above makes and the code below
        // frees.
        let failure = unsafe { classify_lookup_failure(error) };
        // SAFETY: as above. Nothing else holds it.
        unsafe { g_error_free(error) };
        failure
    }

    /// The two errors that EDS returns for an account without a password.
    /// `e_source_credentials_provider_impl_password_lookup_sync` sets
    /// `NOT_FOUND` when the keyring has no entry yet. `NOT_SUPPORTED` is the
    /// default of the abstract `ESourceCredentialsProviderImpl`. An OAuth 2.0
    /// account gets it, because the OAuth 2.0 implementation accepts the source
    /// in `can_process` and does not override `lookup_sync`.
    ///
    /// Neither error is a fault. The first becomes a credentials prompt, and
    /// after the second a token fetch succeeds. A report on the GLib critical
    /// channel puts a `g_critical` in the log of each create and each delete
    /// that then succeeds. Under `G_DEBUG=fatal-criticals`, it also stops the
    /// registry.
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

    /// A lookup that the user cancels is also not a fault. This can occur
    /// because the `GCancellable` of the vfunc goes directly to libsecret.
    #[test]
    fn a_cancelled_lookup_is_not_a_fault() {
        assert_eq!(
            classify(unsafe { g_io_error_quark() }, G_IO_ERROR_CANCELLED),
            LookupFailure::NoPassword
        );
    }

    /// All other failures stay on the critical channel. This is the purpose of
    /// the classification: a lookup that fails for a reason that is not in the
    /// list above is a system fault, and it must be reported.
    #[test]
    fn any_other_failure_is_still_a_fault() {
        assert_eq!(
            classify(unsafe { g_io_error_quark() }, G_IO_ERROR_FAILED),
            LookupFailure::Fault
        );
    }

    /// The function compares the domain and the code, and the domain check is
    /// necessary. `G_IO_ERROR` and `G_DBUS_ERROR` are different enumerations.
    /// With only the code, an unrelated bus failure would read as "this account
    /// has no password", and no report would occur.
    #[test]
    fn the_same_code_in_another_domain_is_a_fault() {
        assert_eq!(
            classify(unsafe { g_dbus_error_quark() }, G_IO_ERROR_NOT_FOUND),
            LookupFailure::Fault
        );
    }

    /// The `g_return_val_if_fail` of
    /// `e_source_credentials_provider_lookup_sync` returns `FALSE` without a
    /// `GError` when the provider has no implementation to ask, not even the
    /// password fallback. This is a real fault, and it stays critical.
    #[test]
    fn a_failure_with_no_gerror_is_a_fault() {
        // SAFETY: NULL is a documented input of this function.
        assert_eq!(
            unsafe { classify_lookup_failure(ptr::null()) },
            LookupFailure::Fault
        );
    }
}

#[cfg(test)]
mod kind_noun_tests {
    use super::*;

    /// Each kind has its own name. Without this test, a `kind_noun` that always
    /// returns one fixed string still compiles.
    #[test]
    fn kind_noun_names_each_kind() {
        assert_eq!(kind_noun(ChildKind::AddressBook), "address book");
        assert_eq!(kind_noun(ChildKind::Calendar), "calendar");
    }
}

#[cfg(test)]
mod create_error_display_tests {
    use crate::authenticate::LoginError;
    use jmap_backend_core::source::SourceError;
    use jmap_collection_sync::CreateFailure;

    use super::*;

    /// The `Display` of `CreateError` writes text for each variant. Without this
    /// test, a `Display` that writes nothing still compiles, and a refused create
    /// shows an empty error message.
    #[test]
    fn every_variant_displays_a_non_empty_message() {
        let unknown_kind = CreateError::UnknownKind;
        assert!(!unknown_kind.to_string().is_empty());

        let login = CreateError::Login(LoginError::Account(SourceError::MissingHost));
        assert_eq!(login.to_string(), SourceError::MissingHost.to_string());

        let unserved = CreateError::Server(CreateFailure::Unserved(ChildKind::Calendar));
        assert!(unserved.to_string().contains("calendar"));

        let unwritable = CreateError::Unwritable(
            ChildKind::AddressBook,
            UnwritableSetting::UnknownProperty {
                group: "Authentication",
                key: "Host",
            },
        );
        assert!(unwritable.to_string().contains("address book"));
    }
}

#[cfg(test)]
mod create_error_to_gerror_tests {
    use super::*;

    /// `UnknownKind` goes through `invalid_arg_gerror`, as do all variants except
    /// `Login` and a server `Client` failure. Without this test, a `to_gerror`
    /// that always returns the null pointer causes no failure.
    #[test]
    fn unknown_kind_carries_the_invalid_arg_code_and_its_own_message() {
        let error = CreateError::UnknownKind;
        let gerror = error.to_gerror();
        assert!(!gerror.is_null());
        // SAFETY: a newly allocated GError that this test owns.
        unsafe {
            assert_eq!((*gerror).domain, eds_sys::e_client_error_quark());
            assert_eq!((*gerror).code, eds_sys::E_CLIENT_ERROR_INVALID_ARG as i32);
            let message = read_string((*gerror).message).unwrap();
            assert_eq!(message, error.to_string());
            glib_sys::g_error_free(gerror);
        }
    }
}

#[cfg(test)]
mod report_critical_tests {
    use jmap_tracing_capture::capture;

    use super::*;

    /// With an account id, the line has the id as a structured field. Without
    /// this test, an empty `report_critical` causes no failure.
    #[test]
    fn with_an_account_id_the_line_carries_it() {
        let (_, entries) = capture(|| report_critical(Some("account-1"), "boom".to_owned()));
        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "account-1"),
            "expected an account_id=account-1 field, got {entries:?}"
        );
    }

    /// Without an account id, the code still logs the line, but without that
    /// field.
    #[test]
    fn with_no_account_id_the_line_is_still_logged() {
        let (_, entries) = capture(|| report_critical(None, "boom".to_owned()));
        assert!(!entries.is_empty(), "expected a critical entry, got none");
        assert!(
            entries.iter().all(|(_, name, _)| name != "account_id"),
            "did not expect an account_id field, got {entries:?}"
        );
    }
}
