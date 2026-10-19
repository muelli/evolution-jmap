<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-config`

## `src/defaults.rs`

`from_identity`'s body survives replaced with `Default::default()` only as
an `Unviable` mutant: `Account` has no `Default` impl, so it does not
compile. No action needed.

## `src/complete.rs`

`<impl std::error::Error for Incomplete>::source`'s body survives replaced
with `Some(Box::leak(Box::new(Default::default())))` only as an `Unviable`
mutant: the boxed value has no concrete type to default-construct, so it
does not compile. No action needed.

## `src/oauth2_service.rs`

Five mutants, all `Unviable` (none compile), no action needed:
- `<impl ObjectSubclass for Service>::interfaces`'s body replaced with
  `vec![Default::default()]`: `InterfaceDecl` has no `Default` impl.
- `Verifiers::redeem`'s body replaced with `Some(Zeroizing::new())`,
  `Some(Zeroizing::from_iter([String::new()]))`,
  `Some(Zeroizing::from_iter(["xyzzy".into()]))` or
  `Some(Zeroizing::from("xyzzy".into()))`: each is a type or arity mismatch
  against `Zeroizing<String>`.

## `src/config_lookup.rs`

Three mutants, all `Unviable` (none compile), no action needed:
- `probe_host`'s `&&` (line 178, the `if let Some(servers) = servers && let
  Some(first) = ...` let chain) replaced with `||`: `||` is not a supported
  operator in a let-chain condition at all, a hard compiler error, not a
  logic change to test against.
- `parse_target`'s body replaced with `Some(Default::default())`: `Target`
  has no `Default` impl.
- `<impl ObjectSubclass for JmapConfigLookup>::interfaces`'s body replaced
  with `vec![Default::default()]`: `InterfaceDecl` has no `Default` impl.

## `src/oauth2.rs`

`crates/jmap-config/src/oauth2.rs:450:5` (`ensure_registered`'s
`#[cfg(eds_oauth2_dynamic)]` arm): the body survives replaced with `()`,
reported MISSED rather than Unviable. Spurious for the same reason as
`jmap-mail/src/sasl.rs`'s `sasl_type`/`auth_type` entries: `eds_oauth2_dynamic`
needs `e_source_oauth2_client_get_type`, upstream `Since: 3.64`, unreleased,
so no EDS this project builds against today sets it. Confirmed with
`cargo build -p jmap-config -vv`: no `--cfg eds_oauth2_dynamic` reaches
rustc here. The `#[cfg(not(eds_oauth2_dynamic))]` arm a few lines above
(`register_static::<Extension>()`) is what every binary built today
actually runs, and its own mutant is caught. cargo-mutants enumerates
mutation sites from the source text without evaluating `#[cfg]`, so it
still generates a mutant for the inactive arm; since that code is not
compiled at all, the mutation changes nothing observable and the mutant
always reports MISSED, regardless of test coverage. Not fixable by a test
until an EDS exposing the marker symbol is the one being built against;
re-check then.
