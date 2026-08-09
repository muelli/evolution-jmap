// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The three sources an account's mail is: `[Mail Account]`,
//! `[Mail Identity]`, `[Mail Transport]`.
//!
//! [`crate::account`] writes the account itself; this writes the three sources
//! that hang off it. They are separate *sources* rather than three more groups
//! in the account's file, and they are not children of the collection *backend*
//! either — [the collection backend's `prepare_mail`][prepare_mail] sets out
//! why at length: `collection_backend_load_resources()` deletes the cache file
//! of any child whose `dup_resource_id` answers NULL, and every reference
//! implementation answers NULL for exactly the mail extensions. So the mail
//! sources live in the registry's own source directory, parented to the
//! account, and are the setup's to write. That is this module.
//!
//! ## Two writers, one file
//!
//! Everything here is also written by `prepare_mail`, and the duplication is
//! real rather than accidental. The two run in different processes with
//! different things in reach: the vfunc has the factory and *not* the user's
//! answers — EDS hands it the three sources and nothing else, which is why it
//! writes no address — while this runs in Evolution, where the answers are, and
//! where no collection factory instance exists to call the vfunc on.
//!
//! Neither can therefore stand in for the other, and in Evolution 3.52 it is
//! this one that runs at all: `e_collection_backend_factory_prepare_mail` has
//! no caller anywhere in evolution-data-server 3.52.3 or evolution 3.52.3. That
//! is precisely why `tests/mail.rs` holds the two against each other — nothing
//! else would notice the uncalled one going stale, and it is the implementation
//! a later Evolution reaching that hook would get.
//!
//! ## What is written, and what writes it instead
//!
//! - **`Parent`**, on all three, is what makes them this account's mail:
//!   `e_collection_backend_list_mail_sources()` finds them by walking the
//!   account's children, and `collection_backend_bind_child_enabled()` binds
//!   each one's `enabled` to the account's `mail-enabled` on the same walk. It
//!   is written here rather than left to Evolution's assistant — which is
//!   believed to set it too — because a writer that produced a complete account
//!   only when called from one particular caller is a writer whose output
//!   depends on something none of its tests can see.
//! - **The service name** on the account and the transport, which is Camel's
//!   protocol and the first line of `libcameljmap.urls`: `jmap` on both,
//!   because JMAP submits over the session it reads through and
//!   [`jmap-mail`'s provider] registers one protocol with a store type *and* a
//!   transport type in it.
//! - **The two links** — the account's `identity-uid` and the identity's
//!   `[Mail Submission] transport-uid` — which are what make three sources one
//!   account.
//! - **`[Mail Identity] Address`**, from the same string as
//!   `[Collection] Identity`. EDS keeps the address in two places; that they
//!   agree is not EDS's business but the setup's, and an identity that
//!   disagreed with its account would send mail from an address the account
//!   does not claim.
//!
//! Not written here:
//!
//! - **`Enabled`** — bound to the account's `mail-enabled` by the collection
//!   backend on every load, so a value written here is one the registry
//!   overwrites. Which is also why the three sources are written whether or not
//!   [`Parts::mail`](jmap_collection_sync::Parts::mail) is on: "receive mail
//!   for this account" is a switch, and a switch needs something to switch.
//! - **`[Mail Identity] Name`** — the user's display name, which is the
//!   assistant's identity page's to write and which an [`Account`] does not
//!   carry. Blank means Evolution sends `From: <address>`, which is
//!   RFC-conformant and is not something to invent an answer for.
//! - **The Camel settings the store connects with** — host, port, security and
//!   user, which reach a Camel service through an `ESourceCamel` extension
//!   generated for the provider's own settings type. Writing them needs
//!   `jmap-mail`'s `CamelJmapSettings` GType, and therefore Camel, which this
//!   crate does not link. **Until that is written the mail account names a
//!   provider but no server**, which is the next increment and is why M7 is not
//!   done.
//!
//! [prepare_mail]: ../../jmap_backend_collection/prepare_mail/index.html
//! [`jmap-mail`'s provider]: ../../jmap_mail/provider/index.html

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend,
    ESourceMailAccount, ESourceMailIdentity, ESourceMailSubmission,
    e_source_backend_set_backend_name, e_source_get_extension, e_source_get_uid,
    e_source_mail_account_get_type, e_source_mail_account_set_identity_uid,
    e_source_mail_identity_get_type, e_source_mail_identity_set_address,
    e_source_mail_submission_get_type, e_source_mail_submission_set_transport_uid,
    e_source_mail_transport_get_type, e_source_set_parent,
};
use jmap_backend_core::error::cstring_lossy;

