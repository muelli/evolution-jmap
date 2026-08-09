// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M6's last acceptance criterion is the same as M3's and M4's: a documented
// manual recipe with a hand-written `.source` keyfile. A recipe is prose, and
// nothing else in this repository fails when a group name in it is wrong or when
// the backend name it tells the reader to write stops being the one the factory
// answers to. For a *collection* the symptom is quieter than for an address
// book, because the registry has a factory for every account it cannot place:
// `BackendName=jmapp` is an account that simply has no children, with no error
// anywhere.
//
// So the keyfile is a file — `docs/examples/jmap-mock-collection.source`, the one
// the recipe says to copy — and these tests read it the way the registry does.
// `e_server_side_source_new` on a GFile is exactly what
// `evolution-source-registry` calls for every file in its sources directory, and
// it needs neither a bus nor a running daemon; what the reader copies is
// therefore parsed by EDS's own keyfile code and handed to the same
// `collection_source` functions `populate` calls.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_COLLECTION, ESource, ESourceBackend, ESourceRegistryServer,
    e_server_side_source_new, e_source_backend_get_backend_name, e_source_collection_get_type,
    e_source_get_extension, e_source_registry_server_new, g_file_new_for_path, g_object_unref,
};
use jmap_backend_collection::collection_source::{parts_of, server_of, user_of};
use jmap_backend_collection::factory::FACTORY_NAME;
use jmap_collection_sync::Parts;

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-collection-backend.md")
}

fn keyfile() -> PathBuf {
    repository().join("docs/examples/jmap-mock-collection.source")
}

/// A `.source` file loaded the way `evolution-source-registry` loads one.
///
/// The same stand-in as `jmap-backend-book`'s and `jmap-backend-cal`'s recipe
/// tests, and a third copy rather than a shared helper: cargo gives each
/// integration test its own binary, and the alternative — a support crate in the
/// workspace whose only user is three test files — would be a published-looking
/// crate that exists to hold thirty lines.
struct RegistrySource {
    server: *mut gobject_sys::GObject,
    source: *mut ESource,
}

impl RegistrySource {
    /// Parses `path`.
    ///
    /// `e_server_side_source_new` insists on a registry server — it is the object
    /// a server-side source reports changes to — but constructing one neither
    /// owns a bus name nor reads the user's sources; that only happens when it is
    /// run. So this is the whole daemon-free part of the daemon, and it is the
    /// part that turns a keyfile into an `ESource`.
    fn load(path: &Path) -> Self {
        let path_string =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("no NUL in a path");
        let mut error = ptr::null_mut();
        // SAFETY: the constructor takes no arguments; the path is NUL-terminated
        // and copied; the GFile is owned here and released below.
        let (server, source) = unsafe {
            // `e_source_registry_server_new` is declared as returning its own
            // base class, `EDBusServer *`, the way EDS's own callers use it; the
            // cast back down is the one C does implicitly.
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

    /// What the `[Collection]` group says the backend is called.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: referencing the extension's GType registers it, so that the
        // lookup below can find it in a test binary that has not otherwise
        // touched a collection source. Then a header constant naming an extension
        // the source owns — created rather than merely fetched, which is what
        // `collection_source` is careful not to do and what a throwaway copy of
        // the file may.
        let name = unsafe {
            e_source_collection_get_type();
            let extension =
                e_source_get_extension(self.source, E_SOURCE_EXTENSION_COLLECTION.as_ptr())
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
    let source = RegistrySource::load(&keyfile());
    // SAFETY: a source alive for the length of each call, which is all
    // `collection_source`'s functions ask for.
    let (server, user) = unsafe { (server_of(source.source), user_of(source.source)) };

    // `jmap-mockd`'s default port, in the clear, which the shared `origin` rules
    // allow for loopback and nothing else.
    assert_eq!(
        server
            .expect("the documented account names a server")
            .origin,
        "http://127.0.0.1:8080"
    );
    // No user, on purpose. An account that names one makes `populate` ask EDS to
    // resolve a password before anything is contacted, and the recipe's first run
    // would be a prompt rather than a fan-out; `jmap-mockd` with no
    // `--basic`/`--bearer` wants no credentials at all.
    assert_eq!(user, None);
}

/// The three switches that decide what a populate lists — and the reason the
/// recipe's account has mail off.
///
/// This backend creates no mail children yet, so `MailEnabled=true` would be an
/// account that claims a part nothing serves. It is not harmless prose: `mail`
/// is one of the three bits `Parts::any` is a disjunction over, so it is the
/// difference between an account whose populate contacts the server and one that
/// returns having done nothing.
#[test]
fn the_recipes_keyfile_switches_on_the_parts_the_backend_fans_out_to() {
    let source = RegistrySource::load(&keyfile());
    // SAFETY: as above.
    let parts = unsafe { parts_of(source.source) };

    assert_eq!(
        parts,
        Parts {
            mail: false,
            contacts: true,
            calendars: true,
        }
    );
    assert!(
        parts.any(),
        "an account with no part enabled is one whose populate does nothing at \
         all, which is not what this recipe is testing"
    );
}

#[test]
fn the_recipes_keyfile_names_the_backend_the_factory_answers_to() {
    // The registry files each collection factory under
    // `"<factory_name>:Collection"` and asks for the key built from this string.
    // A mismatch is silent — `e_source_registry_server_ref_backend_factory`
    // answers NULL and the account keeps sitting there with no children.
    assert_eq!(
        RegistrySource::load(&keyfile()).backend_name().as_deref(),
        Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
    );
}

#[test]
fn the_recipe_quotes_the_keyfile_verbatim() {
    // The reader copies the file, but reads the document; if the two drift apart,
    // whichever one they trusted is the wrong one.
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
        "docs/manual-test-collection-backend.md and \
         docs/examples/jmap-mock-collection.source disagree"
    );
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
