<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-mail`

## `src/changes.rs`

- `crates/jmap-mail/src/changes.rs:159:9`: `<impl Drop for Changes>::drop`
  survives replaced with `()`. The body frees the owned
  `CamelFolderChangeInfo` with `camel_folder_change_info_free`; under the
  mutant, a `Changes` dropped without being handed to
  `camel_folder_changed` (an early return, same as `jmap-backend-core`'s
  `mime.rs` `Unwritable::drop`) would leak that allocation instead of
  freeing it. No test can tell the two apart: `camel_folder_change_info_free`
  deallocates memory and has no other observable effect, and this project
  runs no sanitizer or leak checker (`ci/checks.sh` has no ASAN or valgrind
  pass). Pinning this needs memory instrumentation this project does not
  have, not a better test of this file's logic.

## `src/folder_info.rs`

- `crates/jmap-mail/src/folder_info.rs:183:60`, `184:62`, `185:58`, `186:60`,
  `187:58`, `188:64`: replace `|` with `^` in `flags`, one per
  `CAMEL_FOLDER_TYPE_* | CAMEL_FOLDER_SYSTEM` match arm. `CAMEL_FOLDER_SYSTEM`
  is `1 << 6` and every `CAMEL_FOLDER_TYPE_*` value is `n << CAMEL_FOLDER_TYPE_BIT`
  with `CAMEL_FOLDER_TYPE_BIT == 10` (`camel-enums.h`), so the two operands
  never share a set bit. For disjoint operands `|` and `^` are the same
  operation, and `flags` starts the match fresh each call (not OR-ed onto a
  pre-existing value), so the two forms compute identical results for every
  role.

## `src/provider.rs`

- `crates/jmap-mail/src/provider.rs:88:5` through `:91:5` (`FLAGS`) and
  `:102:5` through `:106:5` (`URL_FLAGS`), all `replace | with ^`, 9 mutants.
  Both constants are plain `|`-chains of `CamelProviderFlags`/
  `CamelProviderURLFlags` members, and every member in each chain is a
  distinct power of two (`camel-enums.h`: `CAMEL_PROVIDER_IS_REMOTE` through
  `SUPPORTS_SSL` are `1 << 0` to `1 << 5`; `camel-provider.h`:
  `CAMEL_URL_ALLOW_USER` through `CAMEL_URL_NEED_HOST` are built from
  `CAMEL_URL_PART_*` values `1 << 0` to `1 << 6`, shifted for `NEED`). Rust's
  `&` binds tighter than `|`, so replacing one `|` with `^` in an
  unparenthesised chain only changes how that one adjacent pair combines; for
  two disjoint bits `^` and `|` compute the same value. Every mutant in this
  pair is therefore equivalent code, not a test gap.
  Checked by hand against `tests/provider.rs`'s field assertions, then
  confirmed by running `cargo mutants` to completion: 9 missed (exactly these
  9), 9 caught, 1 unviable.
- `crates/jmap-mail/src/provider.rs:126:5` (`register`), `replace register ->
  &'static CamelProvider with Box::leak(Box::new(Default::default()))`. Does
  not compile: `CamelProvider` (from `eds-sys`'s bindgen output) has no
  `Default` impl, same pattern as `jmap-config`'s other `Default::default()`
  mutants. Confirmed with `cargo check -p jmap-mail` against the mutated
  source.
  Note for future files with a similar `|`-chain of disjoint flag constants:
  the same `&` binds tighter than `|` rule means a `replace | with &` mutant
  is *not* automatically caught just because the chain's first or last
  member has its own test; it only zeroes the two operands adjacent to the
  mutated operator, so a field that is covered only in combination with
  neighbours further down the chain can still be missed. `URL_FLAGS`' first
  run here (before `tests/provider.rs` gained
  `the_provider_allows_the_url_parts_jmap_uses`) missed four `&` mutants at
  `:103:5` to `:106:5` for exactly this reason: `the_provider_needs_a_host`
  checked only the `NEED_HOST` bit, which sits in the first pair and was
  never the one zeroed by a mutation further along the chain.

## `src/settings.rs`

- `crates/jmap-mail/src/settings.rs:126:9` (`<impl ObjectSubclass for
  JmapSettings>::interfaces`), `replace ... with
  vec![Default::default()]`. Does not compile: `InterfaceDecl` has no
  `Default` impl, the same pattern as `jmap-config`'s `oauth2_service.rs`
  and `config_lookup.rs` `interfaces` mutants. Confirmed with `cargo check
  -p jmap-mail` against the mutated source.

## `src/sasl.rs`

- `crates/jmap-mail/src/sasl.rs:283:5` (`sasl_type`) and `:353:5`
  (`auth_type`), both `replace ... with Default::default()`, in the
  `#[cfg(eds_oauth2_dynamic)]` arm. Not behaviourally equivalent code: a real
  build of this arm with `auth_type` returning NULL would hand the account
  editor's combo a NULL `CamelServiceAuthType` (see `crate::provider`'s
  `authtypes` list). The mutants are spurious for a different reason:
  `eds_oauth2_dynamic` needs `e_source_oauth2_client_get_type`, upstream
  `Since: 3.64`, unreleased (`docs/eds-version-matrix.md`'s "OAuth2Dynamic"
  section measured it only against a from-source EDS master build, never
  against this VM's installed 3.52.3 or CI's pinned 3.60.2 Fedora image).
  Confirmed with `cargo build -p jmap-mail -vv`: no `--cfg eds_oauth2_dynamic`
  reaches rustc here, so the `#[cfg(not(eds_oauth2_dynamic))]` arm a few lines
  above is what actually ships in every binary this provider builds today,
  and it has its own `sasl_type`/`auth_type` (lines 265/318), both caught.
  cargo-mutants enumerates mutation sites from the source text without
  evaluating `#[cfg]`, so it still generates mutants for the inactive arm;
  since that code is not compiled at all, the mutation changes nothing
  observable and the mutant always reports MISSED, regardless of test
  coverage. Nothing here is fixable by a test until an EDS exposing the
  marker symbol is the one being built against; re-check then.
