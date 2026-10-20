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

## `src/manage.rs`

- `crates/jmap-mail/src/manage.rs:150:5`, `:171:5`, `:201:5`
  (`create_folder`, `delete_folder`, `rename_folder`), all `replace ...
  with Ok(Default::default())`. Does not compile: `FolderInfo`
  (`jmap-mail-sync`'s `folder.rs`) derives `Debug, Clone, PartialEq, Eq`
  but not `Default`, the same pattern as this crate's other
  `Ok(Default::default())`/`Default::default()` mutants. Confirmed with
  `cargo check -p jmap-mail`.

## `src/transport.rs`

Found scoped to `tests/transport.rs` alone, then re-checked against
`tests/transport.rs` plus `tests/transport_stale_token.rs` together (the
file this crate's convention usually runs mutants against is only the
same-named test file, but `transport_stale_token.rs` exercises
`refresh_credentials`/`inspect_connection` directly and the first pass's
missed list was shorter once it was included). `replace
JmapTransport::refresh_credentials -> bool with true` and `replace
JmapTransport::inspect_connection -> Option<R> with None` were caught once
`transport_stale_token.rs` ran; record this for whoever sweeps a file next
whose behaviour a sibling `tests/*_stale_token.rs` or `tests/send_retry.rs`
file also drives, so the mutants run is not scoped to the one file that
shares its name.

Seven mutants stay genuinely out of scope, all for one reason this crate
already states twice: the OAuth 2.0 refresh and the `Connected` short-circuit
only do anything once Camel has a real `CamelService`/`CamelSession`, which
this crate's test doubles never build.

- `crates/jmap-mail/src/transport.rs:268:9` (`service`), `replace ... with
  None` and `replace ... with Some(Default::default())`. Every test that
  reaches `refresh_credentials` uses `JmapTransport::detached()`
  (`tests/transport_stale_token.rs`), whose real `service()` already answers
  `None` the same way the first mutant does — there is no `CamelService` to
  check the type of. A `common::Transport` instance does have one, but
  nothing downstream of `service()` tells the two cases apart without
  reaching a configured OAuth 2.0 account's token fetch (see the next
  entries), so a test built only to make `service()` return `Some` would
  prove nothing `is_connected`/`drop_connection`'s own tests do not already
  cover.
- `crates/jmap-mail/src/transport.rs:285:9` (`refresh_credentials`), `replace
  ... with false` (`replace ... with true` is caught by
  `transport_stale_token.rs`'s
  `a_stale_bearer_token_is_reported_once_when_there_is_nothing_to_refresh_from`).
  Every path this crate can drive without a real `CamelSession` already
  returns `false` for real — no service, or a service whose settings do not
  name OAuth 2.0 — so a mutant that forces `false` changes nothing
  observable. Reaching the branch that would return `true` needs
  `camel_session_get_oauth2_access_token_sync` to succeed, and that vfunc
  slot is unimplemented on the plain `CamelSession` `tests/common` builds;
  `tests/oauth2.rs`'s own module doc already states this exact limitation
  for `oauth2::access_token`: only `EMailSession` fills the slot in, and
  that is Evolution's own subclass, covered on the live-EDS side instead.
- `crates/jmap-mail/src/transport.rs:295:12`, `delete !` (inverts `if
  !uses_oauth2 { return false }`). Reaching this line at all needs a real
  `service()` (see above); once reached, both the real code and the mutant
  fall through to the same `camel_service_ref_session` call and the same
  `oauth2::access_token` failure for the reason just given, so both answer
  `false` and a test cannot tell them apart without the live `EMailSession`
  neither this crate nor `jmap-functional` builds for the transport's send
  path today (`jmap-functional`'s own stale-token coverage,
  `mail-stale-token.rs`, is the store's fetch path, not a send).
- `crates/jmap-mail/src/transport.rs:381:9` (`<impl Connected for
  JmapTransport>::release_connection`), `replace ... with ()`. Only called
  from `jmap_mail::service::disconnect_sync`, Camel's own `disconnect_sync`
  vfunc body; every test for this file calls the inherent
  `JmapTransport::drop_connection` directly rather than dispatching that
  vfunc, because dispatching it for real needs the same `CamelSession` this
  file's other entries do not have. `tests/service.rs`'s own module doc
  states the identical limitation for the store's side of the same vfunc
  pair: "what is testable without a `CamelSession`" stops at `authenticate`
  and `report_authentication`, not `connect_sync`/`disconnect_sync`
  themselves.
- `crates/jmap-mail/src/transport.rs:385:9` (`<impl Connected for
  JmapTransport>::holds_connection`), `replace ... with true` and `replace
  ... with false`. Same as `release_connection`: only read from
  `jmap_mail::service::connect_sync`'s own vfunc body (the short-circuit that
  skips reconnecting an already-live service), which no test here dispatches
  for the same `CamelSession` reason.

