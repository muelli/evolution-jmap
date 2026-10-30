// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! SieveScript get, set, query, and validate methods (RFC 9661).

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::Id;
use jmap_proto::error::{self, MethodError, SetError};
use jmap_proto::methods::{GetRequest, GetResponse, QueryRequest, QueryResponse, SetResponse};
use jmap_proto::sieve::{
    SieveScript, SieveScriptQueryFilter, SieveScriptSetRequest, SieveScriptValidateRequest,
    SieveScriptValidateResponse, sieve_set_error,
};
use serde_json::Value;

use crate::dispatch::{account_mut, parse_arguments, to_result};
use crate::patch::apply_patch;
use crate::state::{AccountState, ServerState};

const VACATION_SCRIPT_CONTENT_TYPE: &str = "application/sieve";
const VACATION_SCRIPT_BODY: &str = "keep;\n";

#[derive(Clone)]
struct SieveSettings {
    max_size_script: u64,
    max_number_scripts: Option<u64>,
    max_size_script_name: u64,
    extensions: BTreeSet<String>,
    vacation_script_mode: bool,
    vacation_script_name: String,
}

impl SieveSettings {
    fn from_state(state: &ServerState) -> Self {
        Self {
            max_size_script: state.sieve_max_size_script,
            max_number_scripts: state.sieve_max_number_scripts,
            max_size_script_name: state.sieve_max_size_script_name,
            extensions: state
                .sieve_extensions
                .iter()
                .map(|extension| extension.to_ascii_lowercase())
                .collect(),
            vacation_script_mode: state.sieve_vacation_script,
            vacation_script_name: state.sieve_vacation_script_name.clone(),
        }
    }
}

#[derive(Clone)]
struct SieveToken {
    line: usize,
    kind: SieveTokenKind,
}

#[derive(Clone)]
enum SieveTokenKind {
    Word(String),
    StringLiteral(String),
    Symbol(char),
}

/// `SieveScript/get` (RFC 9661 §2.3).
/// Return all scripts when `ids` is null.
/// List unknown ids in `notFound`.
pub fn sieve_script_get(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: GetRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &request.ids {
        None => list.extend(
            account
                .sieve_scripts
                .iter()
                .map(|(_, script)| script.clone()),
        ),
        Some(ids) => {
            for id in ids {
                match account.sieve_scripts.get(id) {
                    Some(script) => list.push(script.clone()),
                    None => not_found.push(id.clone()),
                }
            }
        }
    }

    to_result(
        &GetResponse::new(request.account_id, account.sieve_scripts.state(), list)
            .with_not_found(not_found),
    )
}

