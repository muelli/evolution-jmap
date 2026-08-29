// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `[JMAP OAuth2]` extension, against real `ESource`s — both through this
// crate's own `apply`/`read` door and through the raw GObject property API,
// which is the door EDS's own `.source` (de)serialisation uses. The second
// is what actually proves the properties are wired: a typo'd name or a
// missing flag would still let `apply`/`read` agree with themselves while
// persisting nothing, which is exactly the "nearly silent" failure
// `jmap_mail::settings` warns about for the same shape of code.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use eds_sys::{
    ESource, ESourceExtension, e_source_extension_ref_source, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid, e_source_parameter_to_key, e_source_to_string,
};
use glib_sys::{GFALSE, g_free};
use gobject_sys::{
    G_TYPE_STRING, GObject, GParamSpec, GValue, g_object_get_property, g_object_set_property,
    g_object_unref, g_signal_connect_data, g_value_get_string, g_value_init, g_value_set_string,
    g_value_unset,
};
use jmap_config::oauth2::{Config, EXTENSION_NAME, PROPERTY_NAMES, apply, client_id, read};

/// Writes one string property through the raw GObject API — the door EDS's
/// own `.source` parser uses, and the one the re-entrancy handlers below take
/// because a `notify` handler is handed the `GObject`, not the `ESource`.
///
/// # Safety
///
/// `object` must be a live instance carrying a `G_TYPE_STRING` property of
/// that name.
unsafe fn set_string_property(object: *mut GObject, name: &CStr, value: &CStr) {
    // SAFETY: the contract above; the `GValue` is initialised, filled and
    // unset here, and `value` outlives the copy `g_value_set_string` makes.
    unsafe {
        let mut gvalue: GValue = std::mem::zeroed();
        g_value_init(&mut gvalue, G_TYPE_STRING);
        g_value_set_string(&mut gvalue, value.as_ptr());
        g_object_set_property(object, name.as_ptr(), &gvalue);
        g_value_unset(&mut gvalue);
    }
}

/// `g_signal_connect`, spelled out — the same shape `jmap-mail`'s own test
/// helper uses, for the same reason: the macro is C-side only.
///
/// # Safety
///
/// `handler` must have the signature `signal` declares, and `data` must stay
/// alive for as long as emissions can reach it.
unsafe fn connect(instance: *mut GObject, signal: &CStr, handler: *const (), data: *mut c_void) {
    // SAFETY: the contract above. The transmute to `GCallback` is what every
    // `g_signal_connect` in C is; the marshaller casts it back to the
    // signature the signal declares.
    let id = unsafe {
        g_signal_connect_data(
            instance,
            signal.as_ptr(),
            Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                handler,
            )),
            data,
            None,
            0,
        )
    };
    assert_ne!(id, 0, "nothing connected to {signal:?}");
}

/// An `ESource` pointer moved to another thread on purpose.
///
/// `ESource` is one of EDS's thread-safe types — the extension table every
/// call below goes through is behind its own `GRecMutex`, and this crate's own
/// storage is behind a `Mutex` — which is the whole premise `oauth2::borrowed`
/// documents when it names "whatever thread the `GDBusProxy` notify arrives
/// on". These tests are where that premise is exercised rather than asserted.
#[derive(Clone, Copy)]
struct Shared(*mut ESource);

impl Shared {
    /// The pointer, reached through a `self` method so that a `move` closure
    /// captures the whole wrapper rather than the bare field inside it —
    /// which, under edition 2021's per-field capture, is exactly the
    /// `unsafe impl Send` this type exists for being skipped.
    fn source(self) -> *mut ESource {
        self.0
    }
}

// SAFETY: see the type's own comment.
unsafe impl Send for Shared {}

struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // In the real module this is done once, at load time, alongside the
        // rest of this project's types — see `oauth2::ensure_registered`'s own
        // doc comment for why an unregistered type is invisible to EDS's own
        // keyfile parser, not only to this crate's `apply`/`read`. Called here
        // because this test also drives the extension through the raw
        // GObject property API, which — unlike `apply`/`read` — does not
        // register the type itself.
        jmap_config::oauth2::ensure_registered();

        let uid = CString::new("jmap-account").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn written(self, config: &Config) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, config) };
        self
    }

    fn config(&self) -> Config {
        // SAFETY: a live source.
        unsafe { read(self.0) }
    }

    /// Reads a property by name through the raw GObject API — the same path
    /// EDS's own keyfile writer uses, and not the one [`Self::config`] takes.
    fn property(&self, name: &str) -> Option<String> {
        let name = CString::new(name).expect("no NUL in a literal");
        // SAFETY: a live extension instance and a freshly initialised GValue
        // of the type every property here declares; `g_object_get_property`
        // fills it and the value is read out before it is unset.
        unsafe {
            let extension = self.extension();
            let mut value: GValue = std::mem::zeroed();
            g_value_init(&mut value, G_TYPE_STRING);
            g_object_get_property(extension.cast(), name.as_ptr(), &mut value);
            let text = g_value_get_string(&value);
            let result =
                (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned());
            g_value_unset(&mut value);
            result
        }
    }

    /// Writes a property by name through the raw GObject API.
    fn set_property(&self, name: &str, value: &str) {
        let name = CString::new(name).expect("no NUL in a literal");
        let value_c = CString::new(value).expect("no NUL in a literal");
        // SAFETY: a live extension instance, and two NUL-terminated strings
        // that outlive the call.
        unsafe { set_string_property(self.extension(), &name, &value_c) };
    }

    /// The extension instance itself, for the two raw-property helpers above.
    fn extension(&self) -> *mut GObject {
        // SAFETY: a live source; `EXTENSION_NAME` is the group this crate's
        // extension registers under.
        unsafe { e_source_get_extension(self.0, EXTENSION_NAME.as_ptr()).cast() }
    }

    /// The source as EDS itself would write it to disk — the `.source`
    /// keyfile an Evolution restart reads back.
    fn serialised(&self) -> String {
        // SAFETY: a live source; `e_source_to_string` is `(transfer full)`
        // and the length out-parameter is optional, so the NUL-terminated
        // result is read and freed here.
        unsafe {
            let text = e_source_to_string(self.0, ptr::null_mut());
            assert!(!text.is_null(), "e_source_to_string returned NULL");
            let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
            g_free(text.cast());
            owned
        }
    }

    fn has_extension(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_has_extension(self.0, EXTENSION_NAME.as_ptr()) != GFALSE }
    }

    /// The `const gchar *` `EOAuth2Service::get_client_id` hands EDS — the
    /// borrowed door, not [`Self::config`]'s owned one.
    fn borrowed_client_id(&self) -> *const c_char {
        // SAFETY: a live source.
        unsafe { client_id(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn config() -> Config {
    Config {
        client_id: Some("client-abc123".to_owned()),
        client_secret: Some("s3cret".to_owned()),
        authorization_endpoint: Some("https://jmap.example.com/authorize".to_owned()),
        token_endpoint: Some("https://jmap.example.com/token".to_owned()),
        redirect_uri: Some("https://client.example.org/callback".to_owned()),
        scope: Some("urn:ietf:params:oauth:scope:mail offline_access".to_owned()),
        resource: Some("https://jmap.example.com/session".to_owned()),
    }
}

#[test]
fn a_source_that_says_nothing_reads_as_a_config_with_nothing_in_it() {
    let source = TestSource::new();

    assert_eq!(source.config(), Config::default());
}

#[test]
fn reading_a_config_does_not_add_the_extension() {
    let source = TestSource::new();

    let _ = source.config();

    assert!(
        !source.has_extension(),
        "reading created the [JMAP OAuth2] group"
    );
}

#[test]
fn a_config_that_was_written_is_the_config_that_is_read_back() {
    let source = TestSource::new().written(&config());

    assert_eq!(source.config(), config());
}

#[test]
fn committing_a_config_that_dropped_its_secret_clears_the_one_that_was_there() {
    let source = TestSource::new().written(&config());

    let second = Config {
        client_secret: None,
        ..config()
    };
    let source = source.written(&second);

    assert_eq!(source.config(), second);
}

#[test]
fn every_field_is_reachable_through_the_gobject_property_it_was_installed_as() {
    // The property names `class_init` installed, matched against the fields
    // `apply` writes — proof the two agree, rather than each independently
    // claiming to.
    let source = TestSource::new().written(&config());

    assert_eq!(
        source.property("client-id").as_deref(),
        Some("client-abc123")
    );
    assert_eq!(source.property("client-secret").as_deref(), Some("s3cret"));
    assert_eq!(
        source.property("authorization-endpoint").as_deref(),
        Some("https://jmap.example.com/authorize")
    );
    assert_eq!(
        source.property("token-endpoint").as_deref(),
        Some("https://jmap.example.com/token")
    );
    assert_eq!(
        source.property("redirect-uri").as_deref(),
        Some("https://client.example.org/callback")
    );
    assert_eq!(
        source.property("scope").as_deref(),
        Some("urn:ietf:params:oauth:scope:mail offline_access")
    );
    assert_eq!(
        source.property("resource").as_deref(),
        Some("https://jmap.example.com/session")
    );
}

#[test]
fn a_value_set_through_the_gobject_property_is_read_back_through_this_crates_own_door() {
    // The direction EDS's own `.source` parser writes in: restoring an
    // account from disk sets properties, it does not call `apply`.
    let source = TestSource::new();
    source.set_property("client-id", "restored-client-id");

    assert_eq!(
        source.config().client_id.as_deref(),
        Some("restored-client-id")
    );
}

// ---------------------------------------------------------------------------
// The lifetime `oauth2::borrowed` promises the `EOAuth2Service` vtable.
//
// Those vfuncs return `const gchar *` into this extension's own storage, and
// EDS's callers use the pointer a few instructions later — `e-oauth2-
// service.c` passes each one straight into `e_oauth2_service_util_set_to_
// form` or `eos_create_soup_message`. Nothing in that path takes a lock, and
// nothing gives the vfunc a chance to free anything afterwards, so the only
// contract that can hold is the one `borrowed`'s doc states: the pointer
// stays valid for as long as the extension does. A write that lands in
// between — `e-source.c`'s `source_parse_dbus_data` → `source_load_from_key_
// file` → `g_object_set_property`, which re-runs every
// `E_SOURCE_PARAM_SETTING` property whenever the registry pushes new data,
// on whatever thread the `GDBusProxy` notify arrives on — must therefore not
// invalidate it.
//
// This is the discipline EDS's own OAuth2 services keep:
// `e-oauth2-service-google.c`'s `eos_google_get_client_id` answers either a
// `static gchar glob_buff[128]` or a value `eos_google_read_settings` caches
// with `g_object_set_data_full` behind an `if (!value)` guard — written once
// and never replaced while the service lives.

/// Reads a borrowed `const gchar *` back the way EDS's own callers do.
///
/// # Safety
///
/// `pointer` is NULL or points at a NUL-terminated string that is still
/// alive — which is exactly what these tests exist to establish, so a
/// failure here is the finding rather than a misuse.
unsafe fn text_at(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    })
}

/// Same-sized allocations, made between a free and a read so that a reused
/// heap block shows up as changed bytes instead of passing on a stale one
/// nothing happened to overwrite.
fn churn_the_allocator(like: &str) -> Vec<CString> {
    let filler = "X".repeat(like.len());
    (0..256)
        .map(|_| CString::new(filler.as_str()).expect("no NUL in a literal"))
        .collect()
}

#[test]
fn rewriting_a_field_with_the_same_value_keeps_the_pointer_it_handed_out() {
    // `apply` is documented as an idempotent rewrite that runs again every
    // time discovery or registration is redone, so "the same value, written
    // again" is its ordinary case — and re-seating the storage under a
    // pointer EDS may still be holding is not something that rewrite is
    // entitled to do.
    let source = TestSource::new().written(&config());
    let first = source.borrowed_client_id();

    let source = source.written(&config());

    assert_eq!(
        first,
        source.borrowed_client_id(),
        "an unchanged rewrite moved the storage a C caller may still point into"
    );
}

#[test]
fn setting_a_property_to_the_value_it_already_has_keeps_the_pointer_it_handed_out() {
    // The same rule through EDS's own door. `source_set_property_from_key_
    // file` skips a set whose value did not differ, so this case is rarer
    // from that caller than from `apply` — but `g_object_set_property` is
    // public API and nothing stops any other caller repeating a value.
    let source = TestSource::new().written(&config());
    let first = source.borrowed_client_id();

    source.set_property("client-id", "client-abc123");

    assert_eq!(
        first,
        source.borrowed_client_id(),
        "re-setting a property to its current value moved the storage under it"
    );
}

#[test]
fn a_pointer_handed_out_before_a_changed_write_still_reads_its_original_bytes() {
    // The race itself, made deterministic: EDS asks for the client id, and
    // before it has copied the answer the registry pushes a source update
    // that changes that very field. The pointer EDS is holding has to
    // survive it.
    let source = TestSource::new().written(&config());
    let handed_out = source.borrowed_client_id();

    source.set_property("client-id", "client-def456");
    let churn = churn_the_allocator("client-abc123");

    // SAFETY: the pointer is expected to still be live — see this function's
    // own point.
    assert_eq!(
        unsafe { text_at(handed_out) }.as_deref(),
        Some("client-abc123"),
        "a write freed the string a C caller was still pointing at"
    );
    drop(churn);
}

#[test]
fn a_pointer_handed_out_survives_writes_arriving_on_another_thread() {
    // The same rule as the test above, minus the part that made it easy. The
    // registry pushes source updates on whatever thread the `GDBusProxy`
    // notify lands on, while `EOAuth2Service`'s vfuncs answer on whichever
    // thread wanted a token, so the two really are concurrent — and the
    // storage a retired value moves into is a `Vec`, which reallocates.
    // Moving a `CString` moves the pointer and not the bytes, so a pointer
    // into a retired value has to survive every one of those reallocations.
    let source = TestSource::new().written(&config());
    let handed_out = source.borrowed_client_id();

    let shared = Shared(source.0);
    let writer = std::thread::spawn(move || {
        for round in 0..2000_u32 {
            let rotating = Config {
                client_id: Some(format!("rotating-{round}")),
                ..config()
            };
            // SAFETY: a live source, kept alive by the `TestSource` the main
            // thread joins this one before dropping.
            unsafe { apply(shared.source(), &rotating) };
        }
    });

    for _ in 0..2000 {
        // SAFETY: the pointer is expected to still be live — that is the point.
        assert_eq!(
            unsafe { text_at(handed_out) }.as_deref(),
            Some("client-abc123"),
            "a write on another thread freed or re-seated the string a C caller was still pointing at"
        );
    }

    writer.join().expect("the writing thread panicked");
}

// ---------------------------------------------------------------------------
// The three contracts this module states in prose and nothing held it to:
// that `E_SOURCE_PARAM_SETTING` is what persists these seven properties, that
// `apply` raises `notify` for each one, and that overriding
// `GObjectClass`'s two property vfuncs did not swallow the parent class's own.

/// Records the `GParamSpec` name of every `notify` emission.
///
/// # Safety
///
/// `data` must point at a live `Mutex<Vec<String>>` — a `notify` handler's
/// signature, with the user-data the connection was made with.
unsafe extern "C" fn record_notify(
    _object: *mut GObject,
    pspec: *mut GParamSpec,
    data: *mut c_void,
) {
    // SAFETY: the contract above, and `pspec` is the emission's own, alive
    // for its duration.
    unsafe {
        let seen = &*data.cast::<Mutex<Vec<String>>>();
        let name = CStr::from_ptr((*pspec).name).to_string_lossy().into_owned();
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(name);
    }
}

/// The property names `notify` fires with while `body` runs.
///
/// Panicking inside a GObject signal handler would unwind through C, so the
/// handler above only records; every assertion happens out here.
fn notifications_during(extension: *mut GObject, body: impl FnOnce()) -> Vec<String> {
    let seen: Mutex<Vec<String>> = Mutex::new(Vec::new());
    // SAFETY: `record_notify` has `notify`'s signature, and `seen` outlives
    // `body`, which is the only thing that can emit.
    unsafe {
        connect(
            extension,
            c"notify",
            record_notify as *const (),
            ptr::from_ref(&seen).cast_mut().cast::<c_void>(),
        );
    }

    body();

    seen.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The names of [`PROPERTY_NAMES`], sorted — what a full `apply` must notify.
fn every_property_name() -> Vec<String> {
    let mut names: Vec<String> = PROPERTY_NAMES
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn applying_a_config_raises_notify_for_every_property_it_wrote() {
    // `apply`'s doc gives exactly one reason for going through
    // `g_object_set_property` instead of writing the fields directly: each
    // write raises `notify`, which is what carries a re-run discovery or
    // re-registration across the `GBinding`s
    // `jmap_backend_collection::child_added` puts between an account's
    // extension and its children's. A write that bypassed the property system
    // would update the account and silently strand every child on the
    // registration it had before — no error, no log line, just children whose
    // token refresh keeps using a client id the server has forgotten.
    let source = TestSource::new();
    let extension = source.extension();

    let mut seen = notifications_during(extension, || {
        // SAFETY: a live source.
        unsafe { apply(source.0, &config()) };
    });
    seen.sort();

    assert_eq!(
        seen,
        every_property_name(),
        "`apply` did not notify every property it wrote; `child_added`'s bindings carry nothing"
    );
}

#[test]
fn an_unchanged_rewrite_still_raises_notify() {
    // `Fields::set` returns early when the value did not change — that is what
    // keeps a handed-out pointer where it was. It must not also suppress the
    // emission: `apply` is documented as an idempotent rewrite that runs again
    // whenever discovery or registration is redone, and a child bound with
    // `G_BINDING_SYNC_CREATE` that was created *after* the account was last
    // written is brought into line by exactly these redundant notifies.
    // Nothing in this class asks for `G_PARAM_EXPLICIT_NOTIFY`, so GObject
    // emits on every set; this pins that, since adding the flag would look
    // like a tidy-up and would quietly cost the re-sync.
    let source = TestSource::new().written(&config());
    let extension = source.extension();

    let seen = notifications_during(extension, || {
        // SAFETY: a live source.
        unsafe { apply(source.0, &config()) };
    });

    assert_eq!(
        seen.len(),
        PROPERTY_NAMES.len(),
        "a rewrite of the values already there notified {} of {} properties",
        seen.len(),
        PROPERTY_NAMES.len()
    );
}

#[test]
fn every_field_persists_into_the_keyfile_an_evolution_restart_reads_back() {
    // The whole reason this module stores its five — now seven — values as
    // GObject properties rather than as plain Rust state: only a property
    // flagged `E_SOURCE_PARAM_SETTING` reaches the account's `.source` file.
    // That flag is *computed* here — `e-source.h` defines it as a bare
    // `#define (1 << G_PARAM_USER_SHIFT)` with no symbol for bindgen to bind —
    // so a wrong shift, or the flag simply going missing from `class_init`,
    // would leave every test above passing while nothing survived a restart:
    // discovery and registration would silently re-run on every boot, which
    // for a deployment that rate-limits RFC 7591 registration is a good deal
    // worse than a visible failure.
    let source = TestSource::new().written(&config());

    let text = source.serialised();

    assert!(
        text.contains("[JMAP OAuth2]"),
        "the extension wrote no group into the keyfile:\n{text}"
    );
    for (property, expected) in PROPERTY_NAMES.iter().zip([
        "client-abc123",
        "s3cret",
        "https://jmap.example.com/authorize",
        "https://jmap.example.com/token",
        "https://client.example.org/callback",
        "urn:ietf:params:oauth:scope:mail offline_access",
        "https://jmap.example.com/session",
    ]) {
        // The keyfile key EDS derives from the property name — "client-id"
        // becomes "ClientId" — asked of EDS rather than spelled out here, so
        // this test states that the value persists and not what EDS's own
        // naming rule happens to be.
        // SAFETY: a NUL-terminated property name; the result is
        // `(transfer full)`.
        let key = unsafe {
            let raw = e_source_parameter_to_key(property.as_ptr());
            assert!(!raw.is_null(), "e_source_parameter_to_key returned NULL");
            let key = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw.cast());
            key
        };
        assert!(
            text.contains(&format!("{key}={expected}")),
            "{key} did not persist into the keyfile:\n{text}"
        );
    }
}

#[test]
fn overriding_the_property_vfuncs_did_not_swallow_the_parent_classes_own_source_property() {
    // `class_init` writes this class's own `set_property`/`get_property` into
    // `GObjectClass`, and installs its first property under id 1 — the same id
    // the parent `ESourceExtension` installs its construct-only `source`
    // property under. Ids are per-class, not per-hierarchy, so those two
    // genuinely collide; what keeps them apart is that GObject dispatches a
    // set through `g_type_class_peek (pspec->owner_type)` rather than through
    // the instance's own class, so the parent's `source` reaches the parent's
    // vfunc and never ours.
    //
    // If that were not so, `e_source_get_extension` would hand the extension
    // an `ESource` in a `GValue` our `set_property` would read as a string —
    // `g_value_get_string` on an object value answers NULL after a GLib
    // critical — and the extension would come out with a NULL source *and*
    // a cleared `client-id`. Both halves are asserted, since either alone
    // could be explained away.
    let source = TestSource::new().written(&config());

    let extension = source.extension().cast::<ESourceExtension>();
    // SAFETY: a live extension of this source; `ref_source` is
    // `(transfer full)`.
    let owner = unsafe { e_source_extension_ref_source(extension) };
    assert_eq!(
        owner, source.0,
        "the extension lost the `ESource` the parent class stores at property id 1"
    );
    // SAFETY: the reference `ref_source` returned.
    unsafe { g_object_unref(owner.cast()) };

    assert_eq!(
        source.config().client_id.as_deref(),
        Some("client-abc123"),
        "this class's own property id 1 was clobbered by the parent's"
    );
}

// ---------------------------------------------------------------------------
// Re-entrancy: what a `notify` handler may do while a write is still on the
// stack.
//
// This is not hypothetical for this class. Every write raises `notify`
// synchronously, inside `g_object_set_property`; `child_added` hangs
// `GBinding`s off exactly these emissions; and EDS's own source-restore path
// (`source_parse_dbus_data` -> `source_load_from_key_file` ->
// `g_object_set_property`) re-runs every `E_SOURCE_PARAM_SETTING` property
// whenever the registry pushes new data. So a handler that reads this
// extension back, or writes another of its properties, is an ordinary thing
// to happen — and the storage behind all seven is one `Mutex`, which is a
// deadlock waiting to happen if the write were to hold it across the
// emission.

/// What [`probe_from_another_thread`] carries, and what it leaves behind.
struct Probe {
    source: Shared,
    /// Only the first emission probes; the rest return at once, so a full
    /// `apply` does not spawn seven threads.
    armed: AtomicBool,
    /// `None` until the handler ran; `Some(None)` if the other thread could
    /// not take the lock in time.
    observed: Mutex<Option<Option<Config>>>,
}

/// Asks another thread to read this extension while `notify` is running, and
/// records whether it got in.
///
/// The read happens on another thread on purpose. Reading from *this* one
/// answers the same question — and
/// [`a_notify_handler_can_read_the_extension_back_on_its_own_thread`] does
/// exactly that — but a regression there expresses itself as a re-locked
/// `std::sync::Mutex`, i.e. a hang, which is a far worse test failure than an
/// assertion. A bounded wait from a second thread turns the same finding into
/// a clean one.
///
/// # Safety
///
/// `data` must point at a live [`Probe`]; the rest is `notify`'s signature.
unsafe extern "C" fn probe_from_another_thread(
    _object: *mut GObject,
    _pspec: *mut GParamSpec,
    data: *mut c_void,
) {
    // SAFETY: the contract above.
    let probe = unsafe { &*data.cast::<Probe>() };
    if !probe.armed.swap(false, Ordering::SeqCst) {
        return;
    }

    let shared = probe.source;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: a live source — the test joins nothing but outlives this,
        // and a thread still blocked on the lock is itself the finding.
        let config = unsafe { read(shared.source()) };
        let _ = sender.send(config);
    });

    let answer = receiver.recv_timeout(Duration::from_secs(10)).ok();
    *probe
        .observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(answer);
}

