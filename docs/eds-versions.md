# Which Evolution Data Server this builds against

An EDS backend module is a shared object. EDS loads it with `dlopen` into its
own process. The classes of the module derive from EDS classes. Its instance
structs begin with EDS instance structs. The **ABI** (struct layouts, field
offsets, vfunc slot order) therefore binds the module to EDS. The API alone
does not.

> **Build the plugin against the exact Evolution Data Server that it will run
> on.** If you build a `libebookbackendjmap.so` against one EDS and install it
> beside another EDS, the behavior is undefined. Distribution packages rebuild
> on every EDS update. If you build from source, do the same.

## Supported versions

| EDS | Status |
| --- | --- |
| < 3.52 | **Not supported.** `rust/crates/eds-sys/build.rs` refuses to configure. The backends use `EBookMetaBackend` and `ECalMetaBackend`. Older releases do not have all of that API. |
| 3.52 | **The pinned target.** The CI image installs it. `docs/functional-tests.md` runs against it. |
| newer | **Expected to work, and checked.** No code uses an API that exists only in 3.52. A later release can move a struct. The checks below find this. |

The Evolution UI modules have one more boundary. Evolution 3.55.1 replaced
GtkUIManager with EUIManager. `rust/crates/evo-sys/build.rs` emits the
`evolution_eui_manager` cfg. `jmap-ui` uses this cfg to select its action API
at compile time.

## How the build finds a version mismatch

bindgen generates the bindings from the installed headers at build time. If a
struct moved between releases, the build input changes.
`rust/crates/eds-sys/tests/layout.rs` then compares three versions:

1. The version that **pkg-config** resolved when `build.rs` chose the include
   paths (`eds_sys::EDS_HEADER_VERSION`).
2. The version that the **headers** state (`EDS_MAJOR_VERSION` and the related
   macros in `libedataserver/eds-version.h`).
3. The version that the **library loaded at run time** states
   (`eds_major_version` and the related symbols in `libedataserver`).

These are the tests:

- **(1) against (2)**: `pkg_config_describes_the_headers_it_pointed_at`.
- **(2) against (3)**:
  `the_running_eds_can_serve_what_these_bindings_were_compiled_against`. This
  test calls `eds_check_version()`, so EDS decides what is compatible.
- **Struct layouts**: each `assert_layout!` compares the `size_of` of a
  subclassed struct with the size that `g_type_query()` reports at run time.
  A wrong offset becomes a failing test. It does not become a segfault.

Each failure message gives the pkg-config, header and runtime versions. A log
from a matrix run therefore shows which release broke.

`build.rs` also stops before it generates anything if the `.pc` files that it
probes disagree on a version. The files are `libebackend-1.2`,
`libedata-book-1.2`, `libedata-cal-2.0` and `camel-1.2`. They ship in one
tarball. A disagreement therefore means a mixed installation.

## What these checks do not find

These checks do not find **semantic** drift. In semantic drift, the struct
size and the function signature stay the same, but the contract changes.
Examples are the owner of a returned pointer, the meaning of an enum value and
the assumptions that a vfunc may make. Two things find semantic drift:

- the behavioral tests in `docs/functional-tests.md`, which drive real EDS
  daemons
- a person who reads the EDS release notes

A green build on a new EDS does not prove that the plugin works there.