/// `SieveScript/set` (RFC 9661 §2.4).
/// Treat `id` and `isActive` as server-managed fields.
/// Reject duplicate names with `alreadyExists`.
/// Reject destruction of the active script with `sieveIsActive`.
pub fn sieve_script_set(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let settings = SieveSettings::from_state(state);

    let request: SieveScriptSetRequest = parse_arguments(arguments)?;
    let SieveScriptSetRequest {
        set,
        on_success_activate_script,
        on_success_deactivate_script,
        ..
    } = request;
    let account_id = set.account_id.clone();
    let account = account_mut(state, &account_id)?;

    let old_state = account.sieve_scripts.state();
    if let Some(expected) = &set.if_in_state
        && expected != &old_state
    {
        return Err(MethodError::new(error::method::STATE_MISMATCH));
    }

    let mut names: BTreeMap<Id, String> = account
        .sieve_scripts
        .iter()
        .map(|(id, script)| (id.clone(), script.name.clone()))
        .collect();
    let active_id = account
        .sieve_scripts
        .iter()
        .find(|(_, script)| script.is_active)
        .map(|(id, _)| id.clone());

    let prospective_destroy_count = set
        .destroy
        .as_ref()
        .map(|destroy| {
            destroy
                .iter()
                .filter(|id| {
                    let Some(script) = account.sieve_scripts.get(id) else {
                        return false;
                    };
                    active_id.as_ref() != Some(id)
                        && !is_protected_vacation_script(&settings, script)
                })
                .cloned()
                .collect::<BTreeSet<Id>>()
                .len()
        })
        .unwrap_or(0);
    let scripts_after_destroy = account
        .sieve_scripts
        .len()
        .saturating_sub(prospective_destroy_count);

    let mut created: BTreeMap<String, SieveScript> = BTreeMap::new();
    let mut not_created: BTreeMap<String, SetError> = BTreeMap::new();
    let mut to_create: Vec<(Id, SieveScript)> = Vec::new();
    let mut created_here: BTreeMap<String, Id> = BTreeMap::new();
    for (creation_id, mut script) in set.create.unwrap_or_default() {
        if script.id.is_some() {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES)
                    .with_description("id is set by the server and must not be given in a create"),
            );
            continue;
        }
        if script.is_active {
            not_created.insert(
                creation_id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isActive is set by the server; activate via onSuccessActivateScript",
                ),
            );
            continue;
        }
        if let Some(limit) = settings.max_number_scripts
            && (scripts_after_destroy + to_create.len() + 1) as u64 > limit
        {
            not_created.insert(
                creation_id,
                SetError::new(error::set::OVER_QUOTA).with_description(format!(
                    "creating this script exceeds maxNumberScripts ({limit})"
                )),
            );
            continue;
        }
        if names.values().any(|name| name == &script.name) {
            not_created.insert(creation_id, SetError::new(error::set::ALREADY_EXISTS));
            continue;
        }
        if let Err(set_error) = validate_script_for_set(account, &script, &settings) {
            not_created.insert(creation_id, set_error);
            continue;
        }

        let id = account.sieve_scripts.alloc_id();
        script.id = Some(id.clone());
        names.insert(id.clone(), script.name.clone());
        created_here.insert(creation_id.clone(), id.clone());
        created.insert(creation_id, script.clone());
        to_create.push((id, script));
    }

    let mut updated: BTreeMap<Id, Option<SieveScript>> = BTreeMap::new();
    let mut not_updated: BTreeMap<Id, SetError> = BTreeMap::new();
    let mut to_update: Vec<(Id, SieveScript)> = Vec::new();
    for (id, patch) in set.update.unwrap_or_default() {
        let Some(existing) = account.sieve_scripts.get(&id) else {
            not_updated.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        let Some(patch_map) = patch.as_object() else {
            not_updated.insert(id, SetError::new(error::set::INVALID_PATCH));
            continue;
        };
        let mut value = serde_json::to_value(existing).map_err(|e| {
            MethodError::new(error::method::SERVER_FAIL).with_description(e.to_string())
        })?;
        let patched = match apply_patch(&mut value, patch_map)
            .map_err(|message| SetError::new(error::set::INVALID_PATCH).with_description(message))
            .and_then(|()| {
                serde_json::from_value::<SieveScript>(value).map_err(|e| {
                    SetError::new(error::set::INVALID_PATCH).with_description(e.to_string())
                })
            }) {
            Ok(patched) => patched,
            Err(set_error) => {
                not_updated.insert(id, set_error);
                continue;
            }
        };
        if patched.id.as_ref() != Some(&id) {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description("id is immutable"),
            );
            continue;
        }
        if patched.is_active != existing.is_active {
            not_updated.insert(
                id,
                SetError::new(error::set::INVALID_PROPERTIES).with_description(
                    "isActive is set by the server; activate via onSuccessActivateScript",
                ),
            );
            continue;
        }
        if patched.name != existing.name
            && names
                .iter()
                .any(|(other, name)| other != &id && name == &patched.name)
        {
            not_updated.insert(id, SetError::new(error::set::ALREADY_EXISTS));
            continue;
        }
        if let Err(set_error) = validate_script_for_set(account, &patched, &settings) {
            not_updated.insert(id, set_error);
            continue;
        }

        names.insert(id.clone(), patched.name.clone());
        to_update.push((id, patched));
    }

    let mut destroyed: Vec<Id> = Vec::new();
    let mut not_destroyed: BTreeMap<Id, SetError> = BTreeMap::new();
    for id in set.destroy.unwrap_or_default() {
        let Some(script) = account.sieve_scripts.get(&id) else {
            not_destroyed.insert(id, SetError::new(error::set::NOT_FOUND));
            continue;
        };
        if is_protected_vacation_script(&settings, script) {
            not_destroyed.insert(
                id,
                SetError::new(error::set::FORBIDDEN)
                    .with_description("vacation script is server-managed and cannot be destroyed"),
            );
        } else if active_id.as_ref() == Some(&id) {
            not_destroyed.insert(id, SetError::new(sieve_set_error::SIEVE_IS_ACTIVE));
        } else {
            destroyed.push(id);
        }
    }

    account.sieve_scripts.transaction(|transaction| {
        for (id, script) in to_create {
            transaction.create(id, script);
        }
        for (id, script) in to_update {
            transaction.update(&id, script);
            updated.insert(id, None);
        }
        for id in &destroyed {
            transaction.destroy(id);
        }
    });

    if let Some(target) = on_success_activate_script {
        let resolved = match target {
            Value::Null => None,
            Value::String(reference) => {
                let id = match reference.strip_prefix('#') {
                    Some(creation_id) => created_here.get(creation_id).cloned(),
                    None => Some(Id::new(reference.clone())),
                };
                match id {
                    Some(id) if account.sieve_scripts.contains(&id) => Some(id),
                    _ => {
                        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                            .with_description("onSuccessActivateScript names an unknown script"));
                    }
                }
            }
            _ => {
                return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
                    .with_description("onSuccessActivateScript must be a string or null"));
            }
        };
        activate(account, resolved.as_ref());
    } else if on_success_deactivate_script == Some(true) {
        activate(account, None);
    }

    let mut response =
        SetResponse::new(account_id, account.sieve_scripts.state()).with_old_state(old_state);
    if !created.is_empty() {
        response = response.with_created(created);
    }
    if !updated.is_empty() {
        response = response.with_updated(updated);
    }
    if !destroyed.is_empty() {
        response = response.with_destroyed(destroyed);
    }
    if !not_created.is_empty() {
        response = response.with_not_created(not_created);
    }
    if !not_updated.is_empty() {
        response = response.with_not_updated(not_updated);
    }
    if !not_destroyed.is_empty() {
        response = response.with_not_destroyed(not_destroyed);
    }
    to_result(&response)
}