#[test]
fn another_thread_can_read_the_extension_while_notify_is_running() {
    let source = TestSource::new().written(&config());
    let probe = Probe {
        source: Shared(source.0),
        armed: AtomicBool::new(true),
        observed: Mutex::new(None),
    };
    // SAFETY: the handler has `notify`'s signature, and `probe` outlives every
    // emission this test causes.
    unsafe {
        connect(
            source.extension(),
            c"notify::client-id",
            probe_from_another_thread as *const (),
            ptr::from_ref(&probe).cast_mut().cast::<c_void>(),
        );
    }

    source.set_property("client-id", "client-def456");

    let observed = probe
        .observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let observed = observed.expect("the notify handler never ran");
    let observed = observed.expect(
        "a second thread could not read the extension while `notify` ran: \
         the write is holding this class's lock across the emission, which is \
         a deadlock for any handler on this thread that reads the extension back",
    );

    assert_eq!(
        observed.client_id.as_deref(),
        Some("client-def456"),
        "`notify` ran before the value it announces had landed in the storage"
    );
}

/// Where [`read_on_the_emitting_thread`] leaves what it saw.
static SEEN_BY_THE_EMITTING_THREAD: Mutex<Option<Config>> = Mutex::new(None);

/// Reads the extension back from inside its own `notify` handler, on the
/// thread that is emitting.
///
/// # Safety
///
/// `data` must point at a live `ESource`; the rest is `notify`'s signature.
unsafe extern "C" fn read_on_the_emitting_thread(
    _object: *mut GObject,
    _pspec: *mut GParamSpec,
    data: *mut c_void,
) {
    // SAFETY: the contract above.
    let config = unsafe { read(data.cast::<ESource>()) };
    *SEEN_BY_THE_EMITTING_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
}

