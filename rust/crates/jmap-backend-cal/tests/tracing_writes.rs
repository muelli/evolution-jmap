// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured tracing tests for `jmap-backend-cal` — Track B1 follow-up.
//!
//! Asserts structured fields attached to `tracing` events during calendar
//! connection, component listing, change diffing, component loading, saving,
//! removal, free-busy querying, and color change pushing.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Arc, Mutex};

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_REJECTED,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_SECURITY, ECalComponent, ESource,
    e_cal_component_new_from_string, e_cal_meta_backend_info_free,
    e_source_authentication_set_host, e_source_authentication_set_port, e_source_get_extension,
    e_source_new_with_uid, e_source_security_set_secure,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_free, g_slist_free, g_slist_free_full, g_slist_prepend, gchar,
};
use gobject_sys::g_object_unref;
use jmap_backend_cal::connect::{self, open_calendar};
use jmap_backend_cal::ops::{self, Outcome};
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_client::Credentials;
use jmap_mock::MockServer;
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

const NEW_EVENT: &str = "BEGIN:VEVENT\r\n\
                         UID:20260810T090000-1234@evolution\r\n\
                         SUMMARY:Standup\r\n\
                         DTSTART:20260810T070000Z\r\n\
                         DURATION:PT30M\r\n\
                         END:VEVENT\r\n";

const WINDOW_START: i64 = 1_788_220_800;
const WINDOW_END: i64 = 1_788_307_200;

struct CapturingSubscriber {
    captured: Arc<Mutex<Vec<(Level, String, String)>>>,
}

struct Recorder<'a> {
    level: Level,
    sink: &'a Mutex<Vec<(Level, String, String)>>,
}

impl Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> SpanId {
        SpanId::from_u64(1)
    }

    fn record(&self, _span: &SpanId, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &SpanId, _follows: &SpanId) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut Recorder {
            level: *event.metadata().level(),
            sink: &self.captured,
        });
    }

    fn enter(&self, _span: &SpanId) {}

    fn exit(&self, _span: &SpanId) {}
}

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn capture(run: impl FnOnce()) -> Vec<(Level, String, String)> {
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run();
    });
    std::mem::take(&mut *captured.lock().unwrap())
}

fn untraced<T>(run: impl FnOnce() -> T) -> T {
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let subscriber = CapturingSubscriber {
        captured: Arc::new(Mutex::new(Vec::new())),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run()
    })
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

struct Fixture {
    server: MockServer,
    calendar_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let calendar_id = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            account.seed_calendar("Personal", true)
        };
        Self {
            server,
            calendar_id,
        }
    }

    fn config(&self) -> SourceConfig {
        SourceConfig {
            target: ConnectTarget::Origin(self.server.origin().to_owned()),
            user: None,
            resource_id: Some(self.calendar_id.to_string()),
        }
    }

    fn sync(&self) -> jmap_cal_sync::CalSync {
        untraced(|| open_calendar(&self.config(), Credentials::none()).expect("connected"))
    }
}

struct TestSource(*mut ESource);

impl TestSource {
    fn new(origin: &str) -> Self {
        let uid = CString::new("jmap-tracing-cal-source").unwrap();
        let mut error = ptr::null_mut();
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null());