/// `SieveScript/query` (RFC 9661 §2.5).
/// Filter by `name` substring, by `isActive`, or both.
pub fn sieve_script_query(state: &mut ServerState, arguments: Value) -> Result<Value, MethodError> {
    let request: QueryRequest<SieveScriptQueryFilter> = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let filter = request.filter.unwrap_or_default();
    let ids: Vec<Id> = account
        .sieve_scripts
        .iter()
        .filter(|(_, script)| sieve_script_matches(script, &filter))
        .map(|(id, _)| id.clone())
        .skip(request.position.max(0) as usize)
        .take(request.limit.unwrap_or(u64::MAX) as usize)
        .collect();

    let total = account
        .sieve_scripts
        .iter()
        .filter(|(_, script)| sieve_script_matches(script, &filter))
        .count() as u64;

    let mut response = QueryResponse::new(request.account_id, account.sieve_scripts.state(), ids)
        .with_position(request.position.max(0) as u64);
    if request.calculate_total {
        response = response.with_total(total);
    }
    to_result(&response)
}

fn sieve_script_matches(script: &SieveScript, filter: &SieveScriptQueryFilter) -> bool {
    if let Some(name) = &filter.name
        && !script.name.contains(name.as_str())
    {
        return false;
    }
    if let Some(is_active) = filter.is_active
        && script.is_active != is_active
    {
        return false;
    }
    true
}

/// `SieveScript/validate` (RFC 9661 §2.6).
pub fn sieve_script_validate(
    state: &mut ServerState,
    arguments: Value,
) -> Result<Value, MethodError> {
    let settings = SieveSettings::from_state(state);

    let request: SieveScriptValidateRequest = parse_arguments(arguments)?;
    let account = account_mut(state, &request.account_id)?;

    let Some(blob) = account.blobs.get(&request.blob_id) else {
        return Err(MethodError::new(error::method::INVALID_ARGUMENTS)
            .with_description("blobId is not a known blob"));
    };

    match validate_sieve_blob(&blob.data, &settings) {
        Ok(()) => to_result(&SieveScriptValidateResponse::valid(
            request.account_id.clone(),
        )),
        Err(set_error) => to_result(&SieveScriptValidateResponse::invalid(
            request.account_id.clone(),
            set_error,
        )),
    }
}