#[test]
fn a_notify_handler_can_read_the_extension_back_on_its_own_thread() {
    // The contract stated without the second thread
    // `another_thread_can_read_the_extension_while_notify_is_running` uses to
    // keep its failure clean: all seven fields sit behind one
    // `std::sync::Mutex`, so if `notify` were emitted while the write still
    // held it, a handler that reads the extension back on the emitting thread
    // would re-lock and wedge.
    //
    // It does not, and the reason is worth writing down because it is
    // GObject's and not this module's: `g_object_set_property` is
    // `g_object_setv`, which freezes the object's notify queue around the
    // `set_property` call and thaws it afterwards, so the emission is always
    // *outside* the critical section. Established rather than read off the
    // documentation — deliberately emitting `g_object_notify` from inside
    // this class's own locked section, which is the worst a change here could
    // do, still did not wedge this test or its sibling, because that emission
    // was queued too. See `docs/AUDIT-FFI-20260828.md` section 8.
    let source = TestSource::new().written(&config());
    // SAFETY: the handler has `notify`'s signature, and the source outlives
    // every emission this test causes.
    unsafe {
        connect(
            source.extension(),
            c"notify::client-id",
            read_on_the_emitting_thread as *const (),
            source.0.cast::<c_void>(),
        );
    }

    source.set_property("client-id", "client-def456");

    let seen = SEEN_BY_THE_EMITTING_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen.expect("the notify handler never ran")
            .client_id
            .as_deref(),
        Some("client-def456"),
        "`notify` announced a value the storage did not have yet"
    );
}