        let (host, port) = origin
            .trim_start_matches("http://")
            .split_once(':')
            .expect("the mock origin has a port");
        let host = CString::new(host).unwrap();
        unsafe {
            let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
            e_source_authentication_set_host(auth.cast(), host.as_ptr());
            e_source_authentication_set_port(auth.cast(), port.parse().unwrap());
            let sec = e_source_get_extension(source, E_SOURCE_EXTENSION_SECURITY.as_ptr());
            e_source_security_set_secure(sec.cast(), 0);
        }
        Self(source)
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn instance(vevent: &str) -> *mut ECalComponent {
    let text = CString::new(vevent).unwrap();
    let component = unsafe { e_cal_component_new_from_string(text.as_ptr()) };
    assert!(!component.is_null(), "the instance did not parse");
    component
}

fn instance_list(components: &[*mut ECalComponent]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for component in components.iter().rev() {
        list = unsafe { g_slist_prepend(list, component.cast()) };
    }
    list
}

unsafe fn drop_instances(list: *mut GSList, components: &[*mut ECalComponent]) {
    unsafe {
        g_slist_free(list);
        for component in components {
            g_object_unref(component.cast());
        }
    }
}

#[test]
fn open_calendar_traces_account_id_and_calendar_id() {
    let fixture = Fixture::start();
    let config = fixture.config();

    let captured = capture(|| {
        let _ = open_calendar(&config, Credentials::none()).expect("opened");
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in open_calendar, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in open_calendar, got {captured:?}"
    );
}

#[test]
fn list_existing_traces_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let captured = capture(|| {
        let result = unsafe { ops::list_existing(&sync, &mut sync_tag, &mut objects, &mut error) };
        assert_eq!(result, GTRUE);
    });

    unsafe {
        if !sync_tag.is_null() {
            g_free(sync_tag.cast());
        }
        if !objects.is_null() {
            g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
        }
    }

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in list_existing, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in list_existing, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "state"),
        "expected state field in list_existing, got {captured:?}"
    );
}

#[test]
fn get_changes_traces_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    // First, list existing to get the state tag under untraced guard
    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    untraced(|| unsafe {
        ops::list_existing(&sync, &mut sync_tag, &mut objects, &mut error);
        if !objects.is_null() {
            g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
        }
    });
    assert!(!sync_tag.is_null());

    let mut new_sync_tag: *mut gchar = ptr::null_mut();
    let mut repeat = GFALSE;
    let mut created: *mut GSList = ptr::null_mut();
    let mut modified: *mut GSList = ptr::null_mut();
    let mut removed: *mut GSList = ptr::null_mut();

    let captured = capture(|| {
        let outcome = unsafe {
            ops::get_changes(
                &sync,
                sync_tag,
                GFALSE,
                &mut new_sync_tag,
                &mut repeat,
                &mut created,
                &mut modified,
                &mut removed,
                &mut error,
            )
        };
        assert!(matches!(outcome, Outcome::Reported));
    });

    unsafe {
        g_free(sync_tag.cast());
        if !new_sync_tag.is_null() {
            g_free(new_sync_tag.cast());
        }
        if !modified.is_null() {
            g_slist_free_full(modified, Some(e_cal_meta_backend_info_free));
        }
        if !removed.is_null() {
            g_slist_free_full(removed, Some(e_cal_meta_backend_info_free));
        }
    }

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in get_changes, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in get_changes, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "last_sync_tag"),
        "expected last_sync_tag in get_changes, got {captured:?}"
    );
}

