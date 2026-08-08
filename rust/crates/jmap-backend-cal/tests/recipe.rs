// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M4's last acceptance criterion, like M3's, is "a documented manual test
// recipe with a hand-written `.source` keyfile" — and a recipe is prose, which
// nothing else in this repository fails over. The symptom of a wrong one is a
// calendar Evolution never even tries to open, with no error anywhere, and a
// reader who concludes the backend is broken.
//
// So the keyfile is a file — `docs/examples/jmap-mock-calendar.source`, the one
// the recipe says to copy — and these tests read it the way the registry does,
// through `e_server_side_source_new`, which needs neither a bus nor a running
// daemon. The shape is `jmap-backend-book`'s `tests/recipe.rs`; what is
// calendar-specific is which extension group carries the `BackendName`, because
// that group is how EDS decides a source is a *calendar* and not a task list —
// and the factory answers for events alone.
//
// Not repeated from the book's recipe test: the `Method=none` versus
// `Secure=true` pair. That one is about `SourceConfig`, which both backends
// share verbatim, and it is pinned once where it lives.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_MEMO_LIST, E_SOURCE_EXTENSION_TASK_LIST,
    ESource, ESourceBackend, ESourceRegistryServer, I_CAL_VEVENT_COMPONENT,
    I_CAL_VJOURNAL_COMPONENT, I_CAL_VTODO_COMPONENT, ICalComponentKind, e_server_side_source_new,
    e_source_backend_get_backend_name, e_source_calendar_get_type, e_source_get_extension,
    e_source_has_extension, e_source_memo_list_get_type, e_source_registry_server_new,
    e_source_task_list_get_type, g_file_new_for_path, g_object_unref,
};
use jmap_backend_cal::factory::{COMPONENT_KIND, FACTORY_NAME};
use jmap_backend_core::source::SourceConfig;

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-cal-backend.md")
}

fn keyfile() -> PathBuf {
    repository().join("docs/examples/jmap-mock-calendar.source")
}

/// EDS's own table of which `ESource` extension carries the `BackendName` for
/// each kind of calendar component, as `e_data_cal_factory` reads it.
///
/// It is a table and not a constant because it is the thing the recipe can get
/// wrong: the group name and [`COMPONENT_KIND`] have to agree, and neither one
/// alone says so. A `[Task List]` group asking for `BackendName=jmap` is looked
/// up as `"jmap:VTODO"`, which no factory in the module claims — the source
/// parses, the registry publishes it, and nothing ever opens it.
fn extension_for(kind: ICalComponentKind) -> Option<&'static CStr> {
    match kind {
        I_CAL_VEVENT_COMPONENT => Some(E_SOURCE_EXTENSION_CALENDAR),
        I_CAL_VTODO_COMPONENT => Some(E_SOURCE_EXTENSION_TASK_LIST),
        I_CAL_VJOURNAL_COMPONENT => Some(E_SOURCE_EXTENSION_MEMO_LIST),
        _ => None,
    }
}

/// Registers the three calendar-ish source extensions.
///
/// `e_source_has_extension` answers out of the table the source built while
/// parsing, and a group whose extension type was never registered leaves no
/// entry in it. Without this, "the keyfile has no `[Task List]` group" and "the
/// test binary never mentioned task lists" are the same answer.
fn register_calendar_extensions() {
    // SAFETY: no arguments, and the EDS type system initialises itself.
    unsafe {
        e_source_calendar_get_type();
        e_source_task_list_get_type();
        e_source_memo_list_get_type();
    }
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
    /// neither owns a bus name nor reads the user's sources; that only happens
    /// when it is run.
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

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: the source is alive, and the name is a header constant.
        unsafe { e_source_has_extension(self.source, name.as_ptr()) != glib_sys::GFALSE }
    }

    /// What the group named `extension` says the backend is called.
    fn backend_name(&self, extension: &CStr) -> Option<String> {
        // SAFETY: the source owns the extension, which it built while parsing;
        // `ESourceCalendar` and its siblings derive from `ESourceBackend`.
        let name = unsafe {
            let extension = e_source_get_extension(self.source, extension.as_ptr());
            e_source_backend_get_backend_name(extension.cast::<ESourceBackend>())
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
    // No user, on purpose: a source that names one makes the backend ask EDS
    // for a password before it sends anything, and the recipe's first run would
    // be a prompt rather than a connection.
    assert_eq!(config.user, None);
    // No `[Resource]` group either. The calendar `jmap-mockd` seeds gets its id
    // at startup, so the recipe cannot name one; the backend resolves "the
    // account's default calendar" instead, which the mock flags.
    assert_eq!(config.resource_id, None);
}

#[test]
fn the_recipes_keyfile_names_the_backend_the_factory_answers_to() {
    register_calendar_extensions();
    let source = RegistrySource::load(&keyfile());

    // `evolution-calendar-factory` matches this string — together with the
    // component kind below — against every registered `ECalBackendFactory`. A
    // mismatch is silent: no factory claims the source, and the calendar simply
    // never opens.
    assert_eq!(
        source.backend_name(E_SOURCE_EXTENSION_CALENDAR).as_deref(),
        Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
    );
}

#[test]
fn the_recipes_keyfile_is_a_calendar_and_not_a_task_or_memo_list() {
    register_calendar_extensions();
    let source = RegistrySource::load(&keyfile());

    // The half of the factory's key the keyfile spells by choosing a group.
    // `[Calendar]` is right because `COMPONENT_KIND` is `VEVENT`; if the
    // backend ever grows a second factory, this is the assertion that says
    // which document has to grow a second keyfile.
    assert_eq!(
        extension_for(COMPONENT_KIND),
        Some(E_SOURCE_EXTENSION_CALENDAR),
        "the recipe documents a `[Calendar]` group, so the factory has to serve \
         events"
    );
    assert!(source.has_extension(E_SOURCE_EXTENSION_CALENDAR));
    // And the two the module deliberately registers no factory for. Written as
    // a keyfile the reader can copy, either would produce a source that parses,
    // publishes, and never opens.
    assert!(!source.has_extension(E_SOURCE_EXTENSION_TASK_LIST));
    assert!(!source.has_extension(E_SOURCE_EXTENSION_MEMO_LIST));
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
        "docs/manual-test-cal-backend.md and docs/examples/jmap-mock-calendar.source disagree"
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