/// Keep one active server-managed vacation script for fallback Stalwart parity.
pub(crate) fn ensure_vacation_script(account: &mut AccountState, script_name: &str) {
    let blob_id = account.add_blob(
        VACATION_SCRIPT_CONTENT_TYPE,
        VACATION_SCRIPT_BODY.as_bytes().to_vec(),
    );
    let existing_id = account
        .sieve_scripts
        .iter()
        .find(|(_, script)| script.name == script_name)
        .map(|(id, _)| id.clone());

    let mut all_ids: Vec<Id> = account
        .sieve_scripts
        .iter()
        .map(|(id, _)| id.clone())
        .collect();

    account.sieve_scripts.transaction(|transaction| {
        let active_id = if let Some(existing_id) = existing_id.clone() {
            let Some(existing) = transaction.get(&existing_id) else {
                return;
            };
            let mut updated = existing.clone();
            updated.blob_id = blob_id.clone();
            updated.is_active = true;
            transaction.update(&existing_id, updated);
            existing_id
        } else {
            let id = transaction.alloc_id();
            let script = SieveScript::new(script_name, blob_id.clone())
                .with_id(id.clone())
                .is_active(true);
            transaction.create(id.clone(), script);
            all_ids.push(id.clone());
            id
        };

        for id in &all_ids {
            if id == &active_id {
                continue;
            }
            let Some(script) = transaction.get(id) else {
                continue;
            };
            if script.is_active {
                let mut updated = script.clone();
                updated.is_active = false;
                transaction.update(id, updated);
            }
        }
    });
}

/// Activate only `target` per RFC 9661 §2.4 `onSuccessActivateScript`.
/// `None` deactivates all scripts.
fn activate(account: &mut AccountState, target: Option<&Id>) {
    let ids: Vec<Id> = account
        .sieve_scripts
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    account.sieve_scripts.transaction(|transaction| {
        for id in &ids {
            let Some(script) = transaction.get(id) else {
                continue;
            };
            let should_be_active = target == Some(id);
            if script.is_active != should_be_active {
                let mut updated = script.clone();
                updated.is_active = should_be_active;
                transaction.update(id, updated);
            }
        }
    });
}

fn is_protected_vacation_script(settings: &SieveSettings, script: &SieveScript) -> bool {
    settings.vacation_script_mode && script.name == settings.vacation_script_name
}

fn validate_script_for_set(
    account: &AccountState,
    script: &SieveScript,
    settings: &SieveSettings,
) -> Result<(), SetError> {
    validate_script_name(script, settings)?;

    let Some(blob) = account.blobs.get(&script.blob_id) else {
        return Ok(());
    };
    validate_sieve_blob(&blob.data, settings)
}

fn validate_script_name(script: &SieveScript, settings: &SieveSettings) -> Result<(), SetError> {
    let octets = script.name.len() as u64;
    if octets > settings.max_size_script_name {
        return Err(
            SetError::new(error::set::TOO_LARGE).with_description(format!(
                "name is {octets} octets, more than maxSizeScriptName {}",
                settings.max_size_script_name,
            )),
        );
    }
    Ok(())
}

fn validate_sieve_blob(blob: &[u8], settings: &SieveSettings) -> Result<(), SetError> {
    let octets = blob.len() as u64;
    if octets > settings.max_size_script {
        return Err(
            SetError::new(error::set::TOO_LARGE).with_description(format!(
                "blob is {octets} octets, more than maxSizeScript {}",
                settings.max_size_script,
            )),
        );
    }

    let source =
        std::str::from_utf8(blob).map_err(|_| invalid_sieve(1, "script is not valid UTF-8"))?;
    validate_sieve_source(source, settings)
}