#[test]
fn save_load_and_remove_component_trace_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let components = [instance(NEW_EVENT)];
    let list = instance_list(&components);

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let captured_save = capture(|| {
        let result = unsafe {
            ops::save_component(
                &sync,
                GFALSE,
                list,
                ptr::null_mut(),
                &mut new_uid,
                &mut extra,
                &mut error,
            )
        };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_save,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in save_component, got {captured_save:?}"
    );
    assert!(
        has(
            &captured_save,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in save_component, got {captured_save:?}"
    );
    assert!(
        has(&captured_save, Level::DEBUG, "overwrite_existing", "false"),
        "expected overwrite_existing in save_component, got {captured_save:?}"
    );

    assert!(!new_uid.is_null());
    let uid_str = unsafe { CStr::from_ptr(new_uid).to_str().unwrap() };

    let mut loaded_component = ptr::null_mut();
    let captured_load = capture(|| {
        let result = unsafe {
            ops::load_component(
                &sync,
                new_uid,
                &mut loaded_component,
                &mut extra,
                &mut error,
            )
        };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_load,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in load_component, got {captured_load:?}"
    );
    assert!(
        has(
            &captured_load,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in load_component, got {captured_load:?}"
    );
    assert!(
        has(&captured_load, Level::DEBUG, "uid", uid_str),
        "expected uid in load_component, got {captured_load:?}"
    );

    let captured_remove = capture(|| {
        let result = unsafe { ops::remove_component(&sync, new_uid, &mut error) };
        assert_eq!(result, GTRUE);
    });

    assert!(
        has(
            &captured_remove,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in remove_component, got {captured_remove:?}"
    );
    assert!(
        has(
            &captured_remove,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in remove_component, got {captured_remove:?}"
    );
    assert!(
        has(&captured_remove, Level::DEBUG, "uid", uid_str),
        "expected uid in remove_component, got {captured_remove:?}"
    );

    unsafe {
        g_free(new_uid.cast());
        if !loaded_component.is_null() {
            jmap_backend_cal::marshal::component_unref(loaded_component);
        }
        drop_instances(list, &components);
    }
}

#[test]
fn connect_from_source_traces_structured_fields() {
    let fixture = Fixture::start();
    let source = TestSource::new(fixture.server.origin());
    let mut auth_result = E_SOURCE_AUTHENTICATION_REJECTED;
    let mut error = ptr::null_mut();

    let captured = capture(|| {
        let sync = unsafe {
            connect::connect(
                source.0,
                ptr::null(),
                ptr::null_mut(),
                &mut auth_result,
                &mut error,
            )
        };
        assert!(sync.is_some());
    });

    assert_eq!(auth_result, E_SOURCE_AUTHENTICATION_ACCEPTED);
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in connect, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in connect, got {captured:?}"
    );
}

#[test]
fn get_free_busy_traces_structured_fields() {
    let fixture = Fixture::start();
    {
        let state = fixture.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&fixture.server.account_id()).unwrap();
        account.seed_principal(jmap_proto::principals::Principal {
            principal_type: Some("individual".to_owned()),
            name: "bob@example.com".to_owned(),
            email: Some("bob@example.com".to_owned()),
            ..Default::default()
        });
    }

    let sync = fixture.sync();
    let user_cstr = CString::new("bob@example.com").unwrap();
    let mut user_list = ptr::null_mut();
    user_list = unsafe { g_slist_prepend(user_list, user_cstr.as_ptr() as *mut _) };

    let mut out: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let captured = capture(|| {
        let outcome = unsafe {
            ops::get_free_busy(
                &sync,
                user_list,
                WINDOW_START,
                WINDOW_END,
                &mut out,
                &mut error,
            )
        };
        assert!(matches!(outcome, ops::FreeBusyOutcome::Reported));
    });

    unsafe {
        g_slist_free(user_list);
        if !out.is_null() {
            g_slist_free_full(out, Some(g_free));
        }
    }

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in get_free_busy, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in get_free_busy, got {captured:?}"
    );
    assert!(
        captured
            .iter()
            .any(|(lvl, name, _)| *lvl == Level::DEBUG && name == "user_count"),
        "expected user_count in get_free_busy, got {captured:?}"
    );
}

#[test]
fn on_source_changed_traces_structured_fields() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let captured = capture(|| {
        let outcome = ops::on_source_changed(&sync, Some("#ff00ff"), Some("#62a0ea"));
        assert!(matches!(outcome, ops::ColorOutcome::Pushed(_)));
    });

    assert!(
        has(
            &captured,
            Level::DEBUG,
            "account_id",
            fixture.server.account_id().as_ref()
        ),
        "expected account_id in on_source_changed, got {captured:?}"
    );
    assert!(
        has(
            &captured,
            Level::DEBUG,
            "calendar_id",
            fixture.calendar_id.as_ref()
        ),
        "expected calendar_id in on_source_changed, got {captured:?}"
    );
}