None of the seven need a `mutants.out`/disk note beyond this file; re-check
once `jmap-functional` grows a real-`EMailSession` proof for the transport's
send-retry path (there is one for the store's fetch path,
`mail-stale-token.rs`, and none yet for a send), the same way the
`eds_oauth2_dynamic` entry above is re-checked once that EDS symbol exists.

## `src/connect.rs`

- `crates/jmap-mail/src/connect.rs:204:9`, `:213:9`, `:241:9` (the three
  `From` impls for `StoreError`), `:341:9` (`reclassify_oauth2_rejection`),
  `:460:5` (`password_credentials`), `:473:5` (`bearer_credentials`), and
  `:499:5` (`open_mail`), all `replace ... with Default::default()` (or
  `Ok(Default::default())`). Does not compile: neither `StoreError` nor
  `jmap_client::Credentials` nor `jmap_mail_sync::MailSync` derives
  `Default`, the same recurring pattern as this crate's other Default-mutant
  entries. Confirmed with `cargo check -p jmap-mail` against each mutated
  source.

## `src/search_sexp.rs`

- `crates/jmap-mail/src/search_sexp.rs:93:9`, delete match arm `')' => None,`
  in `parse_value` (falls through to `_ => Some(parse_token(input))`).
  `parse_value` only ever sees a leading `)` from one call site:
  `translate`'s own top-level call on the raw input. (`parse_list`'s loop
  checks `input.strip_prefix(')')` itself before ever calling `parse_value`,
  so a list body never passes one through.) At that one call site,
  `parse_token` on a string starting with `)` stops immediately (`)` is one
  of its own stop characters) and returns an empty `Ident` with the input
  unconsumed, so `translate`'s own trailing-garbage check
  (`rest.trim().is_empty()`) rejects it exactly as the removed arm would
  have. Verified empirically: patched the arm out and ran a battery of
  probes (`")"`, `") garbage"`, `"(and )"`, `"(and ))"`, `"(and () )"`,
  `"(and (body-contains \"a\") )"`, `"(not )"`) plus the crate's own test
  suite; every result was identical with the arm present or removed.
- `crates/jmap-mail/src/search_sexp.rs:105:40`, `replace + with * in
  skip_ignorable` (`&rest[pos + 1..]` becomes `&rest[pos * 1..]`, i.e.
  `&rest[pos..]`). `pos` is the index of the `\n` ending a `;` comment, so
  the correct `pos + 1` skips past it and the mutant's `pos` stops one
  character short, right at the newline itself. That newline is always
  whitespace, and the very next loop iteration starts by calling
  `input.trim_start()`, which strips it (and anything after it) unasked;
  the one-character difference never survives past that line, for any
  comment body. Verified empirically: patched the mutant in, confirmed the
  crate's full test suite (including the two comment tests added for the
  neighbouring `- ` mutant below) stays green.
- `crates/jmap-mail/src/search_sexp.rs:162:55`, `replace || with && in
  parse_token` (`c.is_whitespace() || c == '(' || c == ')'` becomes
  `c.is_whitespace() || c == '(' && c == ')'`, and `&&` binds tighter, so
  the second disjunct is always false: a token now ends only at
  whitespace). Every legitimate argument in this grammar is a quoted
  string (parsed by `parse_string`, untouched by this mutant); a bare
  token touching `)` with no separating space is therefore always either
  a function name with zero required args (already rejected by an
  explicit `args.is_empty()`/`[x] = args` check) or garbage that fails
  `as_str`'s `Str`-only match, and when the swallowed `)` instead
  mis-closes an enclosing list, the list that should have closed last runs
  out of input first, which `parse_list` already turns into `None`. Every
  path converges on the same `None` the un-mutated parser gives, so no
  input distinguishes them. Verified empirically: patched the mutant in
  and diffed `translate` on a battery of touching-paren cases (bare
  function names, nested `and`/`or`/`not`/`match-all`, siblings) against
  the un-mutated parser; every result matched.