fn validate_sieve_source(source: &str, settings: &SieveSettings) -> Result<(), SetError> {
    let tokens = tokenize_sieve(source)?;
    let mut declared_extensions = BTreeSet::new();
    let mut extension_usage_lines: BTreeMap<String, usize> = BTreeMap::new();

    let mut statement_start: Option<usize> = None;
    let mut statement: Vec<&SieveToken> = Vec::new();

    for token in &tokens {
        let lone_block_close = statement_start.is_none() && is_symbol(token, '}');
        if !lone_block_close {
            statement_start.get_or_insert(token.line);
            statement.push(token);
        }

        if is_symbol(token, ';') {
            let content_len = statement.len().saturating_sub(1);
            validate_statement(
                &statement[..content_len],
                settings,
                &mut declared_extensions,
                &mut extension_usage_lines,
            )?;
            statement.clear();
            statement_start = None;
            continue;
        }

        if is_symbol(token, '{') || is_symbol(token, '}') {
            statement.clear();
            statement_start = None;
        }
    }

    if let Some(line) = statement_start {
        return Err(invalid_sieve(
            line,
            "statement must end with ';' or a block",
        ));
    }

    for (extension, line) in extension_usage_lines {
        if !declared_extensions.contains(&extension) {
            return Err(invalid_sieve(
                line,
                format!("missing require for extension \"{extension}\""),
            ));
        }
    }

    Ok(())
}

fn validate_statement(
    statement: &[&SieveToken],
    settings: &SieveSettings,
    declared_extensions: &mut BTreeSet<String>,
    extension_usage_lines: &mut BTreeMap<String, usize>,
) -> Result<(), SetError> {
    if statement.is_empty() {
        return Ok(());
    }

    if statement_is_require(statement) {
        parse_require_statement(statement, settings, declared_extensions)?;
        return Ok(());
    }

    if let Some(line) = first_word_line(statement, "fileinto") {
        mark_extension_usage(extension_usage_lines, "fileinto", line);
        if statement_has_tag(statement, "copy") {
            mark_extension_usage(extension_usage_lines, "copy", line);
        }
        if statement_has_tag(statement, "mailboxid") {
            mark_extension_usage(extension_usage_lines, "mailboxid", line);
        }
    }

    if let Some(line) = first_word_line(statement, "addflag") {
        mark_extension_usage(extension_usage_lines, "imap4flags", line);
    }

    if let Some(line) = first_word_line(statement, "redirect")
        && statement_has_tag(statement, "copy")
    {
        mark_extension_usage(extension_usage_lines, "copy", line);
    }

    Ok(())
}

fn statement_is_require(statement: &[&SieveToken]) -> bool {
    matches!(
        statement.first(),
        Some(SieveToken {
            kind: SieveTokenKind::Word(word),
            ..
        }) if word.eq_ignore_ascii_case("require")
    )
}

fn parse_require_statement(
    statement: &[&SieveToken],
    settings: &SieveSettings,
    declared_extensions: &mut BTreeSet<String>,
) -> Result<(), SetError> {
    let require_line = statement.first().map(|token| token.line).unwrap_or(1);
    if statement.len() < 2 || !is_symbol(statement[1], '[') {
        return Err(invalid_sieve(
            require_line,
            "require must use a bracketed extension list",
        ));
    }

    let mut cursor = 2;
    let mut expect_string = true;
    let mut closed = false;

    while cursor < statement.len() {
        let token = statement[cursor];
        if expect_string {
            if is_symbol(token, ']') {
                closed = true;
                cursor += 1;
                break;
            }
            let SieveTokenKind::StringLiteral(extension) = &token.kind else {
                return Err(invalid_sieve(
                    token.line,
                    "require expects extension names in double quotes",
                ));
            };
            let normalized = extension.to_ascii_lowercase();
            if !settings.extensions.contains(&normalized) {
                return Err(invalid_sieve(
                    token.line,
                    format!("unsupported extension in require: \"{normalized}\""),
                ));
            }
            declared_extensions.insert(normalized);
            expect_string = false;
            cursor += 1;
            continue;
        }

        if is_symbol(token, ',') {
            expect_string = true;
            cursor += 1;
            continue;
        }
        if is_symbol(token, ']') {
            closed = true;
            cursor += 1;
            break;
        }
        return Err(invalid_sieve(
            token.line,
            "require list must separate names with commas",
        ));
    }

    if !closed {
        return Err(invalid_sieve(
            require_line,
            "require list is missing a closing ']'",
        ));
    }
    if cursor != statement.len() {
        return Err(invalid_sieve(
            statement[cursor].line,
            "unexpected token after require list",
        ));
    }
    Ok(())
}