use crate::account::Account;

/// The Camel protocol the mail account and the mail transport name — the same
/// string [the collection backend's vfunc][prepare_mail] writes, and the one
/// line in `libcameljmap.urls`.
///
/// One name for both, because there is one provider: RFC 8621 §7 submits over
/// the same session the mail is read through, so there is no second service
/// beside it the way `smtp` sits beside `imapx`.
///
/// [prepare_mail]: ../../jmap_backend_collection/prepare_mail/constant.MAIL_BACKEND_NAME.html
pub const MAIL_BACKEND_NAME: &CStr = c"jmap";

/// The three scratch sources a commit is handed, in the order they are talked
/// about everywhere else — receiving account, identity, transport.
///
/// Raw pointers rather than anything owned: they belong to the setup that is
/// committing them, and [`apply`] keeps none of them past the call.
pub struct MailSources {
    /// What Evolution receives through, and what the folder tree hangs off.
    pub account: *mut ESource,
    /// Who the mail is from — a person, not a service.
    pub identity: *mut ESource,
    /// What Evolution sends through.
    pub transport: *mut ESource,
}

/// Writes the account's three mail sources, which afterwards say exactly this
/// account.
///
/// Like [`crate::account::apply`], every field is written every time rather
/// than only when there is something new to say: a commit lands on sources that
/// already say something, and an address left behind because the writer had
/// nothing to add is the `From:` of every message sent afterwards.
///
/// # Safety
///
/// `collection` and all three sources in `sources` must be valid `ESource`s —
/// the account source and the three scratch sources the setup is committing.
/// This call takes no reference to any of them and nothing here outlives it.
pub unsafe fn apply(collection: *mut ESource, sources: &MailSources, account: &Account) {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a type
    // nothing has referenced yet is one it cannot find — and here it would
    // create nothing and return NULL. `e_source_class_init` happens to
    // `g_type_ensure` every built-in extension, these four included, so a live
    // `ESource` already implies them; referencing them anyway costs one
    // already-registered type lookup each and keeps this module's correctness
    // out of EDS's list of built-ins.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_identity_get_type();
        e_source_mail_submission_get_type();
        e_source_mail_transport_get_type();
    }

    // The strings outlive every call that borrows them, which is what keeps the
    // pointers below valid. Truncating at an interior NUL rather than refusing,
    // as everywhere a typed string crosses into C: what is kept is what the
    // address would have meant to every C caller downstream anyway, and
    // refusing the write would leave the *previous* address on an identity
    // being edited.
    let address = cstring_lossy(&account.identity);

    // SAFETY: valid sources by this function's contract; the uid is owned by
    // the account source and outlives the three setters that copy it, and each
    // extension is created on demand and owned by the source it is asked of.
    unsafe {
        // What makes the three the account's mail rather than three top-level
        // sources the collection knows nothing about.
        let account_uid = e_source_get_uid(collection);
        for source in [sources.account, sources.identity, sources.transport] {
            e_source_set_parent(source, account_uid);
        }

        let mail_account: *mut ESourceMailAccount =
            e_source_get_extension(sources.account, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
                .cast();
        // `ESourceMailAccount` derives from `ESourceBackend`, which is where
        // the Camel protocol lives; so does `ESourceMailTransport` below.
        e_source_backend_set_backend_name(
            mail_account.cast::<ESourceBackend>(),
            MAIL_BACKEND_NAME.as_ptr(),
        );
        e_source_mail_account_set_identity_uid(mail_account, e_source_get_uid(sources.identity));

        let identity: *mut ESourceMailIdentity =
            e_source_get_extension(sources.identity, E_SOURCE_EXTENSION_MAIL_IDENTITY.as_ptr())
                .cast();
        e_source_mail_identity_set_address(identity, address.as_ptr());

        // A group of the identity's rather than a source of its own: where a
        // person's mail leaves through is a property of the person.
        let submission: *mut ESourceMailSubmission = e_source_get_extension(
            sources.identity,
            E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr(),
        )
        .cast();
        e_source_mail_submission_set_transport_uid(submission, e_source_get_uid(sources.transport));

        let transport: *mut ESourceBackend = e_source_get_extension(
            sources.transport,
            E_SOURCE_EXTENSION_MAIL_TRANSPORT.as_ptr(),
        )
        .cast();
        e_source_backend_set_backend_name(transport, MAIL_BACKEND_NAME.as_ptr());
    }
}
