// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! vCard 3.0 syntax: content lines, folding, escaping, parameters.
//!
//! This layer knows nothing about contacts — it turns the byte-level format
//! ([RFC 2426] §2, inheriting the grammar of [RFC 2425] §5) into a list of
//! [`Property`] values and back. The semantic mapping lives in
//! [`crate::contact`].
//!
//! vCard 3.0 rather than 4.0 because that is the only format Evolution's
//! `EVCard` emits (`EVC_FORMAT_VCARD_30`), and every vCard we produce is
//! handed straight to `e_contact_new_from_vcard()`.
//!
//! [RFC 2425]: https://www.rfc-editor.org/rfc/rfc2425
//! [RFC 2426]: https://www.rfc-editor.org/rfc/rfc2426

use crate::error::VCardError;

/// Fold limit in octets, excluding the line break (RFC 2426 §2.6).
const FOLD_AT: usize = 75;

/// One vCard content line.
///
/// The value is kept in its escaped, on-the-wire form so that structured
/// values can be split on their real separators before unescaping; use
/// [`Property::text`] or [`Property::components`] to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// The `item1` in `item1.TEL:…`, if the line was grouped.
    pub group: Option<String>,
    /// Upper-cased property name.
    pub name: String,
    params: Vec<(String, Vec<String>)>,
    value: String,
}

impl Property {
    /// A property with a single text value, escaped on the way in.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: escape(value),
        }
    }

    /// A property whose value is a `;`-separated component list, such as `N`.
    /// Empty trailing components are kept: their position is the meaning.
    pub fn structured<I, S>(name: &str, components: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let value = components
            .into_iter()
            .map(|component| escape(component.as_ref()))
            .collect::<Vec<_>>()
            .join(";");
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value,
        }
    }

    /// Add a single-valued parameter.
    pub fn with_param(self, name: &str, value: &str) -> Self {
        self.with_params(name, [value])
    }

    /// Add a multi-valued parameter, e.g. `TYPE=WORK,PREF`. Adding no values
    /// adds no parameter, so callers can pass a filtered list unconditionally.
    pub fn with_params<I, S>(mut self, name: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let values: Vec<String> = values
            .into_iter()
            .map(|value| value.as_ref().to_owned())
            .collect();
        if !values.is_empty() {
            self.params.push((name.to_ascii_uppercase(), values));
        }
        self
    }

    /// The value as text, with escapes resolved.
    pub fn text(&self) -> String {
        unescape(&self.value)
    }

    /// The value split on unescaped `;`, each component unescaped.
    pub fn components(&self) -> Vec<String> {
        split_unescaped(&self.value, ';')
            .into_iter()
            .map(|component| unescape(&component))
            .collect()
    }

    /// The first value of the named parameter.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.param_values(name).first().copied()
    }

    /// Every value of the named parameter, across repeated occurrences.
    pub fn param_values(&self, name: &str) -> Vec<&str> {
        let name = name.to_ascii_uppercase();
        self.params
            .iter()
            .filter(|(key, _)| *key == name)
            .flat_map(|(_, values)| values.iter().map(String::as_str))
            .collect()
    }

    /// Whether `TYPE` carries the given value, case-insensitively.
    pub fn has_type(&self, value: &str) -> bool {
        self.param_values("TYPE")
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(value))
    }

    fn to_line(&self) -> String {
        let mut line = String::new();
        if let Some(group) = &self.group {
            line.push_str(group);
            line.push('.');
        }
        line.push_str(&self.name);
        for (name, values) in &self.params {
            line.push(';');
            line.push_str(name);
            line.push('=');
            let quoted: Vec<String> = values.iter().map(|value| quote_param(value)).collect();
            line.push_str(&quoted.join(","));
        }
        line.push(':');
        line.push_str(&self.value);
        line
    }
}

/// Serialise properties as a complete vCard, `BEGIN`/`END` included.
pub fn write(properties: &[Property]) -> String {
    let mut out = String::from("BEGIN:VCARD\r\n");
    for property in properties {
        fold_into(&mut out, &property.to_line());
    }
    out.push_str("END:VCARD\r\n");
    out
}

/// Parse a complete vCard into its properties, `BEGIN`/`END` stripped.
pub fn parse(text: &str) -> Result<Vec<Property>, VCardError> {
    let lines = unfold(text);
    let mut lines = lines.iter().map(String::as_str);

    let begin = lines.next().ok_or(VCardError::NotAVCard)?;
    if !begin.eq_ignore_ascii_case("BEGIN:VCARD") {
        return Err(VCardError::NotAVCard);
    }

    let mut properties = Vec::new();
    for line in lines {
        if line.eq_ignore_ascii_case("END:VCARD") {
            return Ok(properties);
        }
        properties.push(parse_line(line)?);
    }
    Err(VCardError::Unterminated)
}

