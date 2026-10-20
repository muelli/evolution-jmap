// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP account setup (M7): what Evolution's account editor writes, and
//! the `module-jmap-configuration.so` that writes it.
//!
//! The other crates in this repository *read* an account. `jmap-backend-book`
//! and `jmap-backend-cal` read the child source that they get.
//! `jmap-backend-collection` reads the account source from the registry and
//! writes its child sources. All of their tests start from a `.source` keyfile
//! written by hand: in Evolution, the setup UI writes the account file, and this
//! project had no setup UI. This crate is that setup UI.
//!
//! - [`account`] is the base for all other modules: the collection `ESource`
//!   that *is* the account, written from what the user typed. It is the exact
//!   inverse of the collection backend's `collection_source`.
//!   `tests/account.rs` writes an account with this module and reads it back
//!   with that function, so that the two descriptions of one keyfile cannot
//!   drift. [`account::read`] fills the widgets and gives the vfuncs their
//!   account. It is total, but the collection backend's reader is fallible:
//!   while the user types, most accounts in the dialog are not complete yet.
//! - [`mail`] writes the three mail sources of the account: `[Mail Account]`,
//!   `[Mail Identity]` and `[Mail Transport]`. They are separate sources, not
//!   groups in the account file, and not children of the collection *backend*.
//!   Thus, to write them is a different operation from writing the account.
//!   The collection backend's `prepare_mail` is a second *writer* of the same
//!   three sources, and `tests/mail.rs` compares the two. [`mail`] also has a
//!   reader: the `CamelSettings` object that an `ESourceCamel` extension gives
//!   a `CamelJmapStore`. There, the server that a setup wrote becomes the
//!   server that the provider connects to. The same test compares it with the
//!   `ServerConfig` of `jmap-mail`.
//! - [`defaults`] is the account that the dialog shows when the user first
//!   opens it. It comes from the only answer the assistant has at that time:
//!   the address from its identity page. For JMAP this is a good default,
//!   because RFC 8620 §2.2 makes the domain of the address the place where a
//!   client asks (the module tells why). [`complete::check`] accepts this
//!   account, so the assistant does not open on a page with a disabled *Next*
//!   and nothing to correct. The collection backend reads it back with the
//!   origin that the address named.
//! - [`complete`] decides if a commit can occur. It is the deciding half of
//!   the `check_complete` vfunc of `EMailConfigServiceBackend`. The decision
//!   is ordinary Rust over an [`account::Account`], so tests can examine it,
//!   but they cannot examine the widget that calls it. An account that it
//!   accepts is also an account that the collection backend's `server_of`
//!   accepts. The tests commit each case and read it back to prove this: an
//!   account that the setup accepts and the registry rejects fails everywhere
//!   except in the dialog.
//! - [`oauth2`] is not part of the setup dialog. It is the storage for the
//!   discovered endpoints and the registered client id of a deployment: the
//!   `[JMAP OAuth2]` extension, or upstream's `[OAuth2 Client]` on EDS 3.63.1
//!   and newer. The `EOAuth2Service` interface reads these values
//!   synchronously, because discovery and registration need the network and
//!   its vfuncs cannot use the network.
//! - [`oauth2_service`] is that interface: a service that answers every
//!   per-account vfunc from the storage of [`oauth2`]. All other vfuncs keep
//!   the EDS default, which is correct when the per-account vfuncs have
//!   values. [`module`] registers it if EDS has no `EOAuth2ServiceDynamic`.
//! - [`oauth2_setup`] is the network part: RFC 8414 discovery and RFC 7591
//!   registration. It changes a host name into the [`oauth2::Config`] that
//!   [`oauth2::apply`] stores. Only [`config_lookup`] calls it, not the
//!   server settings page.
//! - [`config_lookup`] is the `EConfigLookupWorker` behind the "Look Up
//!   Account Details" step of the assistant.
//! - [`source_config`] contains the two `ESourceConfigBackend` subclasses that
//!   show JMAP in the *New Address Book* and *New Calendar* dialogs.
//! - [`backend`] is the GObject that gives access to [`account`], [`mail`],
//!   [`defaults`] and [`complete`]: the `EMailConfigServiceBackend` subclass
//!   that the *Receiving Email* page of Evolution creates for the JMAP
//!   provider. It holds the name that the page uses to find this backend, the
//!   initial account, and the decision if the account on screen can be
//!   committed. Each vfunc was added when the decision behind it became
//!   testable, in the same sequence as these four modules.
//!
//! - [`module`] contains the bodies of `e_module_load` and `e_module_unload`,
//!   the two symbols that the Evolution shell resolves from
//!   `module-jmap-configuration.so`. The symbols themselves are in the
//!   `jmap-config-module` cdylib. [`module`] also registers the class of
//!   [`backend`] dynamically against the `GTypeModule` that it gets. There is
//!   no factory, and no registry to register the class *with*: the account
//!   editor of Evolution finds a mail config backend in the children of
//!   `EExtension`. Thus it is sufficient to put the type in the type system.
//!   The module tells why.
//!
//! ## Why the decisions came before the module
//!
//! The M7 deliverable is a GObject module that Evolution dlopens from *its*
//! module directory (`pkg-config --variable=moduledir evolution-shell-3.0`),
//! one directory away from the registry module that M6 installs. But the parts
//! that make decisions (which properties make an account, and what the answers
//! of the user become) need no Evolution headers. They are `ESource` writes,
//! and a plain test can make and read back an `ESource` with no display, no
//! session bus and no running Evolution.
//!
//! Thus these parts came first, with tests. Then came the
//! `EMailConfigServiceBackend` subclass that calls them ([`backend`]), and
//! then the module that registers it. GUI code cannot be verified on the
//! development machine, so a smaller GUI part of M7 means that more of M7 is
//! *checked*, not only compiled. For the same reason, the two writers are
//! complete: a store can open an account that this crate commits, and a
//! transport can send through it, with no step left for the caller. Also for
//! this reason, [`complete`] is a function and not a vfunc. This crate makes
//! all *decisions* of the subclass. The subclass keeps only the widgets and
//! the plumbing, and no test on this machine can cover those.
//!
//! ## What is not here yet
//!
//! **A dialog that is known to operate.** The module registers a class that
//! Evolution will create, and nothing more is checked. These items need a
//! running Evolution and a display, and this machine has neither: a shell that
//! dlopens the installed `.so` shows JMAP in the account type list, and the
//! page that opens behaves correctly. See [`backend`] for the state of the
//! dialog and for the items that a person must still confirm.
//!
//! **`insert_widgets`**, the one vfunc that builds something, is written but
//! not run on this machine (as with all GUI code in this milestone). It shows
//! all five fields of
//! [`Connection`](jmap_collection_sync::child_source::Connection) (the server,
//! the port, the login name, the authentication method and the encryption),
//! and a status label for the refusal reason of [`complete::Incomplete`].
//! They are bound to the same collection that the other vfuncs read and write.
//! The other vfuncs are compositions of the modules above:
//!
//! - `setup_defaults` is [`defaults::from_identity`], limited to the fields
//!   that the address determines, on the collection.
//! - `check_complete` is [`account::read`] and then [`complete::check`].
//! - `commit_changes` is the same reader and then [`mail::apply_server`] on
//!   the one scratch mail source of the backend.
//!
//! The tests here can operate those compositions, but not the vfuncs that
//! Evolution dispatches, and not `insert_widgets`. [`backend`] tells the
//! state of each slot, and which source a commit still leaves with no server.
//!
//! As with the backends, this crate needs the installed EDS headers, so it is
//! not in the `default-members` of the workspace. CMake runs its tests through
//! the `rust-test-eds` target.

pub mod account;
pub mod backend;
pub mod complete;
pub mod config_lookup;
pub mod defaults;
pub mod mail;
pub mod module;
pub mod oauth2;
pub mod oauth2_service;
pub mod oauth2_setup;
pub mod source_config;
