// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw, unsafe FFI bindings to the Evolution *application* libraries:
//! `evolution-shell-3.0` and `evolution-mail-3.0`. [`eds-sys`] plays the
//! same role for the Evolution *Data Server* libraries.
//!
//! This is two libraries in two crates, for this reason: the backends
//! loaded by `evolution-source-registry`, and the data factories, are EDS
//! processes that never link GTK. The setup module is loaded by Evolution
//! itself, which does link GTK. Generating both surfaces into one crate
//! would put GTK, WebKit, and Evolution's own libraries behind every
//! address book backend that only ever wanted `ESource`.
//!
//! The scope of this crate is deliberately narrow. It covers
//! `EMailConfigServiceBackend`, the `EExtension` that Evolution's
//! *Receiving Email* page instantiates one of per known mail provider. Its
//! vfuncs make decisions already written and tested in plain Rust over an
//! `ESource`, in [`jmap-config`]. This crate is how those vfuncs get
//! reached.
//!
//! `EConfigLookupWorker` joins this crate for a related but distinct
//! reason. It is not a class Evolution instantiates per provider. It is an
//! interface that an `EExtension`, registered against `EConfigLookup`,
//! implements to run account-setup-time server discovery off the GTK main
//! thread. This is the mechanism this crate exists to reach, for OAuth 2.0
//! discovery. evolution-ews's `e-ews-config-lookup.c` uses the same
//! mechanism for Exchange autodiscovery. This crate generates the
//! interface's vtable the same way it generates the backend class's own,
//! and fills it the same way `jmap-config/src/oauth2_service.rs` fills
//! `EOAuth2Service`'s.
//!
//! One accessor of the page that class extends comes with it:
//! `e_mail_config_service_page_get_email_address`. It returns the address
//! typed on the assistant's identity page, the entire input to the account
//! a JMAP setup starts from. The page itself is an opaque handle
//! (`EVO_HANDLES`), like the GTK classes below: a widget that nothing here
//! subclasses, allocates, or reads a field of.
//!
//! This crate re-exports every GLib, EDS, and Camel type these headers
//! mention from [`eds-sys`], instead of regenerating them. So an `ESource
//! *` on this side of the ABI is the same Rust type as an `ESource *` on
//! that one. `build.rs` states what that blocklist is, and why GTK is the
//! exception.
//!
//! ## The GTK surface
//!
//! One vfunc of that class, `insert_widgets`, receives a `GtkBox` and must
//! put widgets in it. So this crate also generates a short list of GTK
//! calls: a grid, a mnemonic label, an entry, and the calls that arrange
//! them. `build.rs`'s `ALLOWED_GTK_FUNCTIONS` is that list. It grows one
//! line at a time, with the code that calls it.
//!
//! GTK's *types* are still not generated. Each class is an opaque
//! zero-sized handle (`GTK_HANDLES`), distinct per class, so the pointer
//! casts GTK's C API requires are written down explicitly. `tests/gtk.rs`
//! asks the running GTK whether the classes match what this crate calls
//! them, and whether they relate the way those casts assume. No test here
//! can *use* the widgets: GTK 3 will not construct a widget without a
//! display connection. So the page these calls build is only exercisable
//! in a real Evolution session. Only a human running the account assistant
//! exercises it.
//!
//! Like [`eds-sys`], this crate stays out of the workspace's
//! `default-members`. It needs Evolution's development headers, and `cargo
//! test` on a machine without them must still work. CMake runs its tests
//! through the `rust-test-eds` target.
//!
//! [`eds-sys`]: ../eds_sys/index.html
//! [`jmap-config`]: ../jmap_config/index.html

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// As in eds-sys, bindgen's output is not this crate's to lint.
#![allow(clippy::all)]
#![allow(rustdoc::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
