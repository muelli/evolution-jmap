# ci/checks.sh gate holes

`ci/checks.sh` is meant to be the local, single definition of "green": what
CI's `checks` and `build` jobs enforce, runnable identically before every
push. Twice in two days (`3930b24e`, `d5d3ee96`) a push passed it clean and
then sat red in real CI, both times because a check CI runs was silently
skipped or narrowed locally. This sweep covers every
check `ci/checks.sh`, `ci/build.sh` and the CMake/CTest suite register,
whether it can silently no-op, and whether CI actually has what it needs.

| Check | Needs | If missing, locally | Does CI have it |
|---|---|---|---|
| REUSE lint | `reuse`, `pipx`, or `uvx` | Loud `exit 1` (no silent pass) | Yes (`pipx` ships on GitHub's `ubuntu-24.04` image) |
| `cargo fmt --check` | `cargo fmt` component | Hard error if absent (rustup always installs it) | Yes |
| `cargo clippy -D warnings` | `cargo clippy` component | Hard error if absent | Yes |
| unsafe-count meter | nothing beyond `grep`/`wc` | N/A, can't silently skip | N/A |
| `cargo test --locked` | nothing extra | Runs only `rust/Cargo.toml`'s `default-members`; the EDS-gated crates (`jmap-backend-core`, `jmap-mail`, `jmap-config`, `jmap-backend-collection`, `jmap-backend-book`, `jmap-backend-cal`, `jmap-ui`, `eds-sys`, `evo-sys`) are excluded **by design**, not a hole — see the `rust-test-eds` row | N/A, same scope everywhere |
| `cargo deny check` | `cargo-deny` | Self-installs (`cargo install --locked cargo-deny`) if absent, so no silent skip, just slower | Yes |
| **`rust-test-eds` ctest** | cmake, ninja, the EDS dev headers (`pkg-config evolution-shell-3.0` etc.) | **Was silently excluded**: the packaging block's `ctest -R 'package-deb\|debian-copyright-in-sync'` filter never matched it, so a machine with full EDS headers (this one) never ran `cargo test` on the EDS-gated crates. This is exactly what let `d5d3ee96`'s stale po catalogue (caught by `jmap-backend-core`'s `potfiles.rs`) pass locally and sit red in CI. **Fixed this session**: the filter is now `-E '^rust-test$'` (run everything CTest registers except the already-covered plain `rust-test`). | Yes, CI's `build` job runs the full `ctest --test-dir build` with no `-R` filter |
| **`translations`, `release-workflow`, `install-*-backend` (4 tests)** | same cmake/ninja/EDS-header gate as above, nothing further | Same hole as `rust-test-eds`: registered, cheap, needed nothing beyond what the existing gate already confirms, but excluded by the old narrow filter. **Fixed in the same change.** | Yes, same `build` job |
| `package-deb`, `package-deb-reproducible` | cmake, ninja, EDS headers, `cpack`/`dpkg` | Already ran before this session (matched the old filter) | Yes |
| `package-deb-lintian` | `lintian` (CMake `find_program`, test only registered if found) | Loud: `ci/checks.sh` already `exit 1`s unless `CI_CHECKS_ALLOW_MISSING_LINTIAN=1` is set (fixed after an earlier incident where this ran silently vacuous) | Yes, `ci/install-deps.sh` installs it |
| `debian-copyright-in-sync` | `python3` (CMake `find_program`, test only registered if found) | Loud: `ci/checks.sh` now `exit 1`s unless `CI_CHECKS_ALLOW_MISSING_PYTHON3=1` is set, mirroring the `lintian` row above. | Yes |
| `functional`, `gui-smoke` ctest labels (`Functional.cmake`) | `dbus-run-session`, `gnome-keyring-daemon`, the full `evolution`/Xvfb runtime, `-DENABLE_FUNCTIONAL_TESTS=ON` | Not registered at all without the runtime (`find_program` guards): **by design**, not a hole. These need a live D-Bus/Xvfb registry `ci/checks.sh` has never assumed; they are exercised by the separately gated `functional`/`gui-smoke` CI jobs instead, each independently reproduced from scratch already. | Only on `workflow_dispatch` or a PR label, not every push — also by design, documented in `ci.yml` |
| `eds-version-matrix` job | a newer EDS (Fedora container), `ci/eds-matrix.sh` | Not run by `ci/checks.sh` at all: a second-EDS-version leg, not a correctness gate (`continue-on-error: true` in CI itself) | Only on `workflow_dispatch` or a PR label |
| `#[ignore]`d live-server tests (`--features live-server`) | a real Stalwart server, `STALWART_URL`/credentials | Never run by `ci/checks.sh` or any CI job: no live server exists in CI. **By design**, operator/agent-run only. | No, intentionally |

## Still open after this session

None from this sweep. The `python3` escape hatch is now in place; whether
any runner VM actually lacks `python3` and needs it installed is an
operator-side question, not a code-repo gap.
