// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M3's last acceptance criterion is "a documented manual test recipe with a
// hand-written `.source` keyfile", and a recipe is prose: nothing else in this
// repository fails when a group name in it is wrong, or when the backend name
// it tells the reader to write stops being the one the factory answers to. The
// symptom is an address book Evolution never even tries to open, with no error
// anywhere, and the reader concludes the backend is broken.
//
// So the keyfile is a file — `docs/examples/jmap-mock.source`, the one the
// recipe says to copy — and these tests read it the way the registry does:
// `e_server_side_source_new` on a GFile is exactly what
// `evolution-source-registry` calls for every file in its sources directory,
// and it needs neither a bus nor a running daemon. What the reader copies is
// therefore parsed by EDS's own keyfile code, handed to the same
// `SourceConfig::from_source` the backend calls, and compared against what the
// recipe claims it means.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, ESource, ESourceBackend, ESourceRegistryServer,
    e_server_side_source_new, e_source_address_book_get_type, e_source_backend_get_backend_name,
    e_source_get_extension, e_source_registry_server_new, g_file_new_for_path, g_object_unref,
};
use jmap_backend_book::factory::FACTORY_NAME;
use jmap_backend_core::source::SourceConfig;

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-book-backend.md")
}

fn keyfile() -> PathBuf {
    repository().join("docs/examples/jmap-mock.source")
}

/// A `.source` file loaded the way `evolution-source-registry` loads one.
struct RegistrySource {
    server: *mut gobject_sys::GObject,
    source: *mut ESource,
}

