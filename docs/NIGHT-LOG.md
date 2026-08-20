# Night log

Running record of the autonomous work sessions: what was done, what was
decided and why, what is blocked. Newest entries at the bottom. Older entries
(before 2026-08-20) are rotated to `docs/NIGHT-LOG-archive.md`, which is history
only and NOT read during sessions — grep it if you need past context.

## 2026-08-20 (claim) — Claiming CURRENT PRIORITY item 8: CI RED, lintian-clean .deb regressed by the RUNPATH fix

Fresh survey (`git fetch`: `origin/master` unchanged at `174ea73`, the agy
lane's vCard 3.0 fidelity batch 2 merge — `[agy]`-lane polish, not this
lane's work, and CURRENT PRIORITY's own no-reopen directive keeps that
backend closed). Confirmed via the GitHub Actions API
(`api.github.com/repos/.../actions/runs`) that CI's `build` job has failed on
every run since `e21f97d` (2026-08-19), all failing at the `Run ci/build.sh`
step — consistent with item 8's own diagnosis.

**Why this and not something else:** the last several sessions' surveys
(logged immediately above, ending "NIGHT-SHIFT: BLOCKED") explicitly scoped
themselves to "CURRENT PRIORITY items 1-6" and Round 2 Tracks A-F — item 8
(added by `553d6a2`, the same day) and item 7 (added by `e892ffa`) were never
walked by that survey and carry no DONE marker or claim/delivery pair
anywhere in this log. Item 8 is HIGH priority ("unblocks CI"), concrete,
tool-verifiable, no FFI, no maintainer/operator step needed — reproduced
locally in under a minute (`ctest --test-dir build -R lintian`):

    E: evolution-jmap: custom-library-search-path RUNPATH /usr/lib
    [usr/lib/evolution/modules/module-jmap-configuration.so]
    E: evolution-jmap: custom-library-search-path RUNPATH /usr/lib/evolution
    [usr/lib/evolution/modules/module-jmap-configuration.so]
    E: evolution-jmap: custom-library-search-path RUNPATH
    /usr/lib/x86_64-linux-gnu
    [usr/lib/evolution/modules/module-jmap-configuration.so]

Item 7 (collection-backend discovery + auth-retry loop) needs a real
Fastmail API-token account in real Evolution to reproduce or verify at all —
not headlessly tractable this session — so item 8 is the one to claim.

**Root cause traced past what item 8's own text says.** `readelf -d` on the
built `module-jmap-configuration.so` shows all three paths in one RUNPATH:
`/usr/lib/evolution:/usr/lib/x86_64-linux-gnu:/usr/lib`. Only the first is
the deliberate one item 2(a)/`ac00396` added (Evolution's own `privlibdir`,
confirmed via `pkg-config --variable=privlibdir evolution-shell-3.0`). The
other two are *not* deliberate: `evo-sys/build.rs` (the shared source both
`jmap-config` and `jmap-config-module`'s own `build.rs` read
`DEP_EVOLUTION_SHELL_LIBDIRS` from) builds its published `cargo:libdirs` from
`pkg_config::Library::link_paths` (every `-L` search directory pkg-config's
recursive `Requires:` resolution turns up for `evolution-shell-3.0` +
`evolution-mail-3.0` — confirmed with `pkg-config --libs-only-L`, which
pulls in dependencies' own libdirs), not from the `-Wl,-R` flags the two
`.pc` files actually declare (confirmed both files verbatim: each has
exactly one `-Wl,-R${privlibdir}`, nothing else). The `pkg_config` crate
exposes exactly that narrower list separately as `Library::ld_args`
("Linker options specified by -Wl"), which a probe of both packages shows
contains only `-R/usr/lib/evolution` — nothing else. So the extra two
RUNPATH entries are incidental noise from using the wrong field, not
anything -Wl,-R-declared or deliberate; narrowing to `ld_args` removes them
at the source rather than merely overriding lintian about them, and doesn't
touch the one deliberate entry `ac00396` was for.

**Claiming:** change `evo-sys/build.rs`'s `cargo:libdirs` construction to
read `-R<dir>`/`-rpath[=,]<dir>` directories out of `Library::ld_args`
instead of blanket `Library::link_paths`, verify with `readelf -d` that the
built module's RUNPATH narrows to exactly `/usr/lib/evolution`, and confirm
`ctest --test-dir build -R lintian` goes green. Scoped to `evo-sys/build.rs`
only; `jmap-config`'s and `jmap-config-module`'s own `build.rs` files already
just forward whatever `DEP_EVOLUTION_SHELL_LIBDIRS` says, unchanged. Full
cargo fmt/clippy/test gate (default-members + the seven EDS-gated crates)
before pushing, per the standing rule.

## 2026-08-20 — Delivered: CURRENT PRIORITY item 8, CI red fixed (lintian-clean .deb restored)

Delivered the increment claimed above. Two parts: narrow the RUNPATH at the
source (removing two incidental entries), then a scoped, justified lintian
override for the one entry that is genuinely deliberate and still flagged.

**Narrowing.** `evo-sys/build.rs`'s `cargo:libdirs` (the metadata
`jmap-config`'s and `jmap-config-module`'s own `build.rs` files turn into
`-Wl,-rpath` for the binaries/module they build) was built from
`pkg_config::Library::link_paths` — every `-L` search directory pkg-config's
recursive `Requires:` resolution turns up for `evolution-shell-3.0` +
`evolution-mail-3.0` (confirmed with `pkg-config --libs-only-L`, which pulls
in a chain of dependencies' own libdirs, several of them standard system
paths). Read both `.pc` files verbatim (`evolution-shell-3.0.pc`,
`evolution-mail-3.0.pc`): each carries exactly one `-Wl,-R${privlibdir}` in
its `Libs:` line and nothing else — so only `/usr/lib/evolution` was ever
actually asked for, and the `pkg_config` crate exposes exactly that narrower
list separately as `Library::ld_args` ("Linker options specified by -Wl").
Changed the loop to extract `-R<dir>`/`-rpath[=,]<dir>` directories from
`ld_args` instead of taking every `link_paths` entry. Confirmed with
`readelf -d` on the freshly built `libjmap_config_module.so`: `RUNPATH`
narrowed from `/usr/lib/evolution:/usr/lib/x86_64-linux-gnu:/usr/lib` to
exactly `/usr/lib/evolution` — the one directory item 2(a)/`ac00396` actually
needed, unchanged.

