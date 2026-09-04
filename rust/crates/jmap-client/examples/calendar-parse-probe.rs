// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live differential verification for JSCalendar fidelity against a real
// JMAP server (such as Stalwart): uploads each fixture .ics file, calls
// CalendarEvent/parse, and compares the server's JSCalendar rendering beside
// jmap-ical's own parse of the same bytes, field by field.
//
// Usage:
//   cargo run -p evolution-jmap-client --example calendar-parse-probe -- \
//       <origin> <user> <password>

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_CORE};
use serde_json::{Map, Value, json};

pub const CAPABILITY_CALENDARS_PARSE: &str = "urn:ietf:params:jmap:calendars:parse";
const USING: &[&str] = &[
    CAPABILITY_CORE,
    CAPABILITY_CALENDARS,
    CAPABILITY_CALENDARS_PARSE,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff {
    pub matches: Vec<String>,
    pub differences: BTreeMap<String, (Value, Value)>,
    pub server_only: BTreeMap<String, Value>,
    pub local_only: BTreeMap<String, Value>,
}

pub fn extract_event(blob_parsed: &Value) -> Option<Map<String, Value>> {
    match blob_parsed {
        Value::Array(arr) => arr.first().and_then(Value::as_object).cloned(),
        Value::Object(obj) => Some(obj.clone()),
        _ => None,
    }
}

pub fn compare_events(
    server_obj: &Map<String, Value>,
    local_obj: &Map<String, Value>,
) -> FieldDiff {
    let mut all_keys = BTreeSet::new();
    for k in server_obj.keys() {
        all_keys.insert(k.clone());
    }
    for k in local_obj.keys() {
        all_keys.insert(k.clone());
    }

    let mut matches = Vec::new();
    let mut differences = BTreeMap::new();
    let mut server_only = BTreeMap::new();
    let mut local_only = BTreeMap::new();

    for key in all_keys {
        match (server_obj.get(&key), local_obj.get(&key)) {
            (Some(s), Some(l)) => {
                if s == l {
                    matches.push(key);
                } else {
                    differences.insert(key, (s.clone(), l.clone()));
                }
            }
            (Some(s), None) => {
                server_only.insert(key, s.clone());
            }
            (None, Some(l)) => {
                local_only.insert(key, l.clone());
            }
            (None, None) => {}
        }
    }

    FieldDiff {
        matches,
        differences,
        server_only,
        local_only,
    }
}

pub fn find_fixtures_dir() -> Option<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join("../jmap-ical/tests/fixtures"),
        PathBuf::from("rust/crates/jmap-ical/tests/fixtures"),
        PathBuf::from("crates/jmap-ical/tests/fixtures"),
        PathBuf::from("jmap-ical/tests/fixtures"),
    ];
    candidates.into_iter().find(|cand| cand.is_dir())
}

fn read_stalwart_creds() -> Option<(String, String)> {
    let home = std::env::var("HOME").ok()?;
    let creds_path = Path::new(&home).join(".config/evolution-jmap/stalwart-creds");
    let content = fs::read_to_string(creds_path).ok()?;
    let mut user = None;
    let mut pass = None;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("STALWART_USER=") {
            user = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("STALWART_PASSWORD=") {
            pass = Some(val.trim().to_string());
        }
    }
    match (user, pass) {
        (Some(u), Some(p)) => Some((u, p)),
        _ => None,
    }
}

fn format_val(val: &Value, indent: &str) -> String {
    match serde_json::to_string_pretty(val) {
        Ok(s) if s.contains('\n') => {
            let mut lines = s.lines();
            let first = lines.next().unwrap_or("");
            let rest = lines
                .map(|l| format!("{indent}{l}"))
                .collect::<Vec<_>>()
                .join("\n");
            if rest.is_empty() {
                first.to_string()
            } else {
                format!("{first}\n{rest}")
            }
        }
        _ => val.to_string(),
    }
}

