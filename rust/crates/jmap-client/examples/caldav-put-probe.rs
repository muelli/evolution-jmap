// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live differential verification for outbound CalDAV fidelity against a real
// CalDAV server (such as Stalwart): parses each fixture .ics file with
// jmap-ical into a JSCalendar Event, serializes it back out with jmap-ical's
// own outbound path, PUTs the result to a CalDAV collection on the server,
// GETs it back, and compares the server's normalized .ics beside what was
// sent, field by field.
//
// Usage:
//   cargo run -p evolution-jmap-client --example caldav-put-probe -- \
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
pub struct IcsComponent {
    pub kind: String,
    pub properties: Vec<IcsProperty>,
    pub subcomponents: Vec<IcsComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcsProperty {
    pub name: String,
    pub params: BTreeMap<String, Vec<String>>,
    pub value: String,
}

pub fn unfold_ics(raw: &str) -> Vec<String> {
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

pub fn parse_property_line(line: &str) -> Option<IcsProperty> {
    let colon_parts = split_outside_quotes(line, ':');
    if colon_parts.len() < 2 {
        return None;
    }

    let left = colon_parts[0];
    let value = colon_parts[1..].join(":");

    let semi_parts = split_outside_quotes(left, ';');
    let name = semi_parts[0].trim().to_uppercase();
    if name.is_empty() {
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

    let norm_value = if name == "RRULE" {
        let mut clauses: Vec<String> = split_outside_quotes(&value, ';')
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        clauses.sort();
        clauses.join(";")
    } else if name == "CATEGORIES" {
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

    Some(IcsProperty {
        name,
        params,
        value: norm_value,
    })
}

pub fn parse_ics_component(lines: &[String], mut idx: usize) -> (Option<IcsComponent>, usize) {
    while idx < lines.len() && !lines[idx].starts_with("BEGIN:") {
        idx += 1;
    }
    if idx >= lines.len() {
        return (None, idx);
    }

    let begin_line = &lines[idx];
    let kind = begin_line["BEGIN:".len()..].trim().to_uppercase();
    idx += 1;

    let mut properties = Vec::new();
    let mut subcomponents = Vec::new();

    while idx < lines.len() {
        let line = &lines[idx];
        if line.starts_with("BEGIN:") {
            let (sub, next_idx) = parse_ics_component(lines, idx);
            if let Some(s) = sub {
                subcomponents.push(s);
            }
            idx = next_idx;
        } else if let Some(stripped) = line.strip_prefix("END:") {
            let end_kind = stripped.trim().to_uppercase();
            idx += 1;
            if end_kind == kind {
                break;
            }
        } else {
            if let Some(prop) = parse_property_line(line) {
                properties.push(prop);
            }
            idx += 1;
        }
    }

    (
        Some(IcsComponent {
            kind,
            properties,
            subcomponents,
        }),
        idx,
    )
}

fn format_property(prop: &IcsProperty) -> String {
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

pub fn flatten_component(
    comp: &IcsComponent,
    parent_path: &str,
    out: &mut BTreeMap<String, String>,
) {
    let path = if parent_path.is_empty() {
        comp.kind.clone()
    } else {
        format!("{parent_path}/{}", comp.kind)
    };

    let mut prop_counts: BTreeMap<String, usize> = BTreeMap::new();
    for prop in &comp.properties {
        *prop_counts.entry(prop.name.clone()).or_insert(0) += 1;
    }

    let mut all_categories: Vec<String> = Vec::new();
    for prop in &comp.properties {
        if prop.name == "CATEGORIES" {
            for cat in prop.value.split(',') {
                let trimmed = cat.trim();
                if !trimmed.is_empty() {
                    all_categories.push(trimmed.to_string());
                }
            }
        }
    }
    all_categories.sort();
    all_categories.dedup();

    let mut emitted_categories = false;
    let mut prop_indices: BTreeMap<String, usize> = BTreeMap::new();
    for prop in &comp.properties {
        if prop.name == "CATEGORIES" {
            if !emitted_categories {
                out.insert(format!("{path}/CATEGORIES"), all_categories.join(","));
                emitted_categories = true;
            }
            continue;
        }
        let count = prop_counts.get(&prop.name).copied().unwrap_or(0);
        let prop_key = match prop.name.as_str() {
            "ATTENDEE" => {
                let addr = prop.value.strip_prefix("mailto:").unwrap_or(&prop.value);
                format!("{path}/ATTENDEE[{addr}]")
            }
            "EXDATE" => format!("{path}/EXDATE[{}]", prop.value),
            "RDATE" => format!("{path}/RDATE[{}]", prop.value),
            "ATTACH" => {
                let href = prop.value.trim();
                format!("{path}/ATTACH[{href}]")
            }
            "CONFERENCE" => {
                let uri = prop.value.trim();
                format!("{path}/CONFERENCE[{uri}]")
            }
            _ => {
                if count > 1 {
                    let idx = prop_indices.entry(prop.name.clone()).or_insert(0);
                    let k = format!("{path}/{}[{idx}]", prop.name);
                    *idx += 1;
                    k
                } else {
                    format!("{path}/{}", prop.name)
                }
            }
        };
        out.insert(prop_key, format_property(prop));
    }

    let mut sub_counts: BTreeMap<String, usize> = BTreeMap::new();
    for sub in &comp.subcomponents {
        let sub_id = match sub.kind.as_str() {
            "VEVENT" => {
                let rec_id = sub
                    .properties
                    .iter()
                    .find(|p| p.name == "RECURRENCE-ID")
                    .map(|p| p.value.clone());
                if let Some(rid) = rec_id {
                    format!("VEVENT[recurrence-id={rid}]")
                } else {
                    "VEVENT[master]".to_string()
                }
            }
            "VTIMEZONE" => {
                let tzid = sub
                    .properties
                    .iter()
                    .find(|p| p.name == "TZID")
                    .map(|p| p.value.clone())
                    .unwrap_or_else(|| "unnamed".to_string());
                format!("VTIMEZONE[tzid={tzid}]")
            }
            _ => {
                let idx = sub_counts.entry(sub.kind.clone()).or_insert(0);
                let id = format!("{}[{idx}]", sub.kind);
                *idx += 1;
                id
            }
        };

        let mut sub_comp_copy = sub.clone();
        sub_comp_copy.kind = sub_id;
        flatten_component(&sub_comp_copy, &path, out);
    }
}

pub fn parse_and_flatten_ics(raw: &str) -> BTreeMap<String, String> {
    let lines = unfold_ics(raw);
    let mut out = BTreeMap::new();
    let mut idx = 0;
    while idx < lines.len() {
        let (comp, next_idx) = parse_ics_component(&lines, idx);
        if let Some(c) = comp {
            flatten_component(&c, "", &mut out);
        }
        idx = next_idx;
    }
    out
}

pub fn compare_ics(server_raw: &str, local_raw: &str) -> FieldDiff {
    let server_map = parse_and_flatten_ics(server_raw);
    let local_map = parse_and_flatten_ics(local_raw);

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

pub fn resolve_well_known_caldav(origin: &str, auth_header: &str) -> String {
    let well_known = format!("{origin}/.well-known/caldav");
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
            println!("PASS resolved /.well-known/caldav -> {loc} (HTTP {status})");
            if loc.starts_with("http") {
                return loc.to_string();
            } else if loc.starts_with('/') {
                return format!("{origin}{loc}");
            } else {
                return format!("{origin}/{loc}");
            }
        }
    }
    format!("{origin}/dav/cal")
}

trait ReadWrite: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> ReadWrite for T {}

fn propfind_xml(uri: &str, depth: &str, body: &str, auth_header: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::Arc;

    let (tls, rest) = if let Some(stripped) = uri.strip_prefix("https://") {
        (true, stripped)
    } else {
        (false, uri.strip_prefix("http://")?)
    };

    let slash = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..slash];
    let path = if slash < rest.len() {
        &rest[slash..]
    } else {
        "/"
    };

    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (literal, after) = bracketed.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse::<u16>().ok()?,
                None => {
                    if tls {
                        443
                    } else {
                        80
                    }
                }
            };
            (literal, port)
        }
        None => match authority.split_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok()?),
            None => {
                if tls {
                    (authority, 443)
                } else {
                    (authority, 80)
                }
            }
        },
    };

    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .ok()?;

    let mut stream: Box<dyn ReadWrite> = if tls {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string()).ok()?;
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let client = rustls::ClientConnection::new(client_config, server_name).ok()?;
        Box::new(rustls::StreamOwned::new(client, stream))
    } else {
        Box::new(stream)
    };

    let mut req_str = format!(
        "PROPFIND {path} HTTP/1.1\r\n\
         Host: {authority}\r\n\
         Depth: {depth}\r\n\
         Content-Type: application/xml; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    if !auth_header.is_empty() {
        req_str.push_str(&format!("Authorization: {auth_header}\r\n"));
    }
    req_str.push_str("\r\n");
    req_str.push_str(body);

    stream.write_all(req_str.as_bytes()).ok()?;
    stream.flush().ok()?;

    let mut resp_bytes = Vec::new();
    stream.read_to_end(&mut resp_bytes).ok()?;
    String::from_utf8(resp_bytes).ok()
}

