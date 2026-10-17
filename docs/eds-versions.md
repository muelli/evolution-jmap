# Which Evolution Data Server this builds against

An EDS backend module is a shared object that EDS `dlopen`s into its own
process. Its classes derive from EDS classes and its instance structs begin
with EDS instance structs. The module is therefore bound to EDS by its
**ABI** (struct layouts, field offsets, vfunc slot order), not only by its
API.

> **Build the plugin against the exact Evolution Data Server it will run
> on.** A `libebookbackendjmap.so` built against one EDS and installed beside
> another is undefined behaviour. Distribution packages rebuild on every EDS
> update; if you build from source, do the same.

## Supported versions

| EDS | Status |
| --- | --- |
| < 3.52 | **Not supported.** `rust/crates/eds-sys/build.rs` refuses to configure. The backends use `EBookMetaBackend`/`ECalMetaBackend`, and older releases lack parts of that API. |
| 3.52 | **The pinned target.** The CI image installs it and `docs/functional-tests.md` runs against it. |
| newer | **Expected to work, and checked.** No code uses a 3.52-only API, but a later release can move a struct. The checks below exist for that. |

The Evolution UI modules have one more boundary: Evolution 3.55.1 replaced
GtkUIManager with EUIManager. `rust/crates/evo-sys/build.rs` emits the
`evolution_eui_manager` cfg, and `jmap-ui` picks its action API from it at
compile time.

## How a version mismatch is caught

bindgen generates the bindings from the installed headers at build time, so a
struct that moved between releases changes the build input.
`rust/crates/eds-sys/tests/layout.rs` then compares three versions:

1. what **pkg-config** resolved when `build.rs` chose the include paths
   (`eds_sys::EDS_HEADER_VERSION`);
2. what the **headers** say (`EDS_MAJOR_VERSION` and its siblings in
   `libedataserver/eds-version.h`);
3. what the **library loaded at run time** says (`eds_major_version` and its
   siblings in `libedataserver`).

The tests:

- **(1) against (2)**: `pkg_config_describes_the_headers_it_pointed_at`.
- **(2) against (3)**:
  `the_running_eds_can_serve_what_these_bindings_were_compiled_against`,
  which calls `eds_check_version()` so that EDS decides what is compatible.
- **Struct layouts**: each `assert_layout!` compares a subclassed struct's
  `size_of` with the size that `g_type_query()` reports at run time. A wrong
  offset becomes a failing test instead of a segfault.

Each failure message names the pkg-config, header and runtime versions, so a
log from a matrix run shows which release broke.

`build.rs` also stops before it generates anything when the `.pc` files it
probes (`libebackend-1.2`, `libedata-book-1.2`, `libedata-cal-2.0`,
`camel-1.2`) do not agree on a version. They ship in one tarball, so a
disagreement means a mixed installation.

## What this does not catch

These checks do not see **semantic** drift: the same struct size and function
signature with a changed contract (who owns a returned pointer, what an enum
value means, what a vfunc may assume). Only the behavioural tests in
`docs/functional-tests.md`, which drive real EDS daemons, and a human reading
the EDS release notes catch that. A green build on a new EDS does not prove
that the plugin works there.
