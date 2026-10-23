// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
// Verifies manual test recipe keyfile parsing and SourceConfig recognition.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, ESource, ESourceBackend, e_source_address_book_get_type,
    e_source_backend_get_backend_name, e_source_get_extension,
};
use jmap_backend_book::factory::FACTORY_NAME;
use jmap_backend_core::source::{ConnectTarget, RegistrySource, SourceConfig};

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let relative = manifest_dir.join("../../..");
    if relative.join("docs/manual-test-book-backend.md").exists() {
        return relative
            .canonicalize()
            .expect("the crate directory is inside the repository");
    }
    if let Ok(pwd) = std::env::var("PWD") {
        let mut cur = PathBuf::from(pwd);
        loop {
            if cur.join("docs/manual-test-book-backend.md").exists() {
                return cur.canonicalize().unwrap_or(cur);
            }
            if !cur.pop() {
                break;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur = cwd;
        loop {
            if cur.join("docs/manual-test-book-backend.md").exists() {
                return cur.canonicalize().unwrap_or(cur);
            }
            if !cur.pop() {
                break;
            }
        }
    }
    relative
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-book-backend.md")
}

fn keyfile() -> PathBuf {
    repository().join("docs/examples/jmap-mock.source")
}

/// A documented `.source` file, loaded as `evolution-source-registry` loads it.
struct Example(RegistrySource);

impl Example {
    fn load(path: &Path) -> Self {
        Self(RegistrySource::load(path))
    }

    fn as_ptr(&self) -> *mut ESource {
        self.0.as_ptr()
    }

    fn config(&self) -> SourceConfig {
        // SAFETY: the source is alive for the duration of the call.
        unsafe { SourceConfig::from_source(self.as_ptr()) }
            .expect("the documented account is a valid one")
    }

    /// What the `[Address Book]` group says the backend is called.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: Registers ESourceAddressBook GType for extension lookup.
        let name = unsafe {
            e_source_address_book_get_type();
            let extension =
                e_source_get_extension(self.as_ptr(), E_SOURCE_EXTENSION_ADDRESS_BOOK.as_ptr())
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

#[test]
fn the_recipes_keyfile_describes_the_mock_server() {
    let config = Example::load(&keyfile()).config();

    // `jmap-mockd`'s default port, in the clear, which `SourceConfig` allows
    // for loopback and nothing else.
    assert_eq!(
        config.target,
        ConnectTarget::Origin("http://127.0.0.1:8080".into())
    );
    // Omit user. A configured user makes the backend prompt EDS for a password.
    // `jmap-mockd` without `--basic` or `--bearer` requires no credentials.
    assert_eq!(config.user, None);
    // Omit `[Resource]` group. The seed address book ID is assigned at startup.
    // The backend resolves the default address book instead.
    assert_eq!(config.resource_id, None);
}

#[test]
fn the_recipes_keyfile_names_the_backend_the_factory_answers_to() {
    // `evolution-addressbook-factory` matches this string against the
    // `factory_name` of every registered `EBookBackendFactory`. A mismatch is
    // silent: no factory claims the source, and the address book simply never
    // opens.
    assert_eq!(
        Example::load(&keyfile()).backend_name().as_deref(),
        Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
    );
}

#[test]
fn the_keyfile_spelling_that_turns_tls_on_is_method_not_secure() {
    // Pins `Method=tls` vs invalid `Secure=true` against a remote host.
    // Setting `Secure=true` sets no known EDS property, falling back to no TLS.
    let keyfile = fs::read_to_string(keyfile()).expect("the keyfile is in docs/examples/");
    let remote = keyfile.replace("Host=127.0.0.1", "Host=jmap.example.com");

    let tls = TemporaryKeyfile::holding(&remote.replace("Method=none", "Method=tls"));
    assert_eq!(
        Example::load(tls.path()).config().target,
        ConnectTarget::Origin("https://jmap.example.com:8080".into())
    );

    let mistake = TemporaryKeyfile::holding(&remote.replace("Method=none", "Secure=true"));
    // SAFETY: the source is alive for the duration of the call.
    let source = Example::load(mistake.path());
    assert!(
        matches!(
            unsafe { SourceConfig::from_source(source.as_ptr()) },
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
        // File name is unique per test process in target temp directory.
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
