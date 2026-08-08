// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! iCalendar syntax: components, content lines, folding, escaping, parameters.
//!
//! This layer knows nothing about events — it turns the byte-level format
//! ([RFC 5545] §3.1) into a tree of [`Component`]s holding [`Property`]
//! values, and back. The semantic mapping lives above it.
//!
//! Two things separate this from the vCard grammar its shape is borrowed
//! from: components nest (a `VEVENT` inside a `VCALENDAR`, a `VALARM` inside
//! that), and only TEXT-typed values are escaped — `DTSTART`, `DURATION` and
//! `RRULE` carry their own punctuation, so [`Property::raw`] keeps them
//! verbatim while [`Property::new`] escapes.
//!
//! [RFC 5545]: https://www.rfc-editor.org/rfc/rfc5545

use crate::error::ICalError;

/// Fold limit in octets, excluding the line break (RFC 5545 §3.1).
const FOLD_AT: usize = 75;

/// One iCalendar content line.
///
/// The value is kept in its on-the-wire form so that structured values can be
/// split on their real separators before unescaping; use [`Property::text`],
/// [`Property::texts`] or [`Property::raw_value`] to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Upper-cased property name.
    pub name: String,
    params: Vec<(String, Vec<String>)>,
    value: String,
}

impl Property {
    /// A property with a single TEXT value, escaped on the way in.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: escape(value),
        }
    }

    /// A property whose value is not TEXT — `DTSTART`, `DURATION`, `RRULE`,
    /// `TRIGGER` — kept verbatim, because its separators are structure.
    pub fn raw(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.to_owned(),
        }
    }

    /// Add a single-valued parameter.
    pub fn with_param(self, name: &str, value: &str) -> Self {
        self.with_params(name, [value])
    }

    /// Add a multi-valued parameter, e.g. `DELEGATED-TO=a,b`. Adding no
    /// values adds no parameter, so callers can pass a filtered list
    /// unconditionally.
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

    /// The value split on unescaped `,`, each part unescaped: a TEXT list
    /// such as `CATEGORIES`.
    pub fn texts(&self) -> Vec<String> {
        split_unescaped(&self.value, ',')
            .iter()
            .map(|part| unescape(part))
            .collect()
    }

    /// The value exactly as it appears on the wire.
    pub fn raw_value(&self) -> &str {
        &self.value
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

    fn to_line(&self) -> String {
        let mut line = String::from(&self.name);
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

/// One iCalendar component: `VCALENDAR`, `VEVENT`, `VALARM`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// Upper-cased component name.
    pub name: String,
    /// The component's own content lines, in file order.
    pub properties: Vec<Property>,
    /// Nested components, in file order.
    pub children: Vec<Component>,
}

impl Component {
    /// An empty component.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            properties: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Append a property.
    pub fn with(mut self, property: Property) -> Self {
        self.properties.push(property);
        self
    }

    /// Append a nested component.
    pub fn with_child(mut self, child: Component) -> Self {
        self.children.push(child);
        self
    }

    /// The first property with this name.
    pub fn property(&self, name: &str) -> Option<&Property> {
        let name = name.to_ascii_uppercase();
        self.properties
            .iter()
            .find(|property| property.name == name)
    }

    /// Every property with this name, in file order.
    pub fn all(&self, name: &str) -> Vec<&Property> {
        let name = name.to_ascii_uppercase();
        self.properties
            .iter()
            .filter(|property| property.name == name)
            .collect()
    }

    /// The text value of the first property with this name.
    pub fn text(&self, name: &str) -> Option<String> {
        self.property(name).map(Property::text)
    }

    /// The first nested component with this name.
    pub fn child(&self, name: &str) -> Option<&Component> {
        let name = name.to_ascii_uppercase();
        self.children.iter().find(|child| child.name == name)
    }

    /// Serialise as iCalendar text, `BEGIN`/`END` included.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        out.push_str("BEGIN:");
        out.push_str(&self.name);
        out.push_str("\r\n");
        for property in &self.properties {
            fold_into(out, &property.to_line());
        }
        for child in &self.children {
            child.write_into(out);
        }
        out.push_str("END:");
        out.push_str(&self.name);
        out.push_str("\r\n");
    }
}

