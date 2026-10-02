// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live differential verification for outbound CardDAV fidelity against a real
// CardDAV server (such as Stalwart): parses each fixture .vcf file with
// jmap-vcard into a ContactCard, serializes it back out with jmap-vcard's own
// outbound path, PUTs the result to an address book on the server, GETs it
// back, and compares the server's normalized vCard beside what was sent,
// field by field. Mirrors caldav-put-probe.rs's shape for calendars.
//
// Usage:
//   cargo run -p evolution-jmap-client --example carddav-put-probe -- \
//       <origin> <user> <password>

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff {
    pub matches: Vec<String>,
    pub differences: BTreeMap<String, (String, String)>,
    pub server_only: BTreeMap<String, String>,
    pub local_only: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VCardProperty {
    pub name: String,
    pub params: BTreeMap<String, Vec<String>>,
    pub value: String,
}

pub fn unfold_vcard(raw: &str) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if (line.starts_with(' ') || line.starts_with('\t'))
            && let Some(prev) = unfolded.last_mut()
        {
            prev.push_str(&line[1..]);
            continue;
        }
        unfolded.push(line.to_string());
    }
    unfolded
}

fn split_outside_quotes(s: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;

    for (idx, ch) in s.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == delimiter && !in_quotes {
            parts.push(&s[start..idx]);
            start = idx + ch.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

fn strip_quotes(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn parse_property_line(line: &str) -> Option<VCardProperty> {
    let colon_parts = split_outside_quotes(line, ':');
    if colon_parts.len() < 2 {
        return None;
    }

    let left = colon_parts[0];
    let value = colon_parts[1..].join(":");

    let semi_parts = split_outside_quotes(left, ';');
    let name = semi_parts[0].trim().to_uppercase();
    if name.is_empty() || name == "BEGIN" || name == "END" {
        return None;
    }

    let mut params = BTreeMap::new();
    for part in &semi_parts[1..] {
        let eq_parts: Vec<&str> = part.splitn(2, '=').collect();
        if eq_parts.len() == 2 {
            let p_name = eq_parts[0].trim().to_uppercase();
            let raw_val = eq_parts[1].trim();
            let mut val_items: Vec<String> = split_outside_quotes(raw_val, ',')
                .into_iter()
                .map(strip_quotes)
                .collect();
            val_items.sort();
            params.insert(p_name, val_items);
        }
    }

    let norm_value = if name == "CATEGORIES" {
        let mut cats: Vec<String> = split_outside_quotes(&value, ',')
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        cats.sort();
        cats.join(",")
    } else {
        value
    };

    Some(VCardProperty {
        name,
        params,
        value: norm_value,
    })
}

fn format_property(prop: &VCardProperty) -> String {
    if prop.params.is_empty() {
        prop.value.clone()
    } else {
        let mut p_tokens = Vec::new();
        for (k, vals) in &prop.params {
            p_tokens.push(format!("{k}={}", vals.join(",")));
        }
        format!(";{}:{}", p_tokens.join(";"), prop.value)
    }
}

pub fn parse_vcard_properties(raw: &str) -> Vec<VCardProperty> {
    unfold_vcard(raw)
        .iter()
        .filter_map(|line| parse_property_line(line))
        .collect()
}

/// Flattens a vCard's properties into a path keyed by each property's own
/// `X-JMAP-KEY` parameter when present (the disambiguator `card_to_vcard`
/// already writes for every multi-entry property), falling back to an
/// occurrence index for properties that carry none.
pub fn flatten_properties(props: &[VCardProperty]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for prop in props {
        *counts.entry(prop.name.clone()).or_insert(0) += 1;
    }

    let mut indices: BTreeMap<String, usize> = BTreeMap::new();
    for prop in props {
        let count = counts.get(&prop.name).copied().unwrap_or(0);
        let key = if let Some(jmap_key) = prop.params.get("X-JMAP-KEY").and_then(|v| v.first()) {
            format!("{}[{jmap_key}]", prop.name)
        } else if count > 1 {
            let idx = indices.entry(prop.name.clone()).or_insert(0);
            let k = format!("{}[{idx}]", prop.name);
            *idx += 1;
            k
        } else {
            prop.name.clone()
        };

        // X-JMAP-KEY itself is the disambiguator, not a field to diff.
        let mut params = prop.params.clone();
        params.remove("X-JMAP-KEY");
        let formatted = format_property(&VCardProperty {
            name: prop.name.clone(),
            params,
            value: prop.value.clone(),
        });
        out.insert(key, formatted);
    }

    out
}

pub fn parse_and_flatten_vcard(raw: &str) -> BTreeMap<String, String> {
    flatten_properties(&parse_vcard_properties(raw))
}

pub fn compare_vcard(server_raw: &str, local_raw: &str) -> FieldDiff {
    let server_map = parse_and_flatten_vcard(server_raw);
    let local_map = parse_and_flatten_vcard(local_raw);

    let mut all_keys = BTreeSet::new();
    for k in server_map.keys() {
        all_keys.insert(k.clone());
    }
    for k in local_map.keys() {
        all_keys.insert(k.clone());
    }

    let mut matches = Vec::new();
    let mut differences = BTreeMap::new();
    let mut server_only = BTreeMap::new();
    let mut local_only = BTreeMap::new();

    for key in all_keys {
        match (server_map.get(&key), local_map.get(&key)) {
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

pub fn print_diff(fixture_name: &str, diff: &FieldDiff) {
    println!(
        "--- Outbound comparison for {fixture_name} ({} match, {} diff, {} server-only, {} local-only) ---",
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
        println!("         server: {s}");
        println!("         local:  {l}");
    }
    for (k, s) in &diff.server_only {
        println!("    [+] server-only: {k}: {s}");
    }
    for (k, l) in &diff.local_only {
        println!("    [-] local-only:  {k}: {l}");
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

fn make_agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .redirect_auth_headers(ureq::config::RedirectAuthHeaders::SameHost)
        .build();
    config.into()
}

fn extract_xml_tag(xml: &str, tag_name: &str) -> Option<String> {
    let open = format!("<{tag_name}>");
    let open_ns = format!(":{tag_name}>");
    let close = format!("</{tag_name}>");
    let close_ns = format!(":{tag_name}>");

    let start = if let Some(pos) = xml.find(&open) {
        pos + open.len()
    } else {
        let pos = xml.find(&open_ns)?;
        pos + open_ns.len()
    };

    let rest = &xml[start..];
    let end = if let Some(pos) = rest.find(&close) {
        pos
    } else {
        let pos = rest.find(&close_ns)?;
        let prefix = &rest[..pos];
        prefix.rfind('<').unwrap_or(pos)
    };

    Some(rest[..end].trim().to_string())
}

pub fn resolve_well_known_carddav(origin: &str, auth_header: &str) -> String {
    let well_known = format!("{origin}/.well-known/carddav");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    let mut builder = ureq::http::Request::builder()
        .method("GET")
        .uri(&well_known);
    if !auth_header.is_empty() {
        builder = builder.header("Authorization", auth_header);
    }
    let req = builder.body("").ok();
    if let Some(req) = req
        && let Ok(resp) = agent.run(req)
    {
        let status = resp.status().as_u16();
        if resp.status().is_redirection()
            && let Some(loc) = resp.headers().get("location").and_then(|h| h.to_str().ok())
        {
            println!("PASS resolved /.well-known/carddav -> {loc} (HTTP {status})");
            if loc.starts_with("http") {
                return loc.to_string();
            } else if loc.starts_with('/') {
                return format!("{origin}{loc}");
            } else {
                return format!("{origin}/{loc}");
            }
        }
    }
    format!("{origin}/dav/card")
}

fn find_addressbook_home(
    agent: &ureq::Agent,
    origin: &str,
    card_endpoint: &str,
    user: &str,
    auth_header: &str,
) -> Option<String> {
    let req = ureq::http::Request::builder()
        .method("PROPFIND")
        .uri(card_endpoint)
        .header("Authorization", auth_header)
        .header("Depth", "0")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:carddav\">\
             <D:prop><C:addressbook-home-set/><D:current-user-principal/></D:prop>\
             </D:propfind>",
        )
        .ok()?;

    let mut resp = agent.run(req).ok()?;
    let body = resp.body_mut().read_to_string().ok()?;
    let home = extract_xml_tag(&body, "addressbook-home-set")?;
    let href = extract_xml_tag(&home, "href")?;
    let mut resolved_home = if href.starts_with("http") {
        href
    } else {
        format!(
            "{origin}{}",
            if href.starts_with('/') { &href } else { "/" }
        )
    };

    let head_req = ureq::http::Request::builder()
        .method("PROPFIND")
        .uri(&resolved_home)
        .header("Authorization", auth_header)
        .header("Depth", "0")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body("")
        .ok();
    if let Some(hreq) = head_req
        && let Ok(hresp) = agent.run(hreq)
        && hresp.status().as_u16() == 404
        && !user.contains('@')
    {
        let qualified = resolved_home.replace(
            &format!("/{user}/"),
            &format!("/{user}%40example.internal/"),
        );
        resolved_home = qualified;
    }

    Some(resolved_home)
}

fn find_collection_in_home(
    agent: &ureq::Agent,
    origin: &str,
    home_url: &str,
    auth_header: &str,
) -> Option<String> {
    let req = ureq::http::Request::builder()
        .method("PROPFIND")
        .uri(home_url)
        .header("Authorization", auth_header)
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body("")
        .ok()?;

    let mut resp = agent.run(req).ok()?;
    let body = resp.body_mut().read_to_string().ok()?;

    for response_chunk in body.split("<D:response>") {
        if !response_chunk.contains("addressbook") {
            continue;
        }
        let Some(col_href) = extract_xml_tag(response_chunk, "href") else {
            continue;
        };
        if col_href.trim_end_matches('/') == home_url.trim_end_matches('/') {
            continue;
        }
        let mut col_url = if col_href.starts_with("http") {
            col_href
        } else {
            format!("{origin}{col_href}")
        };
        if !col_url.ends_with('/') {
            col_url.push('/');
        }
        return Some(col_url);
    }
    None
}

pub fn discover_collection_url(
    agent: &ureq::Agent,
    origin: &str,
    user: &str,
    auth_header: &str,
) -> String {
    let card_endpoint = resolve_well_known_carddav(origin, auth_header);
    if let Some(col_url) = find_addressbook_home(agent, origin, &card_endpoint, user, auth_header)
        .and_then(|home_url| find_collection_in_home(agent, origin, &home_url, auth_header))
    {
        return col_url;
    }

    // Default fallback path for Stalwart account
    if user.contains('@') {
        let enc_user = user.replace('@', "%40");
        format!("{origin}/dav/card/{enc_user}/default/")
    } else {
        format!("{origin}/dav/card/{user}%40example.internal/default/")
    }
}

pub fn probe_fixtures(
    agent: &ureq::Agent,
    collection_url: &str,
    auth_header: &str,
    fixtures_dir: &Path,
) -> (usize, usize, usize) {
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
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        println!("\n=== Fixture: {name} ===");

        let vcf_bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL read file {name}: {e}");
                fail += 1;
                continue;
            }
        };
        let vcf_text = match String::from_utf8(vcf_bytes) {
            Ok(t) => t,
            Err(e) => {
                println!("FAIL decode UTF-8 {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 1. Inbound parse with jmap-vcard
        let card = match jmap_vcard::vcard_to_card(&vcf_text) {
            Ok(c) => {
                println!("PASS jmap-vcard parsed inbound fixture");
                c
            }
            Err(e) => {
                println!("FAIL jmap-vcard failed inbound parse for {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 2. Outbound serialize with jmap-vcard
        let local_sent_vcf = jmap_vcard::card_to_vcard(&card);
        println!(
            "PASS jmap-vcard serialized outbound .vcf ({} bytes)",
            local_sent_vcf.len()
        );

        // 3. CardDAV PUT to server
        let resource_name = format!("probe-{stem}.vcf");
        let item_url = format!("{collection_url}{resource_name}");
        let put_res = agent
            .put(&item_url)
            .header("Authorization", auth_header)
            .header("Content-Type", "text/vcard; charset=utf-8")
            .send(local_sent_vcf.as_bytes());

        let put_ok = match put_res {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 201 || status == 204 || status == 200 {
                    println!("PASS CardDAV PUT accepted (HTTP {status})");
                    true
                } else {
                    println!("FAIL CardDAV PUT {name} returned HTTP {status}");
                    false
                }
            }
            Err(e) => {
                println!("FAIL CardDAV PUT {name}: {e}");
                false
            }
        };

        if !put_ok {
            fail += 1;
            continue;
        }

        // 4. CardDAV GET back from server
        let get_res = agent
            .get(&item_url)
            .header("Authorization", auth_header)
            .header("Accept", "text/vcard")
            .call();

        let server_normalized_vcf = match get_res {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    match resp.body_mut().read_to_string() {
                        Ok(body) => {
                            let raw_equal = body == local_sent_vcf;
                            println!(
                                "PASS CardDAV GET retrieved normalized .vcf ({} bytes, raw byte match: {})",
                                body.len(),
                                raw_equal
                            );
                            body
                        }
                        Err(e) => {
                            println!("FAIL read GET response body for {name}: {e}");
                            fail += 1;
                            continue;
                        }
                    }
                } else {
                    println!("FAIL CardDAV GET {name} returned HTTP {status}");
                    fail += 1;
                    continue;
                }
            }
            Err(e) => {
                println!("FAIL CardDAV GET {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 5. Cleanup: DELETE resource from collection
        let _ = agent
            .delete(&item_url)
            .header("Authorization", auth_header)
            .call();

        // 6. Diff normalized server .vcf against what was sent
        let diff = compare_vcard(&server_normalized_vcf, &local_sent_vcf);
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

fn run_mock_server() {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("start mock tiny_http server");
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap();
    let origin = format!("http://127.0.0.1:{port}");
    let collection_url = format!("{origin}/dav/card/mock/default/");
    let mock_col_url = collection_url.clone();

    let handle = std::thread::spawn(move || {
        let mut store: BTreeMap<String, String> = BTreeMap::new();
        while let Ok(mut req) = server.recv() {
            let method = req.method().as_str().to_string();
            let url = req.url().to_string();

            if method == "GET" && url == "/.well-known/carddav" {
                let resp = tiny_http::Response::from_string("")
                    .with_status_code(307)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/dav/card"[..]).unwrap(),
                    );
                let _ = req.respond(resp);
                continue;
            }

            if method == "PROPFIND" {
                let xml = if url == "/dav/card" {
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                     <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:carddav\">\
                     <D:response>\
                     <D:href>/dav/card/mock/</D:href>\
                     <D:propstat>\
                     <D:prop><C:addressbook-home-set><D:href>/dav/card/mock/</D:href></C:addressbook-home-set></D:prop>\
                     <D:status>HTTP/1.1 200 OK</D:status>\
                     </D:propstat>\
                     </D:response>\
                     </D:multistatus>"
                        .to_string()
                } else {
                    format!(
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                         <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:carddav\">\
                         <D:response>\
                         <D:href>{mock_col_url}</D:href>\
                         <D:propstat>\
                         <D:prop>\
                         <D:resourcetype><D:collection/><C:addressbook/></D:resourcetype>\
                         </D:prop>\
                         <D:status>HTTP/1.1 200 OK</D:status>\
                         </D:propstat>\
                         </D:response>\
                         </D:multistatus>"
                    )
                };
                let resp = tiny_http::Response::from_string(xml)
                    .with_status_code(207)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/xml"[..],
                        )
                        .unwrap(),
                    );
                let _ = req.respond(resp);
            } else if method == "PUT" {
                let mut content = String::new();
                let mut reader = req.as_reader();
                let _ = std::io::Read::read_to_string(&mut reader, &mut content);
                store.insert(url, content);
                let resp = tiny_http::Response::from_string("").with_status_code(201);
                let _ = req.respond(resp);
            } else if method == "GET" {
                if let Some(content) = store.get(&url) {
                    let resp = tiny_http::Response::from_string(content.clone())
                        .with_status_code(200)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/vcard"[..])
                                .unwrap(),
                        );
                    let _ = req.respond(resp);
                } else {
                    let resp = tiny_http::Response::from_string("Not Found").with_status_code(404);
                    let _ = req.respond(resp);
                }
            } else if method == "DELETE" {
                store.remove(&url);
                let resp = tiny_http::Response::from_string("").with_status_code(204);
                let _ = req.respond(resp);
            } else {
                let resp = tiny_http::Response::from_string("OK").with_status_code(200);
                let _ = req.respond(resp);
            }
        }
    });

    let agent = make_agent(Duration::from_secs(5));
    let fixtures_dir = find_fixtures_dir().expect("find fixtures directory");
    println!("Running CardDAV outbound differential probe against in-process mock server...");
    let collection_url = discover_collection_url(&agent, &origin, "mock", "Basic bW9jazptb2Nr");
    println!("Discovered mock CardDAV collection: {collection_url}");
    let (total, fail, divergences) =
        probe_fixtures(&agent, &collection_url, "Basic bW9jazptb2Nr", &fixtures_dir);
    println!(
        "\nSummary: {total} fixtures probed, {divergences} with divergences, {fail} check failures"
    );

    drop(agent);
    drop(handle);

    if fail > 0 {
        std::process::exit(1);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first_arg = args.next();

    if first_arg.as_deref() == Some("mock") {
        run_mock_server();
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
                eprintln!("usage: carddav-put-probe <origin> <user> <password>");
                std::process::exit(2);
            }
        }
    };

    println!("Connecting to CardDAV server at {origin} as {user}...");
    let agent = make_agent(Duration::from_secs(15));
    let creds_str = format!("{user}:{password}");
    let auth_header = format!("Basic {}", BASE64.encode(creds_str.as_bytes()));

    let collection_url = discover_collection_url(&agent, &origin, &user, &auth_header);
    println!("Target CardDAV collection: {collection_url}");

    let fixtures_dir = find_fixtures_dir().expect("find fixtures directory");
    println!("Fixtures directory: {}", fixtures_dir.display());

    let (total, fail, divergences) =
        probe_fixtures(&agent, &collection_url, &auth_header, &fixtures_dir);
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
    fn test_unfold_vcard_continuation_lines() {
        let raw =
            "BEGIN:VCARD\r\nFN:First Line \r\n Continued Line\r\n\tTab Continued\r\nEND:VCARD";
        let unfolded = unfold_vcard(raw);
        assert_eq!(unfolded.len(), 3);
        assert_eq!(unfolded[1], "FN:First Line Continued LineTab Continued");
    }

    #[test]
    fn test_parse_property_line_categories_normalization() {
        let line = "CATEGORIES:Zebra,Apple,Mango";
        let prop = parse_property_line(line).expect("parse categories");
        assert_eq!(prop.name, "CATEGORIES");
        assert_eq!(prop.value, "Apple,Mango,Zebra");
    }

    #[test]
    fn test_parse_property_line_params() {
        let line = "EMAIL;X-JMAP-KEY=e1;TYPE=HOME,PREF:arthur.dent@earth.example";
        let prop = parse_property_line(line).expect("parse email");
        assert_eq!(prop.name, "EMAIL");
        assert_eq!(prop.value, "arthur.dent@earth.example");
        assert_eq!(
            prop.params.get("X-JMAP-KEY").unwrap(),
            &vec!["e1".to_string()]
        );
        assert_eq!(
            prop.params.get("TYPE").unwrap(),
            &vec!["HOME".to_string(), "PREF".to_string()]
        );
    }

    #[test]
    fn test_flatten_properties_keys_by_jmap_key_not_index() {
        let props = vec![
            parse_property_line("EMAIL;X-JMAP-KEY=e2;TYPE=WORK:b@example.com").unwrap(),
            parse_property_line("EMAIL;X-JMAP-KEY=e1;TYPE=HOME:a@example.com").unwrap(),
        ];
        let flat = flatten_properties(&props);
        assert_eq!(flat.get("EMAIL[e1]").unwrap(), ";TYPE=HOME:a@example.com");
        assert_eq!(flat.get("EMAIL[e2]").unwrap(), ";TYPE=WORK:b@example.com");
    }

    #[test]
    fn test_compare_vcard_identical() {
        let vcf = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:123\r\nFN:Test\r\nEND:VCARD";
        let diff = compare_vcard(vcf, vcf);
        assert_eq!(diff.matches.len(), 3);
        assert!(diff.differences.is_empty());
        assert!(diff.server_only.is_empty());
        assert!(diff.local_only.is_empty());
    }

    #[test]
    fn test_compare_vcard_differences_and_exclusive_fields() {
        let server =
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:123\r\nFN:Arthur Dent\r\nNOTE:hi\r\nEND:VCARD";
        let local = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:123\r\nFN:Arthur Dent\r\nBDAY:1978-03-08\r\nEND:VCARD";
        let diff = compare_vcard(server, local);

        assert!(diff.matches.contains(&"VERSION".to_string()));
        assert!(diff.matches.contains(&"UID".to_string()));
        assert!(diff.matches.contains(&"FN".to_string()));
        assert!(diff.server_only.contains_key("NOTE"));
        assert!(diff.local_only.contains_key("BDAY"));
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
        assert_eq!(count, 10);
    }

    #[test]
    fn test_extract_xml_tag() {
        let xml = "<D:response><D:href>/dav/card/admin/</D:href></D:response>";
        assert_eq!(
            extract_xml_tag(xml, "href"),
            Some("/dav/card/admin/".to_string())
        );
        let xml_unprefixed = "<item><href>http://example.com/card/</href></item>";
        assert_eq!(
            extract_xml_tag(xml_unprefixed, "href"),
            Some("http://example.com/card/".to_string())
        );
    }

    #[test]
    fn test_discover_collection_url_fallback() {
        let agent = make_agent(Duration::from_millis(100));
        let col = discover_collection_url(&agent, "http://127.0.0.1:1", "alice", "Basic Og==");
        assert_eq!(
            col,
            "http://127.0.0.1:1/dav/card/alice%40example.internal/default/"
        );

        let col_email = discover_collection_url(
            &agent,
            "http://127.0.0.1:1",
            "bob@example.com",
            "Basic Og==",
        );
        assert_eq!(
            col_email,
            "http://127.0.0.1:1/dav/card/bob%40example.com/default/"
        );
    }

    #[test]
    fn test_resolve_well_known_carddav_fallback() {
        let endpoint = resolve_well_known_carddav("http://127.0.0.1:1", "Basic Og==");
        assert_eq!(endpoint, "http://127.0.0.1:1/dav/card");
    }
}