**The override.** `/usr/lib/evolution` is still flagged by lintian's
`custom-library-search-path` even after narrowing — it is genuinely a custom
search path (Evolution's own private libdir, not this package's), and
lintian's exemption for a package's *own* private dir only matches
`/usr/lib/<source-package-name>`, which this is not. This is exactly the
tag item 8's own text, and `cmake/tests/check-deb-lintian.cmake`'s own
comment, said belongs in an override file argued with a comment, not
suppressed by deleting the RUNPATH the Look-Up worker needs to load (item
2(a)). Added `docs/packaging/lintian-overrides` (one justified override
line, Debian's package-named-file convention), installed to
`/usr/share/lintian/overrides/${PACKAGE_NAME}` by `cmake/Packaging.cmake`
(same `config-module` component the changelog/copyright already install
from), and added to `EXPECTED_PACKAGE_FILES` so `cmake/tests/
check-deb-package.cmake`'s exact-file-list check stays accurate. Read
lintian's own override-matching code
(`/usr/share/lintian/lib/Lintian/Group.pm`, `Processable/Overrides.pm`) to
confirm the file format and that the bracketed context after the tag name
is matched by an exact string, not a partial one — the override line names
the file path verbatim, no glob needed since only one binary carries this
RUNPATH.

**Verified, not just argued.** `ctest --test-dir build -R "package-deb"`:
all three packaging tests (`package-deb`, `package-deb-reproducible`,
`package-deb-lintian`) green — `package-deb-lintian` was the reproduction
target and is the primary proof. Full suite `ctest --test-dir build`: 18/18
green, including `rust-test-eds` and all five `functional-*` tests. Full
cargo gate: `cargo fmt --check` clean; `cargo clippy --all-targets --locked
-- -D warnings` (default-members) clean; `cargo clippy -p
evolution-jmap-client -p jmap-backend-core -p jmap-backend-book -p
jmap-backend-cal -p jmap-mail -p jmap-backend-collection -p jmap-config -p
jmap-config-module -p evo-sys --all-targets --locked -- -D warnings` (the
seven EDS-gated crates plus the two crates this change actually touches)
clean; `cargo test --locked` (default-members) green, 0 failed; the same
nine-crate `cargo test --locked` green, 0 failed. Disk filled mid-session on
that nine-crate run (`rust/target/debug` again hit capacity, "No space left
on device" mid-link) — `cargo clean --profile dev` recovered 24.3GiB and the
rerun was clean, the same standing issue prior sessions have logged
([[disk-fills-from-cargo-target]]).

**Why this and not item 7.** Item 7 (collection-backend discovery +
auth-retry loop against a real Fastmail account) needs a real API-token
account in real Evolution to even reproduce, let alone verify — not
headlessly tractable this session, unlike item 8, which reproduced locally
in under a minute and gated fully offline.

**Confirmed the actual CI state, not just the local repro.** Before
claiming, checked the GitHub Actions API directly
(`api.github.com/repos/muelli/evolution-jmap/actions/runs`): the `build` job
had failed on every run back to `e21f97d` (2026-08-19), each at the `Run
ci/build.sh` step — consistent with the `package-deb-lintian` ctest being
what `ci/build.sh` runs and what failed locally. Cannot re-trigger CI from
this session to confirm the fix goes green there too (no `gh`/token per the
standing note, [[github-actions-status-without-gh]]) — pushing this and
letting the next scheduled/triggered run prove it is the closing step.

**Why the last several sessions missed this.** Every survey since
`553d6a2` (which added item 8) explicitly scoped itself to "CURRENT PRIORITY
items 1-6" and Round 2 Tracks A-F, and none of those surveys' own text ever
mentions items 7 or 8 — a real gap in scope, not a re-confirmation of an
already-known block. `docs/ROADMAP.md`'s item 8 entry updated in place with
a `DONE` sub-bullet in the same style items 2(a)/2(b)/5/6 already use, so a
future survey does not re-tread this.

No new dependency; no new user-facing string; both new files
(`docs/packaging/lintian-overrides`, and the `evo-sys/build.rs` edit) fall
under REUSE.toml's existing `docs/**` and per-crate SPDX-header coverage
respectively — no REUSE.toml change needed. `ci/checks.sh` still cannot run
on this VM ([[checks-sh-blocked-on-vm]]).

NIGHT-SHIFT: item 8 (CI red) delivered and pushed. Ending the session here
per the standing rule against starting a second large item — item 7 is the
next candidate but needs a real Fastmail API-token account in real
Evolution, not headlessly tractable this session.

## 2026-08-20 (claim) — Claiming CURRENT PRIORITY item 7's likely root cause: the collection backend's `authenticate.rs` never sends API-token accounts as Bearer

Fresh survey: `git fetch` shows `origin/master` unchanged at `e14d991`
(item 8, previous session). CURRENT PRIORITY items 1/2/3/4/5/6/8 are
code-complete (several pending only operator confirmation already logged as
such); Round 2 Tracks A/C1/C3/D/Track E Phase 0+Path A are done pending
operator verification or a maintainer decision on the remaining
sub-items (Track B1/C2's third-party half/C4/D2 write-back/Track E Phase
B-C); Track F is closed. That leaves item 7 as the only CURRENT PRIORITY
item with headless code work left, and the roadmap's own text names a
concrete lead to chase rather than a diagnosis to redo: "Diff the two
connect paths" between the collection backend's `authenticate_sync` and
the working book/cal/mail backends' `connect_sync`.

Did that diff. `jmap-backend-core/src/connect.rs::connect_with` (what
`connect_sync` uses for the address book, calendar and mail backends) picks
credentials with three branches: `source_uses_oauth2` → OAuth2 bearer,
`source_uses_api_token` → `bearer_credentials` (the item-6 API-token
method), else Basic. `jmap-backend-collection/src/authenticate.rs::login_of`
(what `authenticate_sync` uses for the collection backend — the vfunc that
gates the whole fan-out, i.e. contacts/calendar child creation) has only
**two** branches: `source_uses_oauth2`, else plain Basic
(`jmap_backend_core::connect::credentials`) — `source_uses_api_token` is
never checked here at all, even though `jmap-backend-core::api_token` (item
6) already exists and is already imported by `connect.rs` right next to it.

This is a complete, headless explanation for both halves of item 7: an
account using the "API Token" method (Fastmail, per item 6) has its stored
token sent to the collection backend's `authenticate_sync` as a **Basic**
password (`Credentials::basic(user, token)`), which a real Bearer-only JMAP
endpoint 401s. `ConnectError::auth_result`'s existing rule turns a 401 into
`REJECTED`, which makes EDS discard the "password" and re-prompt — the
~6-second auth-retry loop — and since `fan_out` is never reached, contacts
and calendar children are never created. The book/cal/mail backends on the
*same* account authenticate fine because their `connect_sync` already goes
through the three-branch `connect_with`; only the collection backend's
separate `login_of` was left on the old two-branch shape from before item 6
existed — item 6's own roadmap text scoped itself to "the connect path" and
`jmap-mail`'s Camel side, and never named `jmap-backend-collection`'s
`authenticate.rs` as a third site needing the same branch, so it was missed
then rather than regressed since.

**Claiming:** add the missing `source_uses_api_token` → `bearer_credentials`
branch to `login_of`, mirroring `connect_with`'s exact three-way shape. Red
test first in `jmap-backend-collection/tests/authenticate.rs` (a
`TestSource::api_token()` builder alongside the existing `.oauth2()`,
asserting the fan-out receives `Credentials::Bearer` rather than `Basic` for
a stored token) against the current two-branch code, then green. This is a
pure, mechanical, same-crate-family port of an existing, already-tested
pattern — not new design — so it does not need the escalation the rest of
item 7 (tracing a live `evolution-source-registry`) was flagged for.
**Scope, stated plainly:** this closes the code-side root cause the roadmap
asked to find; it does not by itself prove the fix against a real Fastmail
account in real Evolution (still the operator's step, unchanged from item
7's own text) — but it is a positive, testable finding rather than another
"needs the operator" report, and worth landing regardless of what the
operator's own trace eventually shows.

## 2026-08-20 — Delivered: CURRENT PRIORITY item 7's code-side root cause (collection backend now sends API-token accounts as Bearer)

Delivered the increment claimed above. `jmap-backend-collection/src/
authenticate.rs::login_of` gained the missing third branch:
`source_uses_api_token(source)` → `bearer_credentials(password)`, imported
from `jmap_backend_core::api_token`/`connect` exactly as `connect_with`
already imports and uses them for the address book, calendar and mail
backends. Placed between the existing `source_uses_oauth2` branch and the
plain-Basic fallback, matching `connect_with`'s order verbatim.

**TDD.** `jmap-backend-collection/tests/authenticate.rs` gained a
`TestSource::api_token()` builder (`e_source_authentication_set_method` to
`jmap_backend_core::api_token::API_TOKEN_METHOD`, mirroring the file's
existing `.oauth2()`) and two tests:
`an_api_token_account_is_sent_as_bearer_not_basic` (an account with the
API-token method and a stored secret must reach the fan-out as
`Credentials::Bearer`, never `Basic`) and
`an_api_token_account_with_no_stored_token_asks_for_one` (mirrors the
existing no-password case, `REQUIRED` not a silent empty-Bearer fan-out).
Ran red first against the unmodified two-branch code:
`an_api_token_account_is_sent_as_bearer_not_basic` failed with `Some(Basic
{ user: "vera@example.com", password: "t0k3n" })` — the exact bug the
claim entry diagnosed, not a guess. After the one-branch fix, both new
tests and the file's existing 13 pass, 15/15.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked
-- -D warnings` (default-members) and `cargo clippy -p
evolution-jmap-client -p jmap-backend-core -p jmap-backend-book -p
jmap-backend-cal -p jmap-mail -p jmap-backend-collection -p jmap-config
--all-targets --locked -- -D warnings` (the seven EDS-gated crates) both
clean. `cargo test --locked` (default-members) and the same seven-crate
`cargo test --locked` both green, 0 failed throughout (checked every
`test result:` line, not just exit code). No new dependency; no new
user-facing string; no new file (no REUSE/SPDX concern).

**Why this is not the same shape as the rest of item 7.** The root cause
was findable and fixable by reading code and diffing two same-crate-family
functions — exactly what the roadmap's own text suggested — with no live
`evolution-source-registry`, no Fastmail account, and no GObject-vtable
design question involved (`bearer_credentials`/`source_uses_api_token`
already existed, tested, from item 6; this was a call site that missed
them, not a new abstraction). That is why it did not need the escalation
item 7's text flagged. What is genuinely still open, and still needs the
operator: whether this is the *only* cause of the auth-retry loop and the
missing book/cal children, or whether the operator's live trace turns up
something else alongside it. `docs/ROADMAP.md`'s item 7 updated in place
with a `DONE (code side; pending operator verification)` sub-bullet,
following items 1/2/5/6's own established pattern, rather than tagging
item 7 complete.

`ci/checks.sh` still cannot run on this VM
([[checks-sh-blocked-on-vm]]).

NIGHT-SHIFT: item 7's code-side root cause delivered and pushed. Ending
the session here per the standing rule against starting a second large
item.

## 2026-08-20 (claim) — Claiming FFI-SOUNDNESS-AUDIT Finding 1: `set_raw_gerror`'s overwrite-on-violation is a `debug_assert` only

Fresh survey (`git fetch`: `origin/master` unchanged at `d318dc2`, the
previous session's item-7 delivery). Walked `docs/ROADMAP.md` end to end:
CURRENT PRIORITY items 1–8 are all code-complete (several pending only
operator/maintainer confirmation, already logged as such); Round 2 Track A
(A1–A7) is fully closed, including every IMPROVE pattern in
`docs/UNSAFE-AUDIT.md` except one that itself needs a behaviour decision
(`SourceConfig::from_source`'s unguarded extension reads) and is not
mechanical; Track B1/C2's third-party half/C4 remain NEEDS-DECISION; Track D1
is DONE pending operator verification, D2's write-back is explicitly "NOT
CLAIMABLE YET" (needs a signal-lifecycle/concurrency design first); Track E
Path A is DONE pending operator verification, Phase B/C need a fresh
maintainer decision before starting; Track F is closed. M7/M9/M10 are all
COMPLETE per `docs/MILESTONES.md`. No new unblocked, no-decision-needed,
non-backend-polish milestone/track item exists beyond what is already
claimed or gated.

`docs/FFI-SOUNDNESS-AUDIT.md`'s own findings table still lists two
"logged, not fixed" items: Finding 3 (`oauth2.rs::borrowed`'s pointer
lifetime under concurrent `apply()`) is explicitly named as "exactly the
kind of subtle cross-thread pointer-lifetime reasoning the night-shift
escalation criteria name explicitly" — not claiming that one. Finding 1
(`jmap-backend-core/src/error.rs::set_raw_gerror`) is different in kind: no
concurrency, no design question about *whether* to change behaviour, only
*which* of two well-precedented behaviours to pick, and the audit text
itself calls it "a small, real design choice" rather than escalation-worthy.

**The gap, read from the source (`error.rs:86-95`):** `set_raw_gerror`'s
contract is "`*dest` must already be NULL" (the standard GLib `GError**`
out-parameter contract every EDS vfunc caller obeys), enforced today only by
`debug_assert!`, which compiles out entirely in a release build — so a
future caller that violates it would silently overwrite `*dest` in release,
leaking the `GError` that was already there, with no build catching the bug
short of running the debug-asserted test suite against exactly that call
path. Every call site in the workspace today obeys the contract (confirmed
by the audit; not a live bug), but nothing beyond `debug_assert!` protects
against tomorrow's.

**Fix, following existing precedent in this crate rather than inventing
one:** GLib's own `g_set_error()` family refuses to overwrite a non-NULL
`*error` (`g_return_if_fail (err == NULL || *err == NULL)`), which is a
*runtime* check in ordinary (non-`G_DISABLE_CHECKS`) GLib builds, not a
debug-only one — logs and keeps the first error, dropping the second. This
crate already has the matching idiom for "a caller violated an
un-recoverable-here precondition" in the exact same crate:
`jmap_backend_core::trampoline::log_critical` — "for the cases a vfunc
cannot report any other way… a critical is for 'this cannot happen'" —
already used by `subclass.rs`/`instance.rs` for analogous impossible-callee
states. Applying that same idiom here (log a critical, free the incoming
`error`, leave `*dest` and its existing `GError` untouched) both matches
GLib's own convention and this crate's own, rather than picking a third,
novel behaviour (e.g. free-then-set, which nothing else in the tree does).

**TDD:** red test first in `jmap-backend-core/tests/error.rs` —
`overwriting_an_already_set_gerror_keeps_the_first_and_frees_the_second`
calls `set_gerror` twice into the same out-parameter and asserts the first
message survives unchanged; against the current `debug_assert!`, this panics
the test (confirming the precondition is real and currently only
debug-checked) rather than silently passing, so the red state itself is the
evidence for the finding, not merely a compile failure. Then green after
swapping the `debug_assert!` for the `log_critical`-and-drop path.

Full cargo gate (fmt, clippy `-D warnings` default-members + the seven
EDS-gated crates, `cargo test --locked` both scopes) before pushing, per the
standing rule. No new dependency, no new user-facing string (a `g_critical`
line is developer-facing, not user-facing, per the project's own
"Mark UI strings translatable" directive's own carve-out).

## 2026-08-20 — Delivered: FFI-SOUNDNESS-AUDIT Finding 1 (`set_raw_gerror` no longer overwrites a set `GError` in release builds)

Delivered the increment claimed above. `jmap-backend-core/src/error.rs::
set_raw_gerror` now has three branches instead of two: `dest` NULL (free
`error`, unchanged), `*dest` NULL (write `error`, unchanged), and — new —
`*dest` already set: log a critical via the crate's existing
`trampoline::log_critical` ("this cannot happen" idiom already used by
`subclass.rs`/`instance.rs`) and free the incoming `error`, keeping the
first one in place. Matches GLib's own `g_set_error()` family, which
refuses the same way at runtime, not just in debug builds.

**TDD.** Red first: `jmap-backend-core/tests/error.rs` gained
`overwriting_an_already_set_gerror_keeps_the_first_and_frees_the_second`,
which calls `set_gerror` twice into the same out-parameter and asserts the
first message survives. Run against the unmodified `debug_assert!`, it
panicked at `error.rs:91` ("overwriting an already-set GError") — the red
state doubling as confirmation that the precondition is real and, before
this fix, only checked in a debug build. Green after the fix, with the
expected `evolution-jmap-CRITICAL` log line observed on stderr during the
run, confirming the new path executes and not just that the assertion is
gone.

**Why this one and not Finding 3.** `docs/FFI-SOUNDNESS-AUDIT.md`'s other
open finding (`oauth2.rs::borrowed`'s pointer lifetime under concurrent
`apply()`) is explicitly named in that doc as "exactly the kind of subtle
cross-thread pointer-lifetime reasoning the night-shift escalation criteria
name explicitly" — not attempted here, consistent with that flag. This
finding had no such concurrency dimension: the only open question was which
of two well-precedented behaviours to pick on an already-impossible-today
precondition violation, and this crate already has the matching idiom
(`log_critical`) for exactly that class of "cannot happen, but must not be
UB if it somehow does" case, so the choice was a precedent-following one,
not new design.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked
-- -D warnings` (default-members) and the seven-crate EDS-gated clippy
(`evolution-jmap-client`, `jmap-backend-core`, `jmap-backend-book`,
`jmap-backend-cal`, `jmap-mail`, `jmap-backend-collection`, `jmap-config`)
both clean. `cargo test --locked` (default-members) and the same
seven-crate `cargo test --locked` both green, every `test result: ok`, 0
failed throughout both runs. No new dependency; no new user-facing string
(the added log line is developer-facing, not user-facing, per the
project's own "Mark UI strings translatable" directive's own carve-out for
developer-facing errors); no new file, so no REUSE/SPDX concern.
`docs/FFI-SOUNDNESS-AUDIT.md`'s findings table and Finding 1's own section
updated in place to mark it fixed rather than "logged, not fixed."

`ci/checks.sh` still cannot run on this VM ([[checks-sh-blocked-on-vm]]).

NIGHT-SHIFT: FFI-SOUNDNESS-AUDIT Finding 1 delivered and pushed. Ending the
session here per the standing rule against starting a second large item —
the remaining open item in that audit (Finding 3, `oauth2.rs::borrowed`) is
concurrency/pointer-lifetime design work the audit itself flags as
escalation-worthy, not a Sonnet-sized next step from here.

## 2026-08-20 (escalated to opus) — no tractable Sonnet-sized item; escalating FFI-SOUNDNESS-AUDIT Finding 3

Fresh survey (`git fetch`: `origin/master` unchanged at `3dacd8b`, the
previous session's FFI Finding 1 delivery). Walked `docs/ROADMAP.md` end to
end, including every sub-item's own status text, not just section headers:

- CURRENT PRIORITY items 1-8 are all code-complete, each pending only an
  operator/maintainer confirmation already logged as such (M7, item 5's SRV
  resolver, item 6's API-token method, item 7's collection-backend fix, and
  item 8's CI fix all need a real Evolution/Fastmail session or a live CI
  run, neither available here).
- Round 2 Track A (A1-A7): fully closed. A4's one loose end (fuzzing
  redirect targets beyond the fixed-case test) is explicitly "the one
  narrower thing left... if a future session wants to broaden it", not a
  gap the track's own text asks for.
- Track B1: NEEDS-DECISION (tracing+tracing-journald vs g_log — a
  recommendation exists in the roadmap text but no maintainer sign-off is
  recorded).
- Track C: C1/C3 DONE; C2's remaining half (third-party DEP-5 entries) and
  C4 are both explicitly NEEDS-DECISION.
- Track D: D1 code-complete pending operator verification (both
  create_resource_sync and delete_resource_sync landed). D2's write-back is
  "RESEARCHED, NOT CLAIMABLE YET" — needs a new signal-lifecycle/concurrency
  design (EWS has no server round-trip to mirror), the same kind of
  cross-thread reasoning as the item below, not a mechanical port.
- Track E: Phase 0 + Path A fully code-complete pending operator
  confirmation; Phase B/C explicitly need a fresh maintainer decision before
  starting.
- Track F: closed, no-op per its own spike.
- `docs/UNSAFE-AUDIT.md`: every IMPROVE pattern closed except
  `SourceConfig::from_source`'s unguarded extension reads, which its own
  text says "needs a behaviour decision, not a mechanical port", and Pattern
  E, explicitly deprioritized ("Lowest priority... not scheduled").
- `docs/FFI-SOUNDNESS-AUDIT.md`: Finding 1 fixed last session, Finding 2
  fixed the session before, Finding 4 explicitly "cosmetic; not scheduled".
  Finding 3 (`jmap-config/src/oauth2.rs::borrowed` releasing its mutex
  before returning a raw pointer into the `CString` it was protecting, which
  a concurrent `apply()` can then free out from under an in-flight
  `EOAuth2Service` vtable call) is the one remaining item, and the audit's
  own text calls it out by name: "exactly the kind of subtle cross-thread
  pointer-lifetime reasoning the night-shift escalation criteria name
  explicitly."

No other unblocked, no-decision-needed, non-backend-polish item exists.
Every prior session back through item 8's delivery reached the same
conclusion about Finding 3 (see the "Why this one and not Finding 3" note
two entries above) but stopped short of actually escalating it, instead
ending the session on the item they *did* land. With nothing else left to
land this iteration, Finding 3 is now the best next step by elimination, not
new information: it needs either (a) reading EDS 3.52's own threading
contract closely enough to prove `EOAuth2Service` vtable calls and
`insert_entries`'s `apply()` cannot race on the same source (turning this
into a documented KEEP), or (b) a real fix that changes `borrowed()`'s
return shape (owned copy vs. the current zero-allocation raw pointer) —
either path is exactly the kind of concurrency/pointer-lifetime reasoning
where a plausible-but-wrong answer is likely and would either wrongly wave
off a live race or introduce a subtly broken ownership change. Writing
`claude-opus-5` to `~/.night-shift-escalate` and stopping without claiming
any work — no source changed this session, only this log entry.

`ci/checks.sh` still cannot run on this VM ([[checks-sh-blocked-on-vm]]).

NIGHT-SHIFT: escalating FFI-SOUNDNESS-AUDIT Finding 3 to opus (see
`~/.night-shift-escalate`); no tractable Sonnet-sized item remained this
iteration.

## 2026-08-20 (on opus, per the escalation at `0db0438`) — Claiming: FFI-SOUNDNESS-AUDIT Finding 3 (`oauth2.rs::borrowed`'s pointer lifetime)

Claiming the item the previous session escalated. Scoping read done first,
against the EDS 3.52.3 sources fetched from upstream (the installed version,
`dpkg -l` → `3.52.3-0ubuntu1.2`) rather than inferred — which already
changed the shape of the finding in three ways worth recording before any
code moves:

1. **The racing writer in production is `set_property`, not `apply()`.** The
   audit named `apply()`; nothing outside `jmap-config/tests/backend.rs`
   calls it. `config_lookup::add_result` writes `[JMAP OAuth2]` as
   `EConfigLookupResult` string properties, and `e-source.c`'s
   `source_parse_dbus_data` → `source_load_from_key_file` →
   `g_object_set_property` re-runs every `E_SOURCE_PARAM_SETTING` property
   whenever the registry pushes new source data over D-Bus. So the write
   that frees a handed-out `CString` is EDS's own reload path, on whatever
   thread the `GDBusProxy` notify lands on — more frequent and more
   certainly concurrent than the `apply()` the audit hypothesised, not less.
2. **EDS's threading model does not rule it out — and EDS's own OAuth2
   services avoid the hazard by construction.** `e-oauth2-service.c`'s five
   `const gchar *` wrappers carry no transfer or threading annotation, and
   every use is an immediate `g_strdup` into a form/URI a few instructions
   later. But `e-oauth2-service-google.c` never returns a pointer into
   per-source storage: `eos_google_get_client_id` answers either a
   `static gchar glob_buff[128]` or `eos_google_read_settings`, which caches
   with `g_object_set_data_full` under an `if (!value)` guard — **written
   once and never replaced or freed while the service lives**. So the
   contract EDS's own implementations actually keep is "valid for the
   object's lifetime, never invalidated by a later write", which is
   precisely what this module's `borrowed()` doc *claims* ("stable for as
   long as the extension is") and does not currently deliver.
3. **The `borrowed()` doc's justification is the part that is wrong.** It
   argues this is "the same contract EDS's own extensions keep for their
   string accessors ... with no lock of their own either". EDS's
   `e_source_authentication_get_host` is indeed lock-free, but EDS pairs
   every such getter with a `dup_` variant that takes
   `e_source_extension_property_lock`, and its *OAuth2* vfunc
   implementations use the write-once storage above precisely because a
   vfunc returning `const gchar *` has no `dup_` escape hatch.

**Increment:** adopt EDS's own discipline instead of a lock that cannot
cover the caller's use of the pointer. A field's `CString` is never freed
once written; a write that changes it retires the old value into storage
dropped only at `finalize`, and a write that does *not* change it leaves the
existing allocation (and so the existing pointer) in place — the same
compare-then-skip `source_set_property_from_key_file` already does one frame
up. That makes `borrowed()`'s documented lifetime true as written, with the
zero-allocation shape it exists for intact.

**TDD plan:** red first, two tests — `apply`/`set_property` writing an
unchanged value must return the *same* pointer (deterministic, no UB, fails
today because both rebuild the `CString` unconditionally), and a pointer
taken before a *changed* write must still read its original bytes after
allocator churn (fails today by reading freed memory, which is the defect
itself).

## 2026-08-20 — Delivered: FFI-SOUNDNESS-AUDIT Finding 3 (`oauth2.rs::borrowed` no longer hands out a pointer a later write frees)

Delivered the increment claimed above. **The finding was real, and worse
than the audit recorded it: a demonstrated use-after-free, not a MEDIUM-
confidence design question.**

**What the red test showed.** `jmap-config/tests/oauth2.rs` gained three
tests. The decisive one,
`a_pointer_handed_out_before_a_changed_write_still_reads_its_original_bytes`,
takes the `const gchar *` `EOAuth2Service::get_client_id` would return,
performs the changed `set_property` write EDS's own reload path performs,
churns the allocator with same-sized allocations so a reused block shows as
changed bytes rather than passing on an undisturbed one, and reads the
pointer back. Against unmodified `master` it returned
`Some("XXXXXXXXXXXXX")` — the churn's own filler, in the reused heap
block — where `Some("client-abc123")` was expected. That is the
use-after-free, observed rather than argued. The two pointer-stability tests
(`rewriting_a_field_with_the_same_value_keeps_the_pointer_it_handed_out`
through `apply`, and
`setting_a_property_to_the_value_it_already_has_keeps_the_pointer_it_handed_out`
through `g_object_set_property`) each failed with two distinct addresses.
All three green after the fix; all 9 tests in the file pass.

**What the EDS 3.52.3 source read changed** (fetched from upstream at the
installed version, `dpkg -l` → `3.52.3-0ubuntu1.2`; three of the four files
are quoted in the audit doc):

- **The racing writer is `set_property`, not `apply()`.** The audit named
  `apply()`; nothing outside `jmap-config/tests/backend.rs` calls it.
  `e-source.c`'s `source_parse_dbus_data` → `source_load_from_key_file` →
  `g_object_set_property` re-runs every `E_SOURCE_PARAM_SETTING` property
  whenever the registry pushes new source data over D-Bus. So the writer is
  more frequent and more certainly concurrent than hypothesised.
- **No lock can fix it, and EDS shows what does.** `e-oauth2-service.c`'s
  five `const gchar *` wrappers carry no transfer or threading annotation,
  and every EDS use copies the string a few instructions later
  (`e_oauth2_service_util_set_to_form`, `eos_create_soup_message`) holding
  nothing of ours — a vfunc of that signature has no "done with it" hook to
  hang a free or an unlock on. `e-oauth2-service-google.c` is how EDS's own
  implementations cope: `eos_google_get_client_id` answers either a `static
  gchar glob_buff[128]` or a value `eos_google_read_settings` caches via
  `g_object_set_data_full` behind an `if (!value)` guard — **written once,
  never replaced or freed while the service lives.**
- **The old justification's comparison was wrong.**
  `borrowed()`'s doc claimed "the same contract EDS's own extensions keep
  for their string accessors ... with no lock of their own either".
  `e_source_authentication_get_host` is lock-free, but EDS pairs each such
  getter with a `dup_` variant taking `e_source_extension_property_lock` —
  an escape hatch a fixed-signature vfunc does not have, which is exactly
  why EDS's OAuth2 impls use write-once storage instead.

**The fix** adopts that discipline rather than adding a lock that could not
help. `Fields::set` is now the one path both writing doors (`apply` and
`set_property`) go through: an unchanged write is not performed at all,
keeping the existing allocation and so any pointer already handed out of it
(the same compare-then-skip `source_set_property_from_key_file` already does
one frame up), and a changed write moves the replaced value into a
`Fields::retired` vector dropped only in `finalize`. Moving a `CString`
moves the pointer, not the bytes, so retiring and any later vector
reallocation both leave an outstanding `const gchar *` valid. `borrowed()`'s
documented "valid for as long as the extension is" is now true as written,
with the zero-allocation read shape it exists for untouched. Growth traded
for this is bounded by writes that actually *change* a field — an account's
discovered OAuth 2.0 config changing, a human-paced event EDS itself already
declines to re-apply when the value did not differ. A `get(id)` helper
replaced `get_property`'s parallel match, so the five properties are now
enumerated in exactly two places instead of four.

**Why this needed the opus escalation, honestly assessed:** the escalation
was right, but for a different reason than predicted. The hard part was not
concurrency reasoning about a fix — it was that the *cheap* answers were both
wrong and both looked right. Shrinking or extending the mutex's scope cannot
work (the caller's use is outside any lock we could hold), and the "EDS does
this too" argument that had stood in two audits inverted once EDS's actual
OAuth2 implementations were read instead of its plain extension accessors.
A session that reasoned from the header and the existing comment would
plausibly have closed this as a documented KEEP.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked --
-D warnings` (default-members) and the seven-crate EDS-gated clippy both
clean. `cargo test --locked` (default-members) green, 0 failed. The seven
EDS-gated crates run per-crate (the standing multi-package hang):
`evolution-jmap-client` 174, `jmap-backend-core` 115, `jmap-backend-book`
69, `jmap-backend-cal` 131, `jmap-config` 149, `jmap-backend-collection`
159, `jmap-mail` 440 — **1237 passed, 0 failed.** Disk filled at 980 MB
before `jmap-mail`; `cargo clean --profile dev` recovered 23.3 GB, the
standing note again ([[disk-fills-from-cargo-target]]).

**Also ran the packaging leg, per item 8's "standing fix" note** — the check
no night session was watching when CI went red. `ninja -C build` first
([[ninja-before-ctest]]), then the full `ctest --test-dir build`:
**18/18 passed**, including `package-deb-lintian`,
`package-deb-reproducible` and all five functional legs. This change is
packaging-neutral (pure Rust logic in one crate, no build script, no install
rule), so this confirms rather than fixes — but it is the gate item 8 asked
future sessions to actually run, so it was run.

No new dependency, no new file (so no REUSE/SPDX concern), no new
user-facing string (so no `po/` regeneration).
`docs/FFI-SOUNDNESS-AUDIT.md`'s findings table, Finding 3's own section (a
new "Resolution" subsection) and the overall verdict are updated, as is
`docs/UNSAFE-AUDIT.md`'s INVESTIGATE entry — which had flagged precisely
this argument as "not pinned by a test", and was right.

`ci/checks.sh` still cannot run on this VM ([[checks-sh-blocked-on-vm]]).

**FFI-SOUNDNESS-AUDIT is now closed** except Finding 4, which its own text
calls cosmetic and unscheduled.

NIGHT-SHIFT: FFI-SOUNDNESS-AUDIT Finding 3 delivered and pushed — a
demonstrated use-after-free on the `EOAuth2Service` borrowed-pointer path,
fixed and pinned by three tests. Ending the session here per the standing
rule against starting a second large item.

## 2026-08-20 (claim) — Claiming CURRENT PRIORITY item 8's standing fix: wire the packaging `.deb` ctest into `ci/checks.sh`

Fresh survey first, against `origin/master` at `dab9348` (unchanged since the
last session's Finding 3 delivery): walked `docs/ROADMAP.md` end to end again,
including every sub-item's status text. CURRENT PRIORITY items 1-8 are all
code-complete, each blocked only on an operator/maintainer step already logged
as such (M7, item 5's SRV resolver, item 6's API-token method, item 7's
collection-backend fix, item 8's CI fix itself — all pending a real Evolution
session or a live-CI confirmation this runner cannot produce). Confirmed via
the GitHub Actions API that CI is green at `dab9348` (the head commit), so
item 8's own fix holds. Round 2 Track A is fully closed; Track B1/C2/C4 are
explicit NEEDS-DECISION; Track D1 and Track E Phase 0/Path A are code-complete
pending operator confirmation, D2 and Track E Phase B/C explicitly need a
fresh design/maintainer decision before they are claimable.
`docs/UNSAFE-AUDIT.md`/`docs/FFI-SOUNDNESS-AUDIT.md` are closed except items
their own text calls cosmetic/deprioritized/needing a fresh design. The two
BACKLOG.md findings (jmap-vcard trailing-whitespace nit, jmap-ical DATE-TIME
panic) are both closed-backend (M3/M4) polish the CURRENT PRIORITY directive
says not to reopen. This matches many prior sessions' independent surveys.

No M7/real-server/M9/M10 work remains unblocked. Per the standing rule, that
is a signal to stop looking for backend polish, not to invent some — but item
8's own text names one concrete, still-open, non-backend task: **"make the
agents' pre-push gate run the packaging ctest (or at least lintian) so a red
`.deb` cannot land unseen again."** Confirmed unaddressed: `ci/checks.sh`
(the cargo-only gate every agent runs before pushing) has no packaging/ctest
step; the `.deb`'s lintian check only runs in CI's separate `build` job,
which is exactly why the RUNPATH regression sat red for days before a session
noticed the mismatch. This is a tooling/CI-gate fix, not M1-M8 backend code,
so it does not fall under "do not reopen completed backends."

**Claiming this.** Increment: add a best-effort packaging-ctest step to
`ci/checks.sh`, gated on `cmake`/`ninja`/the EDS pkg-config modules being
present (skip with a message otherwise, preserving the script's documented
"works on a bare Rust-only machine" property), scoped to just the
`package-deb*` ctest tests (not the functional/gui-smoke legs, which need a
live D-Bus/Xvfb registry `ci/checks.sh` has never assumed). Verification plan:
confirm the new step passes on current `master`, then temporarily
(uncommitted) reintroduce the exact RUNPATH regression `ac00396`/item 8 fixed
and confirm the new step fails on it, then revert the temporary change and
confirm green again — proving the gate actually catches the regression class
it exists for, not just that it runs.

## 2026-08-20 — Delivered: item 8's standing fix (`ci/checks.sh` now runs the `.deb` packaging ctest)

Delivered the increment claimed above. `ci/checks.sh` gained a final step
that configures/builds the CMake tree and runs `ctest --test-dir build -R
'package-deb'` (the three CPack-based tests: `package-deb`,
`package-deb-reproducible`, `package-deb-lintian`) — not the full ctest
suite, since the functional/gui-smoke legs need a live D-Bus/Xvfb registry
this script has never assumed, only a CMake configure + build. Gated on
`cmake`, `ninja`, and the four EDS pkg-config modules `CMakeLists.txt`
requires all being present; otherwise it prints a skip message rather than
failing, preserving the script's documented "works on a bare Rust-only
machine" property (`cmake_minimum_required`'s own `pkg_check_modules(...
REQUIRED ...)` would hard-fail the configure step on a machine without EDS
headers, so this has to be checked before invoking cmake, not left for cmake
to report).

**Proved it actually catches the regression it exists for, not just that it
runs.** Ran the new step on unmodified `master` first (green: all three
`package-deb*` tests pass). Then, in the working tree only (never committed),
replaced `evo-sys/build.rs` with its pre-`e14d991` content — the exact
`ac00396` RUNPATH regression CURRENT PRIORITY item 8 fixed — rebuilt, and
reran just `package-deb-lintian`: it failed with the same two
`custom-library-search-path RUNPATH /usr/lib` /
`RUNPATH /usr/lib/x86_64-linux-gnu` findings item 8's own report quoted,
confirmed the lintian override alone (already in the tree) does not mask a
reintroduced regression of the broader kind. Restored the real `build.rs`,
rebuilt, reran: green again, and `git status`/`git diff` confirmed the file
came back byte-identical (no residual diff) before committing anything.

No Rust source changed, so `cargo fmt --check` (already run: clean),
clippy, and the test suite are unaffected by this change and stay covered by
CI's existing green run at `dab9348`; re-ran `cargo fmt --check` anyway as a
sanity check on the touched-tree state. `reuse lint` could not run on this VM
(`[[checks-sh-blocked-on-vm]]`, unchanged), but the only file touched already
carries its SPDX header and needed no new one; `bash -n ci/checks.sh` confirms
the script still parses. No new dependency, no new user-facing string, no
Cargo.lock change.

**Scope, stated plainly:** this closes item 8's own "standing fix" footnote,
not a new roadmap item — CURRENT PRIORITY items 1-8 remain exactly where the
claim entry above described them (all code-complete, pending
operator/maintainer steps this runner cannot produce). No M7/real-server/M9/
M10 work was available to claim this session; this was the one concrete,
unblocked, non-backend-polish gap the roadmap's own text still named.

`ci/checks.sh` still cannot run its first (`reuse lint`) step on this VM
([[checks-sh-blocked-on-vm]]) — everything past that step was verified by
hand, as in every prior session hitting the same gap.

NIGHT-SHIFT: item 8's standing fix delivered and pushed — `ci/checks.sh` now
runs the packaging `.deb` ctest, demonstrated to catch the exact RUNPATH
regression class item 8 closed. Ending the session here; no priority-lane
(M7/real-server/M9/M10) work remained unblocked to start a second item.

## 2026-08-20 — Claiming CURRENT PRIORITY item 9: blob download trusts an unauthenticated cross-host redirect as if it were the message

Fresh survey against `origin/master` at `3e5c0e9` (unchanged since the log
rotation). Items 1-8 are exactly where the last several sessions left them —
code-complete, each blocked on an operator/maintainer step this runner cannot
produce. Item 9 (queued by `680be1e` right after item 7's operator
verification) is the one CURRENT PRIORITY gap with no such block: "reproduce
with a jmap-mock mode that serves downloadUrl from a different host and/or
302-redirects the blob GET… (2) Fix the blob-download auth/host handling in
jmap-client" is headless, TDD-able work, exactly the item's own text says so.
Claiming it.

The item's three hypotheses for *why* Fastmail's live download returns its
marketing homepage (cross-host redirect stripping auth, a different
credential scheme entirely, a `downloadUrl` template bug) are explicitly not
resolvable from here — no live Fastmail token, and the item's own text defers
that to an operator probe. What *is* resolvable headlessly is the shape of
client-side defect the first hypothesis implies and that a mock can
reproduce byte-for-byte: `UreqTransport` already strips `Authorization` on a
cross-host redirect (`86fea00`'s `SameHost` policy, correct per RFC 7235 —
the item's own text says not to weaken this), but nothing stops the client
from then treating whatever the *unauthenticated* redirect target answers
with as the blob. A JSON response with the wrong shape fails to parse and
surfaces as an error; a blob is raw bytes with no shape to be wrong, so nothing
in the existing code would notice the switch — confirmed by writing the
failing test first (below) against unmodified code.

**Increment:** give `jmap-mock` a `download_via_redirect_to(origin)` mode
(mirrors `session_via_redirect`'s shape, on the download route instead of
session discovery); write a red test using a second, genuinely
different-host listener (127.0.0.2, not another port on 127.0.0.1 — `ureq`'s
own `SameHost` check compares hostname only, via `http::uri::Authority::
host()`, confirmed against `ureq-proto`'s `can_redirect_auth_header` source,
so two ports on the same loopback address would not exercise cross-host at
all); fix `jmap-client` to refuse a download whose response came from a
different origin than requested.

## 2026-08-20 — Delivered: `download_blob` refuses a cross-origin redirect's answer instead of returning it as the blob

Delivered the increment claimed above.

**Mock:** `MockServerBuilder::download_via_redirect_to(origin)` makes every
`GET /download/...` answer a `302` to the same path on `origin` instead of
serving the blob — `jmap-mock/src/server.rs`/`state.rs`, alongside
`session_via_redirect`/`advertise_origin`, which it is deliberately distinct
from: `advertise_origin` changes what the *session document* names without
changing what answers a request, this changes what answers the request while
the session still calls this server the download host — the shape of a
redirect the client never agreed to via the session.

**Test:** `jmap-client/tests/redirect_auth.rs` gained
`a_cross_host_redirect_on_download_is_not_trusted_as_the_blob`, plus a
`ForeignHost` test helper — a bare `tiny_http` responder (added as a dev-
dependency; already an existing workspace dependency of `jmap-mock`, so no
new crate in `Cargo.lock`) bound to `127.0.0.2:0`, not another port on
`127.0.0.1`. Confirmed against `ureq-proto`'s actual redirect source
(`can_redirect_auth_header` in `src/client/redirect.rs`, fetched from
GitHub, no local copy of the crate's source in this VM's registry cache) that
`RedirectAuthHeaders::SameHost` compares `http::uri::Authority::host()` —
hostname only, not port — so two mock servers on different ports of the same
loopback address would both read as "the same host" to `ureq` itself and
never exercise the cross-host path this item is about.

**Red confirmed first, by actually removing the fix and rerunning**, not by
inspection: the test failed with the foreign host's own page returned as the
blob, the exact Fastmail failure this item describes, then passed once the
fix (below) was restored — genuine TDD, not written to already-passing code.

**Fix:** `HttpResponse` (`jmap-client/src/transport.rs`) gained a
`final_url: String` field — the URL a response actually came from, via
`ureq`'s own `ResponseExt::get_uri()`, which tracks the post-redirect URL
regardless of `redirect_auth_headers` (that setting only gates whether
`Authorization` follows a redirect, not whether the redirect itself is
taken, so it was already being tracked, just never read). A new
`url::origin_of()` helper (scheme+authority prefix, sibling to the existing
`rebase_origin`, same slicing shape, own proptest for hostile input)
compares the origin requested against `final_url`'s; `download_blob` returns
a new `Error::CrossOriginRedirect { requested, followed }` on a mismatch
instead of the body. Scoped to `download_blob` only, not the shared
`execute_within` every request goes through — a deliberate choice, not an
oversight: rejecting *every* cross-host redirect at the transport level
would also catch a legitimate autodiscovery redirect to a provider's
JMAP-specific subdomain during session discovery, which nothing here tests
is safe to forbid, whereas a blob download addressed by the session's own
`downloadUrl` has no comparable legitimate reason to end up somewhere else.
Two existing fake `Transport` implementations (`srv_discovery.rs`'s, and
`response_size.rs`'s `Recording`, which wraps the real transport and needed
no change) were the only other `HttpResponse` construction sites; both
updated/confirmed.

**Does not close the item.** This closes the "Do" list's headless half — the
mock now reproduces the failure class and the client no longer silently
trusts an unauthenticated bounce. It does **not** determine which of the
item's three hypotheses actually explains Fastmail's live behaviour (the
item's own text already says that needs an operator/live token probe), and
it does not by itself make real Fastmail mail bodies readable if the true
cause turns out to be hypothesis 2 or 3 (a different credential scheme, or a
template bug) rather than 1 — those would still need their own fix once an
operator's live probe identifies which one it is. What this closes is a real
client-side soundness gap regardless of which hypothesis is Fastmail's actual
one: silently accepting an unauthenticated redirect target's content as
message bytes was always wrong, on any server shaped this way.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked --
-D warnings` (default-members) and the seven-crate EDS-gated clippy
(`evolution-jmap-client`, `jmap-backend-core`, `jmap-backend-book`,
`jmap-backend-cal`, `jmap-mail`, `jmap-backend-collection`, `jmap-config`)
both clean. `cargo test --locked` (default-members) green, 0 failed; the
seven EDS-gated crates run per-crate (the standing multi-package hang) all
green, 0 failed throughout. Also ran the packaging leg per item 8's own
standing-fix note: `cmake -S . -B build -G Ninja && ninja -C build && ctest
--test-dir build -R 'package-deb'` — all three packaging tests passed;
packaging-neutral change (no build script, no install rule touched), so this
confirms rather than fixes. Disk filled to "No space left on device" on the
first `cargo test --locked` run (`rust/target` at 24G) — `rm -rf
rust/target` recovered it, the standing note again
([[disk-fills-from-cargo-target]]). No new user-facing string, so no `po/`
regeneration. `reuse lint` could not run on this VM
([[checks-sh-blocked-on-vm]]) — the only files touched already carry their
SPDX header and no new file was added.

NIGHT-SHIFT: item 9's headless half delivered and pushed — a real client-side
soundness gap (an unauthenticated cross-host redirect's answer silently
trusted as blob data) found, TDD'd, and fixed. The item stays open: it needs
an operator's live Fastmail probe to identify which of its three hypotheses
actually explains the marketing-homepage response, and possibly a further
fix once that is known. Ending the session here per the standing rule
against starting a second large item.

## 2026-08-20 — Survey found no unblocked CURRENT PRIORITY / Round 2 work; closed a stale-docs gap and pinned an already-fixed panic instead

Fresh survey against `origin/master` (unchanged at `31919a4`, this thread's
own last push). Walked CURRENT PRIORITY items 1-9, M9/M10, and Round 2
Tracks A-F end to end:

- **Item 9** (mail-body blob download) — headless half already delivered
  last session; the rest needs the operator's live, token-gated Fastmail
  probe. Human-blocked.
- **M7, M9, M10** — all tagged `COMPLETE` in `docs/MILESTONES.md`, confirmed
  by reading the actual milestone text and the CI workflow (`functional`,
  `gui-smoke`, `eds-version-matrix` jobs all exist and are wired).
- **Track A** (A1-A7) — every pattern DONE per `docs/UNSAFE-AUDIT.md` and
  `docs/FFI-SOUNDNESS-AUDIT.md`, **except** their text hadn't caught up with
  two commits already on `master`: `3dacd8b` (Finding 1,
  `set_raw_gerror` hardening) and `dab9348` (Finding 3, the
  `oauth2.rs::borrowed` use-after-free, escalated to opus and fixed) both
  landed after `docs/ROADMAP.md`'s A5 write-up was last touched, so that
  section still called Finding 3 "deliberately left open" when it is not —
  the same "docs-sync gap" pattern A1/A2/A3/A7 hit before. Fixed the roadmap
  text to match `docs/FFI-SOUNDNESS-AUDIT.md`'s current findings table (only
  Finding 4, tagged cosmetic/not-scheduled, is genuinely still open).
- **Track B** — explicitly gated `NEEDS-DECISION`, not claimable.
- **Track C** — C1/C3 done, C2/C4 explicitly `NEEDS-DECISION`.
- **Track D** — D1's code side is complete (create *and* delete both
  wired), pending the operator's real-Evolution right-click-delete
  confirmation; D2's write-back is tagged `RESEARCHED, NOT CLAIMABLE YET`
  (needs a signal-lifecycle design, not a mechanical port).
- **Track E** — Phase 0 + Path A code-complete pending operator
  confirmation (the free/busy panel needs a live registry); Phase B/C
  explicitly "do NOT start until Path A lands + maintainer OK".
- **Track F** — SPIKE closed, no-op.
- **`docs/BACKLOG.md`** — re-read every entry against the current tree
  rather than trusting the file. Two are closed-backend fidelity nits
  correctly left alone (vCard trailing whitespace, calendar colour
  write-back, contact fidelity list). The third, "`jmap-ical` panics on a
  DATE-TIME value with a non-ASCII byte before offset 6" (filed 2026-08-19,
  flagged as the more valuable of two fuzzer survivors precisely because
  it is a real panic on the untrusted-server boundary, not cosmetic
  fidelity), turned out to be **already fixed** —
  `to_local_date_time` (`jmap-ical/src/event.rs:3871`) already checks
  `date.is_char_boundary(8)`/`time.is_char_boundary(6)` before slicing, via
  `3a25473` in the Antigravity/agy-lane polish branch merged same-day
  (`f4c1ae7`), independently of and slightly after whoever wrote that
  backlog entry from their own checkout. Confirmed this really is what
  fixed it, not a coincidence, by reverting just those two
  `is_char_boundary` checks locally and rerunning: the exact minimal input
  from the backlog entry panics again at the same line
  (`end byte index 6 is not a char boundary`); restored, green again.