fn find_calendar_home(
    _agent: &ureq::Agent,
    origin: &str,
    cal_endpoint: &str,
    user: &str,
    auth_header: &str,
) -> Option<String> {
    let propfind_body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <D:propfind xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
         <D:prop><C:calendar-home-set/><D:current-user-principal/></D:prop>\
         </D:propfind>";

    let body = propfind_xml(cal_endpoint, "0", propfind_body, auth_header)?;
    let home = match extract_xml_tag(&body, "calendar-home-set") {
        Some(h) => h,
        None => {
            let principal = extract_xml_tag(&body, "current-user-principal")?;
            let princ_href = extract_xml_tag(&principal, "href")?;
            let princ_url = if princ_href.starts_with("http") {
                princ_href
            } else {
                format!(
                    "{origin}{}",
                    if princ_href.starts_with('/') {
                        &princ_href
                    } else {
                        "/"
                    }
                )
            };
            let princ_body = propfind_xml(&princ_url, "0", propfind_body, auth_header)?;
            extract_xml_tag(&princ_body, "calendar-home-set")?
        }
    };
    let href = extract_xml_tag(&home, "href")?;
    let mut resolved_home = if href.starts_with("http") {
        href
    } else {
        format!(
            "{origin}{}",
            if href.starts_with('/') { &href } else { "/" }
        )
    };

    if !user.contains('@') {
        let test_resp = propfind_xml(&resolved_home, "0", "", auth_header);
        let is_http_404 = test_resp.as_ref().is_some_and(|r| {
            let first_line = r.lines().next().unwrap_or("");
            first_line.contains(" 404 ")
        });
        if is_http_404 {
            let qualified = resolved_home.replace(
                &format!("/{user}/"),
                &format!("/{user}%40example.internal/"),
            );
            resolved_home = qualified;
        }
    }

    Some(resolved_home)
}