/// Undo line folding and drop blank lines, yielding logical content lines.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        match raw.strip_prefix([' ', '\t']) {
            // A continuation of the previous logical line — but only if
            // there is one; leading whitespace at the top is just noise.
            Some(rest) if !lines.is_empty() => lines.last_mut().expect("non-empty").push_str(rest),
            _ => {
                let line = raw.trim_end_matches('\r');
                if !line.is_empty() {
                    lines.push(line.to_owned());
                }
            }
        }
    }
    lines
}

fn parse_line(line: &str) -> Result<Property, VCardError> {
    let colon = find_unquoted(line, ':').ok_or_else(|| VCardError::Malformed(line.to_owned()))?;
    let (head, value) = line.split_at(colon);
    let value = &value[1..];

    let mut tokens = split_unquoted(head, ';').into_iter();
    let first = tokens.next().unwrap_or_default();
    if first.is_empty() {
        return Err(VCardError::Malformed(line.to_owned()));
    }
    let (group, name) = match first.split_once('.') {
        Some((group, name)) => (Some(group.to_owned()), name.to_ascii_uppercase()),
        None => (None, first.to_ascii_uppercase()),
    };

    let mut params: Vec<(String, Vec<String>)> = Vec::new();
    for token in tokens {
        match token.split_once('=') {
            Some((key, values)) => params.push((
                key.to_ascii_uppercase(),
                split_unquoted(values, ',')
                    .iter()
                    .map(|value| unquote_param(value))
                    .collect(),
            )),
            // vCard 2.1 wrote bare type values (`EMAIL;INTERNET:…`), and
            // exporters still do. Read them as TYPE rather than discard them.
            None => params.push(("TYPE".to_owned(), vec![unquote_param(&token)])),
        }
    }

    Ok(Property {
        group,
        name,
        params,
        value: value.to_owned(),
    })
}

/// Append a content line, folded to [`FOLD_AT`] octets. Folds land on
/// character boundaries: a continuation that split a UTF-8 sequence would
/// make the whole vCard undecodable.
fn fold_into(out: &mut String, line: &str) {
    let mut budget = FOLD_AT;
    let mut used = 0;
    for character in line.chars() {
        if used + character.len_utf8() > budget {
            out.push_str("\r\n ");
            // The continuation's leading space counts against the limit.
            budget = FOLD_AT - 1;
            used = 0;
        }
        out.push(character);
        used += character.len_utf8();
    }
    out.push_str("\r\n");
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(character),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n' | 'N') => out.push('\n'),
            // An unknown escape stands for the character itself; that keeps
            // `\;`, `\,`, `\\` right and never loses data on the odd input.
            Some(escaped) => out.push(escaped),
            None => out.push('\\'),
        }
    }
    out
}

/// Split on a separator that is not preceded by a backslash.
fn split_unescaped(value: &str, separator: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for character in value.chars() {
        let current = parts.last_mut().expect("non-empty");
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            current.push(character);
            escaped = true;
        } else if character == separator {
            parts.push(String::new());
        } else {
            current.push(character);
        }
    }
    parts
}

/// Split on a separator that is not inside a double-quoted parameter value.
fn split_unquoted(value: &str, separator: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut quoted = false;
    for character in value.chars() {
        if character == '"' {
            quoted = !quoted;
        }
        if character == separator && !quoted {
            parts.push(String::new());
        } else {
            parts.last_mut().expect("non-empty").push(character);
        }
    }
    parts
}

fn find_unquoted(value: &str, separator: char) -> Option<usize> {
    let mut quoted = false;
    for (index, character) in value.char_indices() {
        if character == '"' {
            quoted = !quoted;
        } else if character == separator && !quoted {
            return Some(index);
        }
    }
    None
}

fn quote_param(value: &str) -> String {
    // RFC 2425 §5.8.2: a parameter value containing the structural
    // characters has to be quoted. There is no escape inside the quotes, so
    // an embedded quote can only be dropped.
    let value = value.replace('"', "");
    if value.contains([';', ':', ',']) {
        format!("\"{value}\"")
    } else {
        value
    }
}

fn unquote_param(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_owned()
}
