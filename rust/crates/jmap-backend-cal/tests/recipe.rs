// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//! Verification of the manual test recipe and example `.source` keyfile.

use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};

use eds_sys::{
    E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_MEMO_LIST, E_SOURCE_EXTENSION_TASK_LIST,
    ESource, ESourceBackend, I_CAL_VEVENT_COMPONENT, I_CAL_VJOURNAL_COMPONENT,
    I_CAL_VTODO_COMPONENT, ICalComponentKind, e_source_backend_get_backend_name,
    e_source_calendar_get_type, e_source_get_extension, e_source_has_extension,
    e_source_memo_list_get_type, e_source_task_list_get_type,
};
use jmap_backend_cal::factory::{COMPONENT_KIND, FACTORY_NAME};
use jmap_backend_core::source::{ConnectTarget, RegistrySource, SourceConfig};

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let relative = manifest_dir.join("../../..");
    if relative.join("docs/manual-test-cal-backend.md").exists() {
        return relative
            .canonicalize()
            .expect("the crate directory is inside the repository");
    }
    if let Ok(pwd) = std::env::var("PWD") {
        let mut cur = PathBuf::from(pwd);
        loop {
            if cur.join("docs/manual-test-cal-backend.md").exists() {
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
            if cur.join("docs/manual-test-cal-backend.md").exists() {
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
    repository().join("docs/manual-test-cal-backend.md")
}

fn keyfile() -> PathBuf {
    repository().join("docs/examples/jmap-mock-calendar.source")
}

/// Maps component kind to corresponding ESource extension name.
fn extension_for(kind: ICalComponentKind) -> Option<&'static CStr> {
    match kind {
        I_CAL_VEVENT_COMPONENT => Some(E_SOURCE_EXTENSION_CALENDAR),
        I_CAL_VTODO_COMPONENT => Some(E_SOURCE_EXTENSION_TASK_LIST),
        I_CAL_VJOURNAL_COMPONENT => Some(E_SOURCE_EXTENSION_MEMO_LIST),
        _ => None,
    }
}

/// Registers calendar-related source extension GTypes.
fn register_calendar_extensions() {
    // SAFETY: no arguments, and the EDS type system initialises itself.
    unsafe {
        e_source_calendar_get_type();
        e_source_task_list_get_type();
        e_source_memo_list_get_type();
    }
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

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: the source is alive, and the name is a header constant.
        unsafe { e_source_has_extension(self.as_ptr(), name.as_ptr()) != glib_sys::GFALSE }
    }

    /// What the group named `extension` says the backend is called.
    fn backend_name(&self, extension: &CStr) -> Option<String> {
        // SAFETY: the source owns the extension, which it built while parsing;
        // `ESourceCalendar` and its siblings derive from `ESourceBackend`.
        let name = unsafe {
            let extension = e_source_get_extension(self.as_ptr(), extension.as_ptr());
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

#[test]
fn the_recipes_keyfile_describes_the_mock_server() {
    let config = Example::load(&keyfile()).config();

    // `jmap-mockd`'s default port, in the clear, which `SourceConfig` allows
    // for loopback and nothing else.
    assert_eq!(
        config.target,
        ConnectTarget::Origin("http://127.0.0.1:8080".into())
    );
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
    let source = Example::load(&keyfile());

    // Matches factory name and component kind against registered ECalBackendFactory.
    assert_eq!(
        source.backend_name(E_SOURCE_EXTENSION_CALENDAR).as_deref(),
        Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
    );
}

#[test]
fn the_recipes_keyfile_is_a_calendar_and_not_a_task_or_memo_list() {
    register_calendar_extensions();
    let source = Example::load(&keyfile());

    // Keyfile chooses Calendar group matching VEVENT component kind.
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