/// Parse a complete `VCALENDAR` object.
pub fn parse(text: &str) -> Result<Component, ICalError> {
    let lines = unfold(text);
    let mut lines = lines.iter().map(String::as_str);

    let first = lines.next().ok_or(ICalError::NotACalendar)?;
    if !begins(first).is_some_and(|name| name == "VCALENDAR") {
        return Err(ICalError::NotACalendar);
    }

    // The innermost open component is the last entry; the first is the
    // VCALENDAR, and popping it ends the object.
    let mut open = vec![Component::new("VCALENDAR")];
    while let Some(line) = lines.next() {
        if let Some(name) = begins(line) {
            open.push(Component::new(&name));
            continue;
        }
        if let Some(name) = ends(line) {
            let finished = open.pop().expect("a component is open");
            if finished.name != name {
                return Err(ICalError::Mismatched {
                    expected: finished.name,
                    found: name,
                });
            }
            match open.last_mut() {
                Some(parent) => parent.children.push(finished),
                // The calendar itself just closed: nothing may follow it.
                None => {
                    return match lines.next() {
                        Some(trailing) => Err(ICalError::Trailing(trailing.to_owned())),
                        None => Ok(finished),
                    };
                }
            }
            continue;
        }
        let property = parse_line(line)?;
        open.last_mut()
            .expect("a component is open")
            .properties
            .push(property);
    }

    Err(ICalError::Unterminated(
        open.pop().expect("a component is open").name,
    ))
}

/// The component name of a `BEGIN:` line.
fn begins(line: &str) -> Option<String> {
    component_name(line, "BEGIN:")
}

/// The component name of an `END:` line.
fn ends(line: &str) -> Option<String> {
    component_name(line, "END:")
}

fn component_name(line: &str, keyword: &str) -> Option<String> {
    let head = line.get(..keyword.len())?;
    head.eq_ignore_ascii_case(keyword)
        .then(|| line[keyword.len()..].trim().to_ascii_uppercase())
        .filter(|name| !name.is_empty())
}

/// Undo line folding and drop blank lines, yielding logical content lines.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        match raw.strip_prefix([' ', '\t']) {
            // A continuation of the previous logical line — but only if there
            // is one; leading whitespace at the top is just noise.
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

fn parse_line(line: &str) -> Result<Property, ICalError> {
    let colon = find_unquoted(line, ':').ok_or_else(|| ICalError::Malformed(line.to_owned()))?;
    let (head, value) = line.split_at(colon);
    let value = &value[1..];

    let mut tokens = split_unquoted(head, ';').into_iter();
    let name = tokens.next().unwrap_or_default();
    if name.is_empty() {
        return Err(ICalError::Malformed(line.to_owned()));
    }

    let mut params: Vec<(String, Vec<String>)> = Vec::new();
    for token in tokens {
        // RFC 5545 §3.2 has no bare parameter values, unlike vCard 2.1: a
        // token without `=` is a broken line, not a TYPE shorthand.
        let (key, values) = token
            .split_once('=')
            .ok_or_else(|| ICalError::Malformed(line.to_owned()))?;
        params.push((
            key.to_ascii_uppercase(),
            split_unquoted(values, ',')
                .iter()
                .map(|value| unquote_param(value))
                .collect(),
        ));
    }

    Ok(Property {
        name: name.to_ascii_uppercase(),
        params,
        value: value.to_owned(),
    })
}

/// Append a content line, folded to [`FOLD_AT`] octets. Folds land on
/// character boundaries: a continuation that split a UTF-8 sequence would make
/// the whole calendar undecodable.
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

fn unquote_param(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_owned()
}

fn quote_param(value: &str) -> String {
    // RFC 5545 §3.1: a parameter value containing the structural characters
    // has to be quoted. There is no escape inside the quotes, so an embedded
    // quote can only be dropped.
    let value = value.replace('"', "");
    if value.contains([';', ':', ',']) {
        format!("\"{value}\"")
    } else {
        value
    }
}
