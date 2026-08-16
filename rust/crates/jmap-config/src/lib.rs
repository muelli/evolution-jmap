// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JMAP account setup (M7): what Evolution's account editor writes, and
//! eventually the `module-jmap-configuration.so` it writes it from.
//!
//! Everything in this repository so far *reads* an account. `jmap-backend-book`
//! and `jmap-backend-cal` read the child source they are handed,
//! `jmap-backend-collection` reads the account source the registry hands it and
//! writes the children that hang off it — and every one of those tests starts
//! from a `.source` keyfile written by hand, because in Evolution the account
//! file itself is written by the setup UI and this project had none. That is
//! what this crate is for.
//!
//! - [`account`] is the first piece and the one the rest hangs off: the
//!   collection `ESource` an account *is*, written from what the user typed.
//!   It is the exact inverse of the collection backend's
//!   `collection_source`, which is not a coincidence and is not left to be
//!   one — `tests/account.rs` writes an
//!   account with this and reads it back with that, because two descriptions of
//!   one keyfile that are only checked separately are two descriptions that
//!   drift. It reads as well as writes: [`account::read`] is what the widgets
//!   are filled from and what the two vfuncs that decide anything are handed,
//!   and it is total where the collection backend's reader is fallible —
//!   a dialog being typed into is full of accounts that are not yet accounts.
//! - [`mail`] is the three sources that hang off it — `[Mail Account]`,
//!   `[Mail Identity]`, `[Mail Transport]`. Separate sources rather than three
//!   more groups in the account's file, and not children of the collection
//!   *backend* either, which is why writing them is a different operation from
//!   writing the account rather than a longer version of it. Here the other
//!   side of the join is not a reader but a second *writer*: the collection
//!   backend's `prepare_mail` writes the same three sources from the registry,
//!   and `tests/mail.rs` holds the two against each other. It also has a reader
//!   the account source does not: the `CamelSettings` object an
//!   `ESourceCamel` extension hands a `CamelJmapStore`, which is where the
//!   server a setup wrote turns into the server the provider connects to, and
//!   which `jmap-mail`'s own `ServerConfig` is asked about in the same test.
//! - [`defaults`] is what comes before either of them: the account the dialog
//!   already says when the user first reaches it, from the one answer the
//!   assistant has by then — the address off its identity page. For JMAP that
//!   is unusually well-determined, because RFC 8620 §2.2 makes the address's
//!   own domain the place a client asks; the module says so at length. Its
//!   joins are with both of the above: the account it offers is one
//!   [`complete::check`] accepts, so the assistant does not open on a page
//!   whose *Next* is greyed out with nothing on it to fix, and one the
//!   collection backend reads back as the origin the address named.
//! - [`complete`] is the other direction: not what a commit writes but whether
//!   there is to be one. It is the deciding half of
//!   `EMailConfigServiceBackend`'s `check_complete` vfunc, and it is here
//!   for the same reason the two writers are — the decision is ordinary Rust
//!   over an [`account::Account`] and can be tested, while the widget that will
//!   ask it cannot be. Its join is with the *readers*: an account it accepts is
//!   one the collection backend's `server_of` accepts, asserted by committing
//!   each case and reading it back, because a setup that accepts what the
//!   registry rejects has written an account that fails everywhere except in
//!   the dialog it was typed into.
//! - [`oauth2`] is not part of the setup dialog at all, but the storage the
//!   real-server-readiness track needs beside it: a fifth `ESource`
//!   extension, `[JMAP OAuth2]`, holding a deployment's discovered endpoints
//!   and this client's registered id — the answer the `EOAuth2Service`
//!   interface reads out synchronously, since discovering and registering
//!   both need the network and its vfuncs cannot.
//! - [`oauth2_service`] is that interface: a service that answers every
//!   per-account vfunc out of [`oauth2`]'s storage and leaves every other
//!   vfunc at EDS's own default, which already does the right thing once the
//!   per-account ones are filled in. Not yet registered with an
//!   `EOAuth2Services` registry or reachable from any module load path —
//!   also the setup UI's job, once it exists.
//! - [`oauth2_setup`] is the network half neither of the above can be: RFC
//!   8414 discovery and RFC 7591 registration, turning a host name into the
//!   [`oauth2::Config`] [`oauth2::apply`] stores. Deliberately not reachable
//!   from the dialog yet — driving the user to the authorization endpoint and
//!   choosing a redirect URI is the consent exchange itself, `insert_widgets`'
//!   job once it exists.
//! - [`backend`] is the GObject the four above are reached through: the
//!   `EMailConfigServiceBackend` subclass Evolution's *Receiving Email* page
//!   instantiates for the JMAP provider. It carries the name the page finds
//!   this backend by, the account a new one starts as, and whether the one on
//!   screen may be committed — each further vfunc lands there as the decision
//!   behind it becomes testable, which is the same order the four modules above
//!   were written in.
//!
//! - [`module`] is how the rest of it arrives: the bodies of `e_module_load`
//!   and `e_module_unload`, the two symbols Evolution's shell resolves out of
//!   `module-jmap-configuration.so` (the symbols themselves are in the
//!   `jmap-config-module` cdylib), and the dynamic registration of
//!   [`backend`]'s class against the `GTypeModule` it is handed. There is no
//!   factory and nothing to register the class *with* — Evolution's account
//!   editor finds a mail config backend by walking the children of `EExtension`,
//!   so putting the type in the type system is the whole of it; the module says
//!   why at length.
//!
//! ## Why the decisions came before the module
//!
//! M7's deliverable is a GObject module Evolution dlopens out of *its* module
//! directory (`pkg-config --variable=moduledir evolution-shell-3.0`), one
//! directory over from the registry module M6 installs. But the parts of it
//! that decide anything — which properties an account is, and what the user's
//! answers turn into — need no Evolution headers at all: they are `ESource`
//! writes, and an `ESource` can be built and read back in a plain test with no
//! display, no session bus and no running Evolution.
//!
//! So those came first and are tested, then the `EMailConfigServiceBackend`
//! subclass that calls them ([`backend`]), and only then the module that
//! registers it. The roadmap's rule about this milestone is the reason for the
//! order: GUI code cannot be verified on the machine this is developed on, so
//! the smaller the part of M7 that is GUI, the more of M7 is actually *checked*
//! rather than merely compiled. It is also why the two writers above are as
//! complete as they are — an account this crate commits is one a store can open
//! and a transport can send through, with no step left for the caller to
//! remember — and why [`complete`] is a function rather than a vfunc:
//! everything the subclass has to *decide* is decided here, so what is left for
//! it is the widgets and the plumbing, which is the part no test on this
//! machine could cover anyway.
//!
//! ## What is not here yet
//!
//! **The dialog actually working.** The module registers a class Evolution will
//! instantiate, and that is as far as anything here has been checked: that a
//! shell which dlopens the installed `.so` then offers JMAP in the account type
//! list, and that the page it opens behaves, needs a running Evolution and a
//! display this machine has neither of. See [`backend`] for the state the
//! dialog is left in and exactly what a human still has to confirm.
//!
//! **The one vfunc that has to build something**: `insert_widgets`, now written
//! but — like everything else GUI in this milestone — not run on this machine.
//! It puts two of [`Connection`](jmap_collection_sync::child_source::Connection)'s five fields on screen, the server
//! and the login name, bound to the same collection the other four vfuncs
//! already read and write; a port entry, a security toggle and a status label
//! for [`complete::Incomplete`]'s refusal reason are not there yet. The other
//! four vfuncs are compositions of the modules above — `setup_defaults` is
//! [`defaults::from_identity`] narrowed onto the collection, `check_complete` is
//! [`account::read`] and [`complete::check`] over it, and `commit_changes` is
//! that same reader followed by [`mail::apply_server`] onto the one scratch mail
//! source a backend holds. What a test here can drive is those compositions
//! rather than the vfuncs Evolution dispatches, and `insert_widgets` itself not
//! at all. [`backend`] says so slot by slot, including which source a commit
//! still leaves without a server.
//!
//! Like the backends, this crate needs the installed EDS headers and so stays
//! out of the workspace's `default-members`; CMake runs its tests via the
//! `rust-test-eds` target.

pub mod account;
pub mod backend;
pub mod complete;
pub mod defaults;
pub mod mail;
pub mod module;
pub mod oauth2;
pub mod oauth2_service;
pub mod oauth2_setup;