fn find_collection_in_home(
    _agent: &ureq::Agent,
    origin: &str,
    home_url: &str,
    auth_header: &str,
) -> Option<String> {
    let body = propfind_xml(home_url, "1", "", auth_header)?;

    let split_tag = if body.contains("<D:response") {
        "<D:response"
    } else if body.contains("<d:response") {
        "<d:response"
    } else {
        "<response"
    };

    let home_path = home_url
        .strip_prefix("http://")
        .or_else(|| home_url.strip_prefix("https://"))
        .and_then(|rest| rest.find('/').map(|idx| &rest[idx..]))
        .unwrap_or(home_url);

    for response_chunk in body.split(split_tag) {
        if !response_chunk.contains(":calendar")
            && !response_chunk.contains("<calendar")
            && !response_chunk.contains(":CALENDAR")
        {
            continue;
        }
        let Some(col_href) = extract_xml_tag(response_chunk, "href") else {
            continue;
        };
        if col_href.trim_end_matches('/') == home_url.trim_end_matches('/')
            || col_href.trim_end_matches('/') == home_path.trim_end_matches('/')
        {
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
    let cal_endpoint = resolve_well_known_caldav(origin, auth_header);
    if let Some(col_url) = find_calendar_home(agent, origin, &cal_endpoint, user, auth_header)
        .and_then(|home_url| find_collection_in_home(agent, origin, &home_url, auth_header))
    {
        return col_url;
    }

    // Default fallback path for Stalwart account
    if user.contains('@') {
        let enc_user = user.replace('@', "%40");
        format!("{origin}/dav/cal/{enc_user}/default/")
    } else {
        format!("{origin}/dav/cal/{user}%40example.internal/default/")
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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "ics"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        fixture_count += 1;
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        println!("\n=== Fixture: {name} ===");

        let ics_bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                println!("FAIL read file {name}: {e}");
                fail += 1;
                continue;
            }
        };
        let ics_text = match String::from_utf8(ics_bytes) {
            Ok(t) => t,
            Err(e) => {
                println!("FAIL decode UTF-8 {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 1. Inbound parse with jmap-ical
        let event = match jmap_ical::ical_to_event(&ics_text) {
            Ok(ev) => {
                println!("PASS jmap-ical parsed inbound fixture");
                ev
            }
            Err(e) => {
                println!("FAIL jmap-ical failed inbound parse for {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 2. Outbound serialize with jmap-ical
        let local_sent_ics = jmap_ical::event_to_ical(&event);
        println!(
            "PASS jmap-ical serialized outbound .ics ({} bytes)",
            local_sent_ics.len()
        );

        // 3. CalDAV PUT to server
        let resource_name = format!("probe-{stem}.ics");
        let item_url = format!("{collection_url}{resource_name}");
        let put_res = agent
            .put(&item_url)
            .header("Authorization", auth_header)
            .header("Content-Type", "text/calendar; charset=utf-8")
            .send(local_sent_ics.as_bytes());

        let put_ok = match put_res {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 201 || status == 204 || status == 200 {
                    println!("PASS CalDAV PUT accepted (HTTP {status})");
                    true
                } else {
                    println!("FAIL CalDAV PUT {name} returned HTTP {status}");
                    false
                }
            }
            Err(e) => {
                println!("FAIL CalDAV PUT {name}: {e}");
                false
            }
        };

        if !put_ok {
            fail += 1;
            continue;
        }

        // 4. CalDAV GET back from server
        let get_res = agent
            .get(&item_url)
            .header("Authorization", auth_header)
            .header("Accept", "text/calendar")
            .call();

        let server_normalized_ics = match get_res {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                if status == 200 {
                    match resp.body_mut().read_to_string() {
                        Ok(body) => {
                            let raw_equal = body == local_sent_ics;
                            println!(
                                "PASS CalDAV GET retrieved normalized .ics ({} bytes, raw byte match: {})",
                                body.len(),
                                raw_equal
                            );
                            if !raw_equal {
                                let body_lines: Vec<&str> = body.lines().collect();
                                let local_lines: Vec<&str> = local_sent_ics.lines().collect();
                                for (idx, (bl, ll)) in
                                    body_lines.iter().zip(local_lines.iter()).enumerate()
                                {
                                    if bl != ll {
                                        println!("     [line diff {idx}]:");
                                        println!("       server: {bl}");
                                        println!("       local:  {ll}");
                                    }
                                }
                                if body_lines.len() != local_lines.len() {
                                    println!(
                                        "     [line count diff]: server={} vs local={}",
                                        body_lines.len(),
                                        local_lines.len()
                                    );
                                }
                            }
                            body
                        }
                        Err(e) => {
                            println!("FAIL read GET response body for {name}: {e}");
                            fail += 1;
                            continue;
                        }
                    }
                } else {
                    println!("FAIL CalDAV GET {name} returned HTTP {status}");
                    fail += 1;
                    continue;
                }
            }
            Err(e) => {
                println!("FAIL CalDAV GET {name}: {e}");
                fail += 1;
                continue;
            }
        };

        // 5. Cleanup: DELETE resource from collection
        let _ = agent
            .delete(&item_url)
            .header("Authorization", auth_header)
            .call();

        // 6. Diff normalized server .ics against what was sent
        let diff = compare_ics(&server_normalized_ics, &local_sent_ics);
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
    let collection_url = format!("{origin}/dav/cal/mock/default/");
    let mock_col_url = collection_url.clone();

    let handle = std::thread::spawn(move || {
        let mut store: BTreeMap<String, String> = BTreeMap::new();
        while let Ok(mut req) = server.recv() {
            let method = req.method().as_str().to_string();
            let url = req.url().to_string();

            if method == "GET" && url == "/.well-known/caldav" {
                let resp = tiny_http::Response::from_string("")
                    .with_status_code(307)
                    .with_header(
                        tiny_http::Header::from_bytes(&b"Location"[..], &b"/dav/cal"[..]).unwrap(),
                    );
                let _ = req.respond(resp);
                continue;
            }

            if method == "PROPFIND" {
                let xml = if url == "/dav/cal" {
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                     <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                     <D:response>\
                     <D:href>/dav/cal/mock/</D:href>\
                     <D:propstat>\
                     <D:prop><C:calendar-home-set><D:href>/dav/cal/mock/</D:href></C:calendar-home-set></D:prop>\
                     <D:status>HTTP/1.1 200 OK</D:status>\
                     </D:propstat>\
                     </D:response>\
                     </D:multistatus>"
                        .to_string()
                } else {
                    format!(
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                         <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                         <D:response>\
                         <D:href>{mock_col_url}</D:href>\
                         <D:propstat>\
                         <D:prop>\
                         <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
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
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"text/calendar"[..],
                            )
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
    println!("Running CalDAV outbound differential probe against in-process mock server...");
    let collection_url = discover_collection_url(&agent, &origin, "mock", "Basic bW9jazptb2Nr");
    println!("Discovered mock CalDAV collection: {collection_url}");
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
                eprintln!("usage: caldav-put-probe <origin> <user> <password>");
                std::process::exit(2);
            }
        }
    };

    println!("Connecting to CalDAV server at {origin} as {user}...");
    let agent = make_agent(Duration::from_secs(15));
    let creds_str = format!("{user}:{password}");
    let auth_header = format!("Basic {}", BASE64.encode(creds_str.as_bytes()));

    let collection_url = discover_collection_url(&agent, &origin, &user, &auth_header);
    println!("Target CalDAV collection: {collection_url}");

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
    fn test_unfold_ics_continuation_lines() {
        let raw = "BEGIN:VCALENDAR\r\nSUMMARY:First Line \r\n Continued Line\r\n\tTab Continued\r\nEND:VCALENDAR";
        let unfolded = unfold_ics(raw);
        assert_eq!(unfolded.len(), 3);
        assert_eq!(
            unfolded[1],
            "SUMMARY:First Line Continued LineTab Continued"
        );
    }

    #[test]
    fn test_parse_property_line_rrule_normalization() {
        let line = "RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=5;BYDAY=FR";
        let prop = parse_property_line(line).expect("parse rrule");
        assert_eq!(prop.name, "RRULE");
        assert_eq!(prop.value, "BYDAY=FR;COUNT=5;FREQ=WEEKLY;INTERVAL=2");
    }

    #[test]
    fn test_parse_property_line_attendee_params() {
        let line = "ATTENDEE;CN=\"Alicia Vance\";ROLE=REQ-PARTICIPANT:mailto:alicia@example.com";
        let prop = parse_property_line(line).expect("parse attendee");
        assert_eq!(prop.name, "ATTENDEE");
        assert_eq!(prop.value, "mailto:alicia@example.com");
        assert_eq!(
            prop.params.get("CN").unwrap(),
            &vec!["Alicia Vance".to_string()]
        );
        assert_eq!(
            prop.params.get("ROLE").unwrap(),
            &vec!["REQ-PARTICIPANT".to_string()]
        );
    }

    #[test]
    fn test_compare_ics_identical() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:123\r\nSUMMARY:Test\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let diff = compare_ics(ics, ics);
        assert_eq!(diff.matches.len(), 3);
        assert!(diff.differences.is_empty());
        assert!(diff.server_only.is_empty());
        assert!(diff.local_only.is_empty());
    }

    #[test]
    fn test_compare_ics_normalizes_split_and_comma_categories() {
        let server = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:123\r\nCATEGORIES:Engineering\r\nCATEGORIES:Rust\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let local = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:123\r\nCATEGORIES:Rust,Engineering\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let diff = compare_ics(server, local);
        assert!(
            diff.matches
                .contains(&"VCALENDAR/VEVENT[master]/CATEGORIES".to_string())
        );
        assert!(diff.differences.is_empty());
        assert!(diff.server_only.is_empty());
        assert!(diff.local_only.is_empty());
    }

    #[test]
    fn test_compare_ics_differences_and_exclusive_fields() {
        let server = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:server-prod\r\nBEGIN:VEVENT\r\nUID:123\r\nSUMMARY:Meeting\r\nSTATUS:CONFIRMED\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let local = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:local-prod\r\nBEGIN:VEVENT\r\nUID:123\r\nSUMMARY:Meeting\r\nPRIORITY:1\r\nEND:VEVENT\r\nEND:VCALENDAR";
        let diff = compare_ics(server, local);

        assert!(diff.matches.contains(&"VCALENDAR/VERSION".to_string()));
        assert!(
            diff.matches
                .contains(&"VCALENDAR/VEVENT[master]/UID".to_string())
        );
        assert!(
            diff.matches
                .contains(&"VCALENDAR/VEVENT[master]/SUMMARY".to_string())
        );
        assert!(diff.differences.contains_key("VCALENDAR/PRODID"));
        assert!(
            diff.server_only
                .contains_key("VCALENDAR/VEVENT[master]/STATUS")
        );
        assert!(
            diff.local_only
                .contains_key("VCALENDAR/VEVENT[master]/PRIORITY")
        );
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
        assert_eq!(count, 12);
    }

    #[test]
    fn test_extract_xml_tag() {
        let xml = "<D:response><D:href>/dav/cal/admin/</D:href></D:response>";
        assert_eq!(
            extract_xml_tag(xml, "href"),
            Some("/dav/cal/admin/".to_string())
        );
        let xml_unprefixed = "<item><href>http://example.com/cal/</href></item>";
        assert_eq!(
            extract_xml_tag(xml_unprefixed, "href"),
            Some("http://example.com/cal/".to_string())
        );
    }

    #[test]
    fn test_discover_collection_url_fallback() {
        let agent = make_agent(Duration::from_millis(100));
        let col = discover_collection_url(&agent, "http://127.0.0.1:1", "alice", "Basic Og==");
        assert_eq!(
            col,
            "http://127.0.0.1:1/dav/cal/alice%40example.internal/default/"
        );

        let col_email = discover_collection_url(
            &agent,
            "http://127.0.0.1:1",
            "bob@example.com",
            "Basic Og==",
        );
        assert_eq!(
            col_email,
            "http://127.0.0.1:1/dav/cal/bob%40example.com/default/"
        );
    }

    #[test]
    fn test_resolve_well_known_caldav_fallback() {
        let endpoint = resolve_well_known_caldav("http://127.0.0.1:1", "Basic Og==");
        assert_eq!(endpoint, "http://127.0.0.1:1/dav/cal");
    }

    #[test]
    fn test_discover_collection_url_resolves_via_current_user_principal() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("start mock server");
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap();
        let origin = format!("http://127.0.0.1:{port}");
        let handle = std::thread::spawn(move || {
            while let Ok(req) = server.recv() {
                let url = req.url().to_string();
                let method = req.method().as_str().to_string();
                if method == "GET" && url == "/.well-known/caldav" {
                    let resp = tiny_http::Response::from_string("")
                        .with_status_code(301)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Location"[..], &b"/dav/calendars"[..])
                                .unwrap(),
                        );
                    let _ = req.respond(resp);
                    continue;
                }
                if method == "PROPFIND" && url == "/dav/calendars" {
                    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                               <D:multistatus xmlns:D=\"DAV:\">\
                               <D:response>\
                               <D:href>/dav/calendars</D:href>\
                               <D:propstat>\
                               <D:prop>\
                               <D:current-user-principal>\
                               <D:href>/dav/principals/user/writer/</D:href>\
                               </D:current-user-principal>\
                               </D:prop>\
                               <D:status>HTTP/1.1 200 OK</D:status>\
                               </D:propstat>\
                               </D:response>\
                               </D:multistatus>";
                    let resp = tiny_http::Response::from_string(xml).with_status_code(207);
                    let _ = req.respond(resp);
                    continue;
                }
                if method == "PROPFIND" && url == "/dav/principals/user/writer/" {
                    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                               <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                               <D:response>\
                               <D:href>/dav/principals/user/writer/</D:href>\
                               <D:propstat>\
                               <D:prop>\
                               <C:calendar-home-set>\
                               <D:href>/dav/calendars/user/writer/</D:href>\
                               </C:calendar-home-set>\
                               </D:prop>\
                               <D:status>HTTP/1.1 200 OK</D:status>\
                               </D:propstat>\
                               </D:response>\
                               </D:multistatus>";
                    let resp = tiny_http::Response::from_string(xml).with_status_code(207);
                    let _ = req.respond(resp);
                    continue;
                }
                if method == "PROPFIND" && url == "/dav/calendars/user/writer/" {
                    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                               <D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
                               <D:response>\
                               <D:href>/dav/calendars/user/writer/</D:href>\
                               <D:propstat>\
                               <D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>\
                               <D:status>HTTP/1.1 200 OK</D:status>\
                               </D:propstat>\
                               </D:response>\
                               <D:response>\
                               <D:href>/dav/calendars/user/writer/Default/</D:href>\
                               <D:propstat>\
                               <D:prop><D:resourcetype><D:collection/><C:calendar/></D:resourcetype></D:prop>\
                               <D:status>HTTP/1.1 200 OK</D:status>\
                               </D:propstat>\
                               </D:response>\
                               </D:multistatus>";
                    let resp = tiny_http::Response::from_string(xml).with_status_code(207);
                    let _ = req.respond(resp);
                    continue;
                }
                let resp = tiny_http::Response::from_string("").with_status_code(404);
                let _ = req.respond(resp);
            }
        });
        let agent = make_agent(Duration::from_secs(5));
        let col = discover_collection_url(&agent, &origin, "writer", "Basic Og==");
        assert_eq!(col, format!("{origin}/dav/calendars/user/writer/Default/"));
        drop(handle);
    }
}