fn print_diff(fixture_name: &str, diff: &FieldDiff) {
    println!(
        "--- Field comparison for {fixture_name} ({} match, {} diff, {} server-only, {} local-only) ---",
        diff.matches.len(),
        diff.differences.len(),
        diff.server_only.len(),
        diff.local_only.len(),
    );

    for m in &diff.matches {
        println!("    [=] {m}");
    }
    for (k, (s, l)) in &diff.differences {
        println!("    [!=] {k}:");
        println!("         server: {}", format_val(s, "                 "));
        println!("         local:  {}", format_val(l, "                 "));
    }
    for (k, s) in &diff.server_only {
        println!(
            "    [+] server-only: {k}: {}",
            format_val(s, "                     ")
        );
    }
    for (k, l) in &diff.local_only {
        println!(
            "    [-] local-only:  {k}: {}",
            format_val(l, "                     ")
        );
    }
}

fn probe_fixtures(client: &Client, account_id: &Id, fixtures_dir: &Path) -> (usize, usize, usize) {
    let mut fail = 0;
    let mut fixture_count = 0;
    let mut divergence_count = 0;

    let mut entries: Vec<_> = fs::read_dir(fixtures_dir)
        .expect("read fixtures directory")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "ics"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        fixture_count += 1;
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        println!("\n=== Fixture: {name} ===");

        let ics_bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL read file {name}: {e}");
                fail += 1;
                continue;
            }
        };
        let ics_text = match String::from_utf8(ics_bytes.clone()) {
            Ok(t) => t,
            Err(e) => {
                println!("FAIL decode UTF-8 {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 1. Upload .ics blob
        let upload_res = client.upload_blob(account_id, "text/calendar", ics_bytes);
        let blob_id = match upload_res {
            Ok(up) => {
                println!("PASS upload: blobId = {}", up.blob_id.as_str());
                up.blob_id
            }
            Err(e) => {
                println!("FAIL upload {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 2. Call CalendarEvent/parse
        let parse_req = json!({
            "accountId": account_id,
            "blobIds": [blob_id.clone()],
        });
        let parse_res = client.single_call(USING, "CalendarEvent/parse", &parse_req);
        let parsed_val = match parse_res {
            Ok(val) => {
                println!("PASS CalendarEvent/parse accepted");
                val
            }
            Err(e) => {
                println!("FAIL CalendarEvent/parse {name}: {e}");
                fail += 1;
                continue;
            }
        };

        let server_event_obj = parsed_val
            .get("parsed")
            .and_then(|p| p.get(blob_id.as_str()))
            .and_then(extract_event);

        let Some(server_obj) = server_event_obj else {
            let not_parsable = parsed_val.get("notParsable");
            let not_found = parsed_val.get("notFound");
            println!(
                "FAIL server returned no parsed event for {name} (notParsable: {not_parsable:?}, notFound: {not_found:?})"
            );
            fail += 1;
            continue;
        };

        // 3. Parse locally with jmap-ical
        let local_event_res = jmap_ical::ical_to_event(&ics_text);
        let local_obj = match local_event_res {
            Ok(evt) => {
                println!("PASS jmap-ical parsed locally");
                match serde_json::to_value(&evt) {
                    Ok(Value::Object(map)) => map,
                    _ => {
                        println!("FAIL serialize jmap-ical event to object for {name}");
                        fail += 1;
                        continue;
                    }
                }
            }
            Err(e) => {
                println!("FAIL jmap-ical failed to parse {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 4. Compare field by field
        let diff = compare_events(&server_obj, &local_obj);
        if !diff.differences.is_empty()
            || !diff.server_only.is_empty()
            || !diff.local_only.is_empty()
        {
            divergence_count += 1;
        }
        print_diff(&name, &diff);
    }

    (fixture_count, fail, divergence_count)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first_arg = args.next();

    if first_arg.as_deref() == Some("mock") {
        let server = jmap_mock::MockServer::builder().start();
        let account_id = server.account_id();
        let client = Client::connect(server.origin(), Credentials::none()).expect("connect mock");
        let fixtures_dir = find_fixtures_dir().expect("find fixtures directory");

        println!("Running differential harness against in-process mock server...");
        let (total, fail, divergences) = probe_fixtures(&client, &account_id, &fixtures_dir);
        println!(
            "\nSummary: {total} fixtures probed, {divergences} with divergences, {fail} check failures"
        );
        if fail > 0 {
            std::process::exit(1);
        }
        return;
    }

    let (origin, user, password) = match (first_arg, args.next(), args.next()) {
        (Some(o), Some(u), Some(p)) => (o, u, p),
        _ => {
            if let Some((u, p)) = read_stalwart_creds() {
                ("http://10.128.0.2:8080".to_string(), u, p)
            } else {
                eprintln!("usage: calendar-parse-probe <origin> <user> <password>");
                std::process::exit(2);
            }
        }
    };

    println!("Connecting to {origin} as {user}...");
    let client = match Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .rebase_urls_to_origin(true)
        .connect(&origin, Credentials::basic(user, password))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FAIL connect to {origin}: {e}");
            std::process::exit(1);
        }
    };

    let session = client.session();
    let has_cal_parse = session
        .capabilities
        .contains_key(CAPABILITY_CALENDARS_PARSE);
    println!("Capability {CAPABILITY_CALENDARS_PARSE} advertised: {has_cal_parse}");

    let account_id = if session.accounts.contains_key(&Id::from("d333333")) {
        Id::from("d333333")
    } else {
        client
            .primary_account(CAPABILITY_CALENDARS)
            .or_else(|_| client.primary_account(CAPABILITY_CORE))
            .unwrap_or_else(|_| Id::from("d333333"))
    };
    println!("Target account: {}", account_id.as_str());

    let fixtures_dir = find_fixtures_dir().expect("find fixtures directory");
    println!("Fixtures directory: {}", fixtures_dir.display());

    let (total, fail, divergences) = probe_fixtures(&client, &account_id, &fixtures_dir);
    println!(
        "\nSummary: {total} fixtures probed, {divergences} with divergences, {fail} check failures"
    );

    if fail == 0 {
        println!("\nALL CHECKS PASSED");
    } else {
        println!("\n{fail} CHECK(S) FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_event_from_array() {
        let val = json!([{"@type": "Event", "title": "Team Standup"}]);
        let extracted = extract_event(&val).expect("extracted event");
        assert_eq!(
            extracted.get("title").and_then(Value::as_str),
            Some("Team Standup")
        );
    }

    #[test]
    fn test_extract_event_from_object() {
        let val = json!({"@type": "Event", "title": "Planning"});
        let extracted = extract_event(&val).expect("extracted event");
        assert_eq!(
            extracted.get("title").and_then(Value::as_str),
            Some("Planning")
        );
    }

    #[test]
    fn test_compare_events_divergences() {
        let server = json!({
            "@type": "Event",
            "title": "Meeting",
            "updated": "2026-09-01T09:00:00Z",
            "recurrenceRule": {"frequency": "weekly"}
        });
        let local = json!({
            "@type": "Event",
            "title": "Meeting",
            "recurrenceRules": [{"frequency": "weekly"}]
        });

        let server_obj = server.as_object().unwrap();
        let local_obj = local.as_object().unwrap();
        let diff = compare_events(server_obj, local_obj);

        assert_eq!(diff.server_only.len(), 2);
        assert!(diff.server_only.contains_key("recurrenceRule"));
        assert!(diff.server_only.contains_key("updated"));
        assert_eq!(diff.local_only.len(), 1);
        assert!(diff.local_only.contains_key("recurrenceRules"));
        assert_eq!(diff.differences.len(), 0);
        assert_eq!(diff.matches, vec!["@type", "title"]);
    }

    #[test]
    fn test_compare_events_identical() {
        let event = json!({
            "@type": "Event",
            "title": "Meeting",
            "start": "2026-09-01T09:00:00Z"
        });
        let obj = event.as_object().unwrap();
        let diff = compare_events(obj, obj);
        assert_eq!(diff.matches.len(), 3);
        assert!(diff.differences.is_empty());
        assert!(diff.server_only.is_empty());
        assert!(diff.local_only.is_empty());
    }

    #[test]
    fn test_find_fixtures_dir() {
        let dir = find_fixtures_dir().expect("fixtures dir must exist");
        assert!(dir.is_dir());
        let count = fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "ics"))
            .count();
        assert_eq!(count, 9);
    }
}
