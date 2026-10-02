// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live differential verification for JSContact fidelity against a real JMAP
// server (such as Stalwart): uploads each fixture .vcf file, calls
// ContactCard/parse, and compares the server's JSContact rendering beside
// jmap-vcard's own parse of the same bytes, field by field. Mirrors
// calendar-parse-probe.rs's shape for contacts.
//
// Usage:
//   cargo run -p evolution-jmap-client --example contact-parse-probe -- \
//       <origin> <user> <password>

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use jmap_client::{Client, Credentials};
use jmap_proto::Id;
use jmap_proto::contacts::ContactCardParseRequest;
use jmap_proto::session::CAPABILITY_CONTACTS;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff {
    pub matches: Vec<String>,
    pub differences: BTreeMap<String, (Value, Value)>,
    pub server_only: BTreeMap<String, Value>,
    pub local_only: BTreeMap<String, Value>,
}

pub fn compare_cards(server_obj: &Map<String, Value>, local_obj: &Map<String, Value>) -> FieldDiff {
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
        manifest_dir.join("../jmap-vcard/tests/fixtures"),
        PathBuf::from("rust/crates/jmap-vcard/tests/fixtures"),
        PathBuf::from("crates/jmap-vcard/tests/fixtures"),
        PathBuf::from("jmap-vcard/tests/fixtures"),
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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "vcf"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        fixture_count += 1;
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        println!("\n=== Fixture: {name} ===");

        let vcf_bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL read file {name}: {e}");
                fail += 1;
                continue;
            }
        };
        let vcf_text = match String::from_utf8(vcf_bytes.clone()) {
            Ok(t) => t,
            Err(e) => {
                println!("FAIL decode UTF-8 {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 1. Upload .vcf blob
        let upload_res = client.upload_blob(account_id, "text/vcard", vcf_bytes);
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

        // 2. Call ContactCard/parse
        let parse_req = ContactCardParseRequest::new(account_id.clone(), [blob_id.clone()]);
        let parse_res = client.contact_card_parse(&parse_req);
        let response = match parse_res {
            Ok(response) => {
                println!("PASS ContactCard/parse accepted");
                response
            }
            Err(e) => {
                println!("FAIL ContactCard/parse {name}: {e}");
                fail += 1;
                continue;
            }
        };

        let server_card = response
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.get(&blob_id));

        let Some(server_card) = server_card else {
            println!(
                "FAIL server returned no parsed card for {name} (notParsable: {:?}, notFound: {:?})",
                response.not_parsable, response.not_found
            );
            fail += 1;
            continue;
        };

        let server_obj = match serde_json::to_value(server_card) {
            Ok(Value::Object(map)) => map,
            _ => {
                println!("FAIL serialize server card to object for {name}");
                fail += 1;
                continue;
            }
        };

        // 3. Parse locally with jmap-vcard
        let local_card_res = jmap_vcard::vcard_to_card(&vcf_text);
        let local_obj = match local_card_res {
            Ok(card) => {
                println!("PASS jmap-vcard parsed locally");
                match serde_json::to_value(&card) {
                    Ok(Value::Object(map)) => map,
                    _ => {
                        println!("FAIL serialize jmap-vcard card to object for {name}");
                        fail += 1;
                        continue;
                    }
                }
            }
            Err(e) => {
                println!("FAIL jmap-vcard failed to parse {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 4. Compare field by field
        let diff = compare_cards(&server_obj, &local_obj);
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
                let default_url = std::env::var("STALWART_URL").unwrap_or_else(|_| {
                    "http://[2603:8000:4300:ed77:5054:ff:fe3d:2806]:8080".to_string()
                });
                (default_url, u, p)
            } else {
                eprintln!("usage: contact-parse-probe <origin> <user> <password>");
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
    let has_contacts = session.capabilities.contains_key(CAPABILITY_CONTACTS);
    let has_contacts_parse = session
        .capabilities
        .contains_key("urn:ietf:params:jmap:contacts:parse");
    println!("Capability {CAPABILITY_CONTACTS} advertised: {has_contacts}");
    println!("Capability urn:ietf:params:jmap:contacts:parse advertised: {has_contacts_parse}");

    let account_id = match client.primary_account(CAPABILITY_CONTACTS) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("FAIL no primary contacts account: {e}");
            std::process::exit(1);
        }
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
    use serde_json::json;

    #[test]
    fn test_compare_cards_divergences() {
        let server = json!({
            "@type": "Card",
            "uid": "abc",
            "name": {"full": "Vera Oldenburg"},
        });
        let local = json!({
            "@type": "Card",
            "uid": "abc",
            "nicknames": {"n1": {"name": "Vee"}},
        });

        let server_obj = server.as_object().unwrap();
        let local_obj = local.as_object().unwrap();
        let diff = compare_cards(server_obj, local_obj);

        assert_eq!(diff.server_only.len(), 1);
        assert!(diff.server_only.contains_key("name"));
        assert_eq!(diff.local_only.len(), 1);
        assert!(diff.local_only.contains_key("nicknames"));
        assert_eq!(diff.differences.len(), 0);
        assert_eq!(diff.matches, vec!["@type", "uid"]);
    }

    #[test]
    fn test_compare_cards_identical() {
        let card = json!({
            "@type": "Card",
            "uid": "abc",
            "name": {"full": "Vera Oldenburg"},
        });
        let obj = card.as_object().unwrap();
        let diff = compare_cards(obj, obj);
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
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "vcf"))
            .count();
        assert_eq!(count, 9);
    }
}