fn first_word_line(statement: &[&SieveToken], needle: &str) -> Option<usize> {
    statement.iter().find_map(|token| match &token.kind {
        SieveTokenKind::Word(word) if word.eq_ignore_ascii_case(needle) => Some(token.line),
        _ => None,
    })
}

fn statement_has_tag(statement: &[&SieveToken], needle: &str) -> bool {
    statement
        .windows(2)
        .any(|window| is_symbol(window[0], ':') && token_is_word(window[1], needle))
}

fn mark_extension_usage(
    extension_usage_lines: &mut BTreeMap<String, usize>,
    extension: &str,
    line: usize,
) {
    extension_usage_lines
        .entry(extension.to_owned())
        .or_insert(line);
}

fn token_is_word(token: &SieveToken, expected: &str) -> bool {
    matches!(&token.kind, SieveTokenKind::Word(word) if word.eq_ignore_ascii_case(expected))
}

fn is_symbol(token: &SieveToken, expected: char) -> bool {
    matches!(token.kind, SieveTokenKind::Symbol(symbol) if symbol == expected)
}

fn tokenize_sieve(source: &str) -> Result<Vec<SieveToken>, SetError> {
    let mut tokens = Vec::new();
    let mut current_word = String::new();
    let mut current_string = String::new();

    let mut line = 1usize;
    let mut in_comment = false;
    let mut in_string = false;
    let mut escaped = false;
    let mut brace_depth = 0i64;
    let mut bracket_depth = 0i64;

    for ch in source.chars() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
                line += 1;
            }
            continue;
        }

        if in_string {
            if escaped {
                current_string.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::StringLiteral(current_string.clone()),
                });
                current_string.clear();
                in_string = false;
                continue;
            }
            if ch == '\n' {
                line += 1;
            }
            current_string.push(ch);
            continue;
        }

        match ch {
            '#' => {
                flush_word(&mut tokens, &mut current_word, line);
                in_comment = true;
            }
            '"' => {
                flush_word(&mut tokens, &mut current_word, line);
                in_string = true;
                escaped = false;
                current_string.clear();
            }
            '{' => {
                flush_word(&mut tokens, &mut current_word, line);
                brace_depth += 1;
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::Symbol(ch),
                });
            }
            '}' => {
                flush_word(&mut tokens, &mut current_word, line);
                if brace_depth == 0 {
                    return Err(invalid_sieve(line, "unmatched '}'"));
                }
                brace_depth -= 1;
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::Symbol(ch),
                });
            }
            '[' => {
                flush_word(&mut tokens, &mut current_word, line);
                bracket_depth += 1;
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::Symbol(ch),
                });
            }
            ']' => {
                flush_word(&mut tokens, &mut current_word, line);
                if bracket_depth == 0 {
                    return Err(invalid_sieve(line, "unmatched ']'"));
                }
                bracket_depth -= 1;
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::Symbol(ch),
                });
            }
            ';' | ':' | ',' => {
                flush_word(&mut tokens, &mut current_word, line);
                tokens.push(SieveToken {
                    line,
                    kind: SieveTokenKind::Symbol(ch),
                });
            }
            '\n' => {
                flush_word(&mut tokens, &mut current_word, line);
                line += 1;
            }
            c if c.is_whitespace() => {
                flush_word(&mut tokens, &mut current_word, line);
            }
            other => current_word.push(other),
        }
    }

    flush_word(&mut tokens, &mut current_word, line);

    if in_string {
        return Err(invalid_sieve(line, "unterminated string literal"));
    }
    if brace_depth != 0 {
        return Err(invalid_sieve(line, "unbalanced '{' and '}'"));
    }
    if bracket_depth != 0 {
        return Err(invalid_sieve(line, "unbalanced '[' and ']'"));
    }

    Ok(tokens)
}

fn flush_word(tokens: &mut Vec<SieveToken>, current_word: &mut String, line: usize) {
    if current_word.is_empty() {
        return;
    }
    tokens.push(SieveToken {
        line,
        kind: SieveTokenKind::Word(std::mem::take(current_word)),
    });
}

fn invalid_sieve(line: usize, message: impl Into<String>) -> SetError {
    SetError::new(sieve_set_error::INVALID_SIEVE)
        .with_description(format!("line {line}: {}", message.into()))
}