impl RegistrySource {
    /// Parses `path`.
    ///
    /// `e_server_side_source_new` insists on a registry server — it is the
    /// object a server-side source reports changes to — but constructing one
    /// neither owns a bus name nor reads the user's sources; that only
    /// happens when it is run. So this is the whole daemon-free part of the
    /// daemon, and it is the part that turns a keyfile into an `ESource`.
    fn load(path: &Path) -> Self {
        let path_string =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("no NUL in a path");
        let mut error = ptr::null_mut();
        // SAFETY: the constructor takes no arguments; the path is
        // NUL-terminated and copied; the GFile is owned here and released
        // below.
        let (server, source) = unsafe {
            // `e_source_registry_server_new` is declared as returning its own
            // base class, `EDBusServer *`, the way EDS's own callers use it;
            // the cast back down is the one C does implicitly.
            let server = e_source_registry_server_new().cast::<ESourceRegistryServer>();
            let file = g_file_new_for_path(path_string.as_ptr());
            let source = e_server_side_source_new(server, file, &mut error);
            g_object_unref(file.cast());
            (server, source)
        };
        assert!(
            !source.is_null(),
            "the registry could not read {}: {}",
            path.display(),
            // SAFETY: a NULL return means the GError was set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );
        Self {
            server: server.cast(),
            source,
        }
    }

    fn config(&self) -> SourceConfig {
        // SAFETY: the source is alive for the duration of the call.
        unsafe { SourceConfig::from_source(self.source) }
            .expect("the documented account is a valid one")
    }

    /// What the `[Address Book]` group says the backend is called.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: referencing the extension's GType registers it, so that the
        // lookup below can find it in a test binary that has not otherwise
        // touched an address book source. Then a header constant naming an
        // extension the source owns.
        let name = unsafe {
            e_source_address_book_get_type();
            let extension =
                e_source_get_extension(self.source, E_SOURCE_EXTENSION_ADDRESS_BOOK.as_ptr())
                    .cast::<ESourceBackend>();
            e_source_backend_get_backend_name(extension)
        };
        if name.is_null() {
            return None;
        }
        // SAFETY: a non-NULL return is a NUL-terminated string owned by the
        // extension, which outlives the copy taken here.
        Some(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl Drop for RegistrySource {
    fn drop(&mut self) {
        // SAFETY: we hold the only reference to each.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server);
        }
    }
}

#[test]
fn the_recipes_keyfile_describes_the_mock_server() {
    let config = RegistrySource::load(&keyfile()).config();

    // `jmap-mockd`'s default port, in the clear, which `SourceConfig` allows
    // for loopback and nothing else.
    assert_eq!(config.origin, "http://127.0.0.1:8080");
    // No user, on purpose. A source that names one makes the backend ask EDS
    // for a password before it sends anything, and the recipe's first run
    // would then be a prompt rather than a connection; `jmap-mockd` with no
    // `--basic`/`--bearer` wants no credentials at all.
    assert_eq!(config.user, None);
    // No `[Resource]` group either: the address book `jmap-mockd` seeds gets
    // its id at startup, so the recipe cannot name one. The backend resolves
    // "the account's default" instead.
    assert_eq!(config.address_book_id, None);
}

#[test]
fn the_recipes_keyfile_names_the_backend_the_factory_answers_to() {
    // `evolution-addressbook-factory` matches this string against the
    // `factory_name` of every registered `EBookBackendFactory`. A mismatch is
    // silent: no factory claims the source, and the address book simply never
    // opens.
    assert_eq!(
        RegistrySource::load(&keyfile()).backend_name().as_deref(),
        Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
    );
}

#[test]
fn the_keyfile_spelling_that_turns_tls_on_is_method_not_secure() {
    // `[Security] Method=none` is the one line in the recipe whose opposite
    // is not a visible failure. `ESourceSecurity:secure` is a boolean *over*
    // the `Method` string, so a keyfile saying `Secure=true` sets no property
    // EDS knows and reads back as "no method", which is `none` — an account
    // that looks like it asked for TLS and did not. This pins both spellings
    // against a remote host, where the difference is refuse-to-connect rather
    // than a scheme.
    let keyfile = fs::read_to_string(keyfile()).expect("the keyfile is in docs/examples/");
    let remote = keyfile.replace("Host=127.0.0.1", "Host=jmap.example.com");

    let tls = TemporaryKeyfile::holding(&remote.replace("Method=none", "Method=tls"));
    assert_eq!(
        RegistrySource::load(tls.path()).config().origin,
        "https://jmap.example.com:8080"
    );

    let mistake = TemporaryKeyfile::holding(&remote.replace("Method=none", "Secure=true"));
    // SAFETY: the source is alive for the duration of the call.
    let source = RegistrySource::load(mistake.path());
    assert!(
        matches!(
            unsafe { SourceConfig::from_source(source.source) },
            Err(jmap_backend_core::source::SourceError::InsecureTransport(_))
        ),
        "`Secure=true` has to keep being refused, or the day EDS starts \
         honouring it is the day this recipe is wrong in the other direction"
    );
}

#[test]
fn the_recipe_quotes_the_keyfile_verbatim() {
    // The reader copies the file, but reads the document; if the two drift
    // apart, whichever one they trusted is the wrong one.
    let recipe = fs::read_to_string(recipe()).expect("the recipe is in docs/");
    let quoted = fenced_blocks(&recipe, "ini");

    assert_eq!(
        quoted.len(),
        1,
        "expected exactly one ini block in the recipe, found {}",
        quoted.len()
    );
    assert_eq!(
        quoted[0],
        fs::read_to_string(keyfile()).expect("the keyfile is in docs/examples/"),
        "docs/manual-test-book-backend.md and docs/examples/jmap-mock.source disagree"
    );
}

/// A `.source` file written next to the test binary, for the variants of the
/// recipe's keyfile that are about what *not* to write.
struct TemporaryKeyfile(PathBuf);

impl TemporaryKeyfile {
    fn holding(contents: &str) -> Self {
        // The uid a server-side source gets is its file name, so the name has
        // to be unique per test process; the directory is cargo's, which is
        // writable wherever the suite runs.
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "recipe-{}-{}.source",
            std::process::id(),
            contents.len()
        ));
        fs::write(&path, contents).expect("the cargo temp directory is writable");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryKeyfile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The contents of every ```` ```<language> ```` block in `markdown`.
fn fenced_blocks<'a>(markdown: &'a str, language: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&'a str>> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim_end()) {
            (None, fence) if fence == format!("```{language}") => current = Some(Vec::new()),
            (Some(lines), "```") => {
                let mut block = lines.join("\n");
                block.push('\n');
                blocks.push(block);
                current = None;
            }
            (Some(lines), _) => lines.push(line),
            (None, _) => {}
        }
    }
    blocks
}