**No genuinely unblocked CURRENT PRIORITY or Round 2 `[claude]`-lane item
exists this session** — everything left is operator-verification-blocked,
maintainer-decision-gated, or backlog-listed closed-backend polish this
thread is directed not to reopen. Rather than end with a bare `BLOCKED`
report and nothing to show, closed the one real gap the survey turned up:
the `jmap-ical` panic fix had no regression test pinning it down, so a
future refactor could silently reopen it with no fuzzer run to catch it
before it reached `master` again. Added
`jmap-ical/tests/hostile.rs::a_dtend_with_a_multibyte_character_at_the_slice_boundary_does_not_panic`
(the untrusted-input-hardening file this class of bug belongs in, per
Track A3/A4's own framing), asserting the event still parses with `DTEND`
dropped rather than invented, confirmed red against the reverted code
first. Also corrected `docs/ROADMAP.md`'s stale A5 write-up and
`docs/BACKLOG.md`'s panic entry to say "fixed", so neither misleads a future
session into re-escalating or re-diagnosing work already done.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked --
-D warnings` (default-members, includes `jmap-ical`) clean — no EDS-gated
crate touched, so the seven-crate clippy/test rerun was skipped as
redundant. `cargo test --locked` (default-members) green, 0 failed. No new
dependency, no new user-facing string, no new file (only an existing
SPDX-headed test file grew a test), so no `po/`/`reuse` action needed.

NIGHT-SHIFT: no unblocked increment existed; closed a stale-docs gap
(ROADMAP A5, BACKLOG's jmap-ical entry) and pinned an already-fixed panic
with a permanent regression test instead of ending with nothing. Next
session: same survey will likely find the same shape unless the operator
has run one of the pending live-Evolution/Fastmail confirmations (item 9,
D1, Path A) or the maintainer has ruled on Track B/C2/D2's open decisions.

## 2026-08-20 (claim) — Claiming CURRENT PRIORITY item 9 plan step (1): the `Accept: application/json` smell on `download_blob`'s GET

`git fetch`: `origin/master` unchanged at `3013174` (this thread's own last
push, the roadmap entry queuing this exact plan). That entry's own text
marks step (1) — "fix the `Accept` header on `download_blob`'s GET, with a
jmap-mock mode that refuses/redirects a blob GET carrying `Accept:
application/json` and serves it for a spec-appropriate `Accept`" —
headless, claimable now, and explicitly "CLAIM THIS FIRST". Claiming it.

## 2026-08-20 — Delivered: `download_blob` declares `Accept: */*`, not `application/json`

**Root cause, as the roadmap entry already named it:** `execute_within`
(`jmap-client/src/client.rs`) hardcoded `Accept: application/json` on every
outgoing request, including `download_blob`'s GET. A blob download never
answers JSON — RFC 8620 §6.2 gives it no reason to declare that header — and
a server doing RFC 7231 §5.3.2 content negotiation is free to refuse or
redirect a request that (wrongly) claims JSON is the only acceptable answer.
This was flagged as the leading headless explanation for Fastmail's blob GET
302-redirecting to `www.fastmail.com` instead of answering the message.

**Fix:** `execute_within` gained an `accept: &str` parameter instead of the
hardcoded literal. `execute_with_content_type` (used by every API call and
the upload endpoint) passes `"application/json"` — unchanged behaviour, the
one existing call site of `execute_within` outside `download_blob`.
`download_blob` (`mail.rs`) is now the one caller that declares `Accept:
*/*` — "any", the conventional way to state no format preference, and
correct regardless of which content type the server happens to answer a
given blob with.

**TDD:** `jmap-mock` gained `MockServerBuilder::reject_download_accept_json()`
(mirrors `download_via_redirect_to`'s shape): the `/download/...` route
answers `406` when the GET's `Accept` header is exactly `application/json`,
and serves the blob for anything else (including no `Accept` header at all).
Red confirmed first, not by inspection: temporarily reverted
`download_blob`'s call site back to `"application/json"` and reran the new
test — failed with the mock's `406`, the same shape of refusal a real
content-negotiating server could give; restored, green.
`jmap-client/tests/redirect_auth.rs::download_blob_does_not_declare_accept_application_json`
pins the fix down permanently, alongside the existing cross-origin-redirect
test in the same file (both now cover the download path from different
angles).

**Does not close item 9.** This is plan step (1) only — headless,
jmap-mock-verified, and a real spec-compliance fix regardless of whether it
alone explains Fastmail's behaviour. Steps (2) (compare this client's
download request shape to a reference client like `mujmap`/`jmapc`) and (3)
(the operator's live, token-gated probe of the real `downloadUrl` exchange
against Fastmail) are still open and are what will show whether removing
this `Accept: application/json` claim is sufficient to turn Fastmail's 302
into a 200, or whether a different credential scheme or URL-template issue
(the item's other two hypotheses) is also in play.

**Gate.** `cargo fmt --check` clean. `cargo clippy --all-targets --locked --
-D warnings` (default-members) and the seven-crate EDS-gated clippy
(`evolution-jmap-client`, `jmap-backend-core`, `jmap-backend-book`,
`jmap-backend-cal`, `jmap-mail`, `jmap-backend-collection`, `jmap-config`)
both clean. `cargo test --locked` (default-members) green, 0 failed; the
same seven crates run per-crate (the standing multi-package hang) all
green, 0 failed throughout. Packaging leg per item 8's own standing-fix
note: `ninja -C build && ctest --test-dir build -R 'package-deb'` — all
three packaging tests (`package-deb`, `package-deb-reproducible`,
`package-deb-lintian`) passed. No new dependency; no new user-facing
string, so no `po/` regeneration. No new file, so no `reuse lint` action
needed — every touched file already carries its SPDX header
([[checks-sh-blocked-on-vm]]).

NIGHT-SHIFT: item 9 plan step (1) delivered and pushed — the `Accept:
application/json` spec smell on `download_blob`'s GET, fixed and TDD'd
against a new jmap-mock content-negotiation mode. The item stays open: steps
(2) and (3) (reference-client comparison, operator's live Fastmail probe)
are what will confirm whether this alone makes real Fastmail mail bodies
readable. Ending the session here per the standing rule against starting a
second large item.

## 2026-08-20 (claim) — Claiming CURRENT PRIORITY item 9 plan step (2): reference-client comparison

`git fetch`: `origin/master` unchanged at `059c543` (this thread's own last
push, step (1)). That entry's own plan step (2) — "compare our download
request to a known-good JMAP client's — candidate `mujmap` (Rust, syncs
Fastmail↔maildir) or `jmapc` (Python)" — is headless and claimable now.
Claiming it.