/// Only the first emission writes back; without this a re-entrant write would
/// recurse for as long as GObject kept emitting.
static REENTRANT_WRITE_ARMED: AtomicBool = AtomicBool::new(true);

/// Writes a *different* property of the same extension from inside its
/// `notify` handler.
///
/// # Safety
///
/// `object` is the emitting extension; the rest is `notify`'s signature.
unsafe extern "C" fn write_another_property(
    object: *mut GObject,
    _pspec: *mut GParamSpec,
    _data: *mut c_void,
) {
    if !REENTRANT_WRITE_ARMED.swap(false, Ordering::SeqCst) {
        return;
    }
    // SAFETY: the emitting instance of this class, which installs
    // `token-endpoint` as a string.
    unsafe {
        set_string_property(
            object,
            c"token-endpoint",
            c"https://reentrant.example.com/token",
        );
    }
}

#[test]
fn a_notify_handler_can_write_another_property_of_the_same_extension() {
    // The shape `child_added`'s bindings already have — a handler on one
    // property setting another — turned on this very object. Two things have
    // to hold: the nested write must not deadlock against the outer one (this
    // class keeps all seven fields behind one `Mutex`), and it must *survive*,
    // rather than be undone when the outer write's own bookkeeping resumes.
    let source = TestSource::new().written(&config());
    // SAFETY: the handler has `notify`'s signature and takes no user-data.
    unsafe {
        connect(
            source.extension(),
            c"notify::client-id",
            write_another_property as *const (),
            ptr::null_mut(),
        );
    }

    source.set_property("client-id", "client-def456");

    let after = source.config();
    assert_eq!(
        after.client_id.as_deref(),
        Some("client-def456"),
        "the outer write was lost"
    );
    assert_eq!(
        after.token_endpoint.as_deref(),
        Some("https://reentrant.example.com/token"),
        "the write a `notify` handler made from inside the outer one did not survive it"
    );
}
