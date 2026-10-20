# EDS version matrix

bindgen generates `eds-sys` at build time. It reads the EDS headers that
`pkg-config` finds. The FFI layer is therefore consistent with one EDS: the
EDS that is installed where `cargo build` ran. `eds-sys/tests/layout.rs`
finds a layout drift on that version (see `docs/eds-versions.md`). It does not
find a renamed symbol, a changed signature or a different layout in another
release. The matrix turns such a drift on a newer EDS into a failing CI job.
Without the matrix, the drift causes a segfault on the machine of a user.

## The two legs

- **Pinned 3.52**: the `build` job in `.github/workflows/ci.yml`.
  `ci/install-deps.sh` installs the EDS 3.52 headers of Ubuntu 24.04.
  `ctest --test-dir build` includes the `rust-test-eds` target
  (`cmake/Rust.cmake`). This target runs `cargo test` for `eds-sys`, `evo-sys`
  and every crate that needs EDS headers. The plugin ships against this
  version.
- **Newer EDS**: the `eds-version-matrix` job. It runs on `workflow_dispatch`
  or on a pull request with the label `run-eds-version-matrix`. It runs
  `ci/eds-matrix.sh` (clippy with `-D warnings`, then `cargo test` for the
  same set of crates). It runs in a Fedora container that a digest pins. The
  container has `evolution-data-server-devel` and `evolution-devel`. To see
  which EDS the digest gives, run `pkg-config --modversion camel-1.2` in the
  container. The job has `continue-on-error: true`.

A digest pins the container. A move to a newer EDS is therefore an explicit
change to `ci.yml`.

## Reproducing the newer-EDS leg locally

Any machine with `docker` (or `podman`) can run the container that `ci.yml`
pins:

```sh
sudo docker run -d --name edsmatrix -v "$PWD:/src:ro" -w /work \
    fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c86c791644496d9ac230de7711f5b8ec3b898 \
    sleep infinity
sudo docker exec edsmatrix dnf -y install git evolution-data-server-devel \
    evolution-devel pkgconf-pkg-config gcc clang-devel
sudo docker exec edsmatrix sh -c \
    'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
     --default-toolchain stable --profile minimal'
sudo docker exec edsmatrix sh -c \
    'cd /src/rust && PATH=/root/.cargo/bin:$PATH CARGO_TARGET_DIR=/work/target \
     ../ci/eds-matrix.sh'
```

The checkout is mounted read-only and cargo builds into `/work/target`, so
the host's `rust/target` is neither touched nor copied. The checkout is mounted read-only. Cargo builds into `/work/target`. The
`rust/target` of the host is therefore not changed and not copied. To see
every failure and not only the first, add `--no-fail-fast` to the
`cargo test` line in `ci/eds-matrix.sh`.

## How a version difference is expressed

`eds-sys/build.rs` reads the bindings that bindgen generated. For each entry
of its `EDS_FEATURES` table, it emits one `cargo::rustc-cfg` if the marker
identifier is present. The test is "does the installed header declare this
identifier". It is never a version comparison. A guessed version is silently
wrong on exactly the release for which the guess is wrong.

- If both releases have the same operation with a different spelling,
  `eds-sys/src/compat.rs` has one wrapper. The rest of the tree calls it.
- If an API was removed with no replacement, a `#[cfg]` gate protects the
  call site.
- A `rustc-cfg` reaches only the crate that emitted it. `eds-sys` therefore
  also publishes each detected feature as `cargo::metadata` (through its
  `links` key). Dependent build scripts, such as `jmap-mail/build.rs`, emit
  the cfgs that they need again. Detection stays only in `eds-sys/build.rs`.

These are the current markers:

| cfg | Marker | Meaning |
| --- | --- | --- |
| `eds_vcard_version_enum` | `EVCardVersion` | vCard functions take an `EVCardVersion` (see (A0) below) |
| `camel_provider_auto_detect` | `CamelProviderAutoDetectFunc` | `CamelProvider` still has `auto_detect` (removed in 3.58) |
| `camel_provider_url_helpers` | `pub url_hash: ` | `CamelProvider` still has `url_hash` and `url_equal` (removed in 3.60) |
| `camel_folder_search_object` | `camel_folder_search_new` | `CamelFolderSearch` exists and the provider implements search |
| `camel_summary_records` | `CamelMIRecord` | old summary record struct names |
| `camel_folder_get_uids` | `camel_folder_free_uids` | borrowed folder UID arrays |
| `eds_death_date_field` | `E_CONTACT_DEATHDATE` | the newer contact model (see (B)) |
| `camel_folder_search_body_sync` | `camel_folder_search_body_sync` | server-side body search entry point |
| `camel_uid_cache_get_new_uids` | `camel_uid_cache_get_new_uids` | the UID cache API |
| `camel_folder_thread_boxed` | `camel_folder_thread_messages_get_type` | the boxed thread type |
| `eds_credential_store_id` | `e_source_authentication_set_credential_store_id` | `credential-store-id` property (eds#663) |
| `eds_oauth2_dynamic` | `e_source_oauth2_client_get_type` | own OAuth2Dynamic support of EDS |
| `camel_data_cache_atomic` | `camel_data_cache_add_atomic` | atomic mail cache writes |

## Known differences between 3.52 and 3.60

### (A0) vCard and date strings, in `compat.rs`

- **`e_vcard_to_string` lost its format argument.** On 3.60, the function
  `e_vcard_convert_to_string(EVCard *, EVCardVersion)` receives the version.
  `EVCardVersion` and `E_VCARD_VERSION_30` replace `EVCardFormat` and
  `EVC_FORMAT_VCARD_30`. Both legs call `compat::e_vcard_to_string_vcard_30`.
  This wrapper asks explicitly for vCard 3.0. The call with one argument
  emits the own version of the card. The parser of `jmap-vcard` does not
  expect that version.
- **`e_contact_date_to_string` gained an `EVCardVersion` argument.** The
  wrapper is `compat::e_contact_date_to_string_vcard_30`.

### (A) The Camel surface of `jmap-mail`

- **`CamelFolderSearch` is gone (3.58+).** EDS removed
  `camel/camel-folder-search.h` and the `camel_folder_search_util_*` helpers.
  `CamelStoreSearch` replaces them. `search_sync` replaces
  `CamelFolderClass.search_by_expression` and `.search_by_uids`. The base
  class now installs a complete `search_sync` over `CamelStoreSearch`. The
  provider therefore implements the two old vfuncs on 3.52 and nothing on
  3.58+. The cfg `camel_folder_search_object` controls this.
  `jmap-mail/tests/search.rs` checks the search behavior through the entry
  point that the installed EDS has. The checks include the complement and
  "all". A leg that ignores the expression therefore fails.
- **EDS renamed the summary records.** `CamelStoreDBMessageRecord` and
  `CamelStoreDBFolderRecord` of 3.60 (in `camel-store-db.h`) still have the
  `bdata` field that the provider uses. `compat::CamelSummaryMessageRecord`
  and `compat::CamelSummaryFolderRecord` are aliases for them.
- **`CamelFolderSummaryClass.summary_header_save` changed.** It returned a
  new record. It now fills a `CamelStoreDBFolderRecord` that the caller owns.
  `jmap-mail/src/summary.rs` has both versions, which `#[cfg]` selects. They
  share the function that writes `bdata`. The ownership of `bdata` (a
  `g_malloc`ed string that the record frees) is the same on both.
- **`camel_folder_summary_get_array`, `_free_array` and `_get_changed`**
  became `dup_uids` and `dup_changed`. The caller frees them with
  `g_ptr_array_unref`. The wrappers are `compat::summary_dup_uids`,
  `summary_dup_changed` and `summary_free_uids`. If you call the wrong
  function, memory leaks. No error occurs.
- **`CamelProvider` lost `auto_detect` in 3.58, and `url_hash` and
  `url_equal` in 3.60.** Two `#[cfg]` gates protect the fields, one for each
  release. A field has no identifier of its own in the bindings, so the
  marker of the second gate is the field declaration. The provider stays a struct literal that
  names every field. A field that a future EDS adds is therefore a compile
  error. It is not a silent NULL.
- **`camel_folder_get_uids` and `camel_folder_free_uids` became
  `camel_folder_dup_uids`.** This function returns a reference-counted array
  that the caller owns. The wrappers are `compat::folder_dup_uids` and
  `folder_free_uids`. `tests/functional/mail-client.c` has its own local
  wrappers with the same names.
- **`camel_folder_search_util_hash_message_id` became
  `camel_search_util_hash_message_id`** in `camel-search-utils.h`. The
  wrapper is `compat::search_hash_message_id`.

### (B) Contact model semantics

`eds-sys/tests/contacts.rs` characterises the own C behavior of EDS. These
results differ between the two legs. The function names and signatures are the
same:

| vCard line | 3.52 resolves to | 3.60 resolves to |
| --- | --- | --- |
| `X-JABBER`, `X-AIM`, `X-GADUGADU`, `X-SKYPE`, `X-MATRIX`, `X-ICQ`, `X-MSN`, `X-YAHOO`, `X-GOOGLE-TALK`, `X-GROUPWISE` | the attribute-list field (for example `im_jabber`) | the first `_HOME_1` slot (for example `im_jabber_home_1`) |
| `ANNIVERSARY` | not mapped | `anniversary` |
| `X-EVOLUTION-ANNIVERSARY` | `anniversary` | not mapped |
| `DEATHDATE` | not mapped | `deathdate` (`E_CONTACT_DEATHDATE`) |
| `X-SIP`, `X-TWITTER`, `BDAY`, `X-DEATHDATE` | unchanged | unchanged |

If there is no `FILE_AS`, `E_CONTACT_NAME_OR_ORG` is `"Family, Given"` on
3.52. On 3.60 it is the full name.

Only `E_CONTACT_DEATHDATE` is visible in the headers. The tests therefore use
`eds_death_date_field` as the switch for all of these differences. They change
together between the two releases that this repository tests.

Not decided: whether `jmap-vcard` must map differently on a newer EDS. The
open questions are these:

- which IM field to read and write
- `ANNIVERSARY` or `X-EVOLUTION-ANNIVERSARY`
- whether anything depends on `NAME_OR_ORG`

### (C) clippy and bindgen's `_IO_FILE`

On the newer leg, rustc reports the accessor that bindgen generates for the
`_flags2` bitfield of `_IO_FILE` of glibc as `unnecessary_transmutes`. The EDS
headers include `_IO_FILE`. `eds-sys/src/lib.rs` allows that lint on the
generated module. `ci/eds-matrix.sh` can therefore run clippy with
`-D warnings`.

## The functional tests on a newer EDS

The functional tests (`docs/functional-tests.md`) can also run in the pinned
Fedora container. The full `evolution` package must be installed there. Note
these two points:

- Rootless podman drops `CAP_SETPCAP`, also with `--cap-add=SETPCAP`.
  `gnome-keyring-daemon` aborts without it. Every functional test then fails.
  Run the container with `--privileged`. The rootless user namespace still
  confines it.
- The C clients in `tests/functional/` do not link any crate from this
  repository. They therefore use `EDS_CHECK_VERSION` for API differences.

## The `functional` job itself, reproduced from scratch

To reproduce the gated `functional` job of `ci.yml` exactly, use a bare
`ubuntu:24.04` container. Run `ci/install-deps.sh`,
`ci/install-deps-functional.sh` and `ci/functional.sh`. The hosted runner has
a preinstalled Rust toolchain. In the container, first install a toolchain
with `rustup`. `ci/install-deps.sh` installs `libclang-dev`, which bindgen
needs.

Mount the checkout read-only (`-v "$PWD:/src:ro"`). Give cargo its own target
directory. Do not copy the checkout with `rust/target` into the container.
That directory is very large.

The `gui-smoke` job reproduces in the same way. Use
`ci/install-deps-gui-smoke.sh`, `cmake --install build --component
camel-provider` and `ci/gui-smoke.sh` (see `docs/gui-smoke-test.md`).

## Building EDS master locally

Some API changes are not in a packaged EDS yet. To test against EDS master,
build it in the pinned Fedora container:

```sh
sudo podman run -d --name edsmaster -v "$PWD:/src:ro" <the pinned fedora image> sleep infinity
sudo podman exec edsmaster mkdir -p /work
sudo podman exec edsmaster dnf -y install dnf-plugins-core git gperf cmake \
    ninja-build gcc gcc-c++ clang-devel pkgconf-pkg-config libdb-devel evolution-devel
sudo podman exec edsmaster dnf -y builddep evolution-data-server
sudo podman exec edsmaster sh -c \
    'cd /work && git clone --depth 1 --branch <branch> \
     https://gitlab.gnome.org/<fork>/evolution-data-server.git eds-src'
sudo podman exec edsmaster sh -c \
    'cmake -S /work/eds-src -B /work/build -G Ninja \
     -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCMAKE_INSTALL_PREFIX=/work/prefix \
     -DENABLE_GOA=OFF -DENABLE_INTROSPECTION=OFF -DENABLE_VALA_BINDINGS=OFF \
     -DENABLE_GTK_DOC=OFF -DENABLE_GTK4=OFF -DWITH_KRB5=OFF -DWITH_OPENLDAP=OFF'
sudo podman exec edsmaster sh -c 'cd /work/build && ninja -j"$(nproc)" install'
sudo podman exec edsmaster sh -c \
    'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
     --default-toolchain stable --profile minimal'
sudo podman exec edsmaster sh -c \
    'export PATH=/root/.cargo/bin:$PATH
     export PKG_CONFIG_PATH=/work/prefix/lib/pkgconfig:/usr/lib64/pkgconfig:/usr/share/pkgconfig
     export LD_LIBRARY_PATH=/work/prefix/lib
     export CARGO_TARGET_DIR=/work/cargo-target
     cd /src/rust && cargo test -p jmap-mail -p jmap-config -p jmap-backend-collection --locked'
```

- Install `libdb-devel` before the first `cmake` configure. If you do not,
  CMake caches a failed `HAVE_LIBDB` check. In that case, delete `/work/build`
  and configure again.
- `evolution-shell-3.0` and the other `evolution-*` pkg-config files come from
  the packaged `evolution-devel`. `PKG_CONFIG_PATH` lists the from-source
  prefix first. It therefore has priority for EDS.
- Build only the `install` target. A plain `ninja` also builds the own test
  programs of EDS. They take much longer.
- The build needs several gigabytes. After `ninja install`, delete
  `/work/eds-src` and `/work/build`.

`cargo build -p eds-sys -vv` shows which `rustc-cfg` markers the build script
emitted.

## OAuth2Dynamic

EDS 3.63.1 and newer have their own OAuth 2.0 support for dynamically
registered clients (`EOAuth2ServiceDynamic`, the `"OAuth2 Client"` source
extension). The marker `eds_oauth2_dynamic` detects it. If the marker is
present, these tests run:

- `eds-sys`'s
  `upstreams_dynamic_oauth2_pieces_answer_to_the_names_this_project_spells`
  checks two literals against the running EDS. The literals are
  `"OAuth2 Client"` and `"OAuth2Dynamic"`. bindgen cannot emit them.
- `jmap-config`'s
  `the_class_behind_the_group_is_the_one_this_eds_owns_it_with` and
  `what_this_crate_writes_is_what_upstreams_own_service_reads` check that the
  registration that this project writes is the registration that the own
  service of EDS reads.
- The SASL tests of `jmap-mail` check that
  `camel_sasl_authtype("OAuth2Dynamic")` finds `CamelSaslXOAuth2Dynamic` of
  EDS.

EDS master also prefers the collection source in
`e_source_registry_server_get_access_token_sync`. The function
`e_util_can_use_collection_as_credential_source` gates this. This does not
replace the `follow_oauth2` binding in `jmap_backend_collection::child_added`.
The gate declines if the host and user of a child are set and differ from
the host and user of the collection.

## What this does not catch

Struct-size and symbol checks are necessary, but they are not sufficient. A
contract change with an unchanged signature and size (ownership, enum meaning,
vfunc assumptions) passes the matrix. Two things find such a change:

- the behavioral tests (the functional tests, on the shipped version)
- a person who reads the EDS release notes

## Supported versions

Build the plugin against the EDS that it will run with. This is the ABI
contract of every EDS module. This repository ships for 3.52. The newer-EDS
leg shows what breaks next. It does not claim support.
