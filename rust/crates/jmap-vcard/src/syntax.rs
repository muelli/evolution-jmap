// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! vCard 3.0 syntax: content lines, folding, escaping, parameters.
//!
//! This layer knows nothing about contacts — it turns the byte-level format
//! ([RFC 2426] §2, inheriting the grammar of [RFC 2425] §5) into a list of
//! [`Property`] values and back. The semantic mapping lives in
//! [`crate::contact`].
//!
//! **Reading is [`calcard`](https://github.com/stalwartlabs/calcard)'s**, the
//! text layer of Stalwart's CalDAV/CardDAV stack: unfolding, unescaping,
//! parameter quoting, the case rules, and the legacy transfer encodings
//! (`QUOTED-PRINTABLE`, `BASE64`) that exporters still emit. That is the side
//! hostile input arrives on, and it is not a liability worth carrying
//! ourselves.
//!
//! **Writing is still ours**, deliberately, because calcard's vCard writer
//! targets 4.0 output and three of its choices are wrong for a 3.0 reader —
//! see [`fold_into`] and [`quote_param`], and `docs/NIGHT-LOG.md` for the
//! measurements. vCard 3.0 rather than 4.0 because that is the only format
//! Evolution's `EVCard` emits (`EVC_FORMAT_VCARD_30`), and every vCard we
//! produce is handed straight to `e_contact_new_from_vcard()`.
//!
//! [RFC 2425]: https://www.rfc-editor.org/rfc/rfc2425
//! [RFC 2426]: https://www.rfc-editor.org/rfc/rfc2426

use calcard::common::IanaString;
use calcard::vcard::{VCardEntry, VCardParameterValue, VCardValue};
use calcard::{Entry, Parser};

use crate::error::VCardError;

/// Fold limit in octets, excluding the line break (RFC 2426 §2.6).
const FOLD_AT: usize = 75;

/// One vCard content line.
///
/// The values are held decoded — unescaped, unfolded, and with any legacy
/// transfer encoding already undone — because that is what the mapping wants;
/// the escaping is applied on the way out, by [`write`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// The `item1` in `item1.TEL:…`, if the line was grouped.
    pub group: Option<String>,
    /// Upper-cased property name.
    pub name: String,
    params: Vec<(String, Vec<String>)>,
    /// The `;`-separated components of the value; a plain property has one.
    values: Vec<String>,
}

impl Property {
    /// A property with a single text value.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: vec![value.to_owned()],
        }
    }

    /// A property whose value is a `;`-separated component list, such as `N`.
    /// Empty trailing components are kept: their position is the meaning.
    pub fn structured<I, S>(name: &str, components: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: components
                .into_iter()
                .map(|component| component.as_ref().to_owned())
                .collect(),
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

    /// The value as text. A structured value reads back as its components
    /// rejoined on `;`, which is how it looked on the wire.
    pub fn text(&self) -> String {
        self.values.join(";")
    }

    /// The value split into its `;`-separated components.
    pub fn components(&self) -> Vec<String> {
        self.values.clone()
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
        let escaped: Vec<String> = self.values.iter().map(|value| escape(value)).collect();
        line.push_str(&escaped.join(";"));
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
///
/// calcard runs in its strict mode: this reads either a whole vCard or
/// nothing. Being liberal about a truncated card would mean handing the
/// mapping half a contact, which the next save would write back over the
/// whole one.
pub fn parse(text: &str) -> Result<Vec<Property>, VCardError> {
    let card = match Parser::new(text).strict().entry() {
        Entry::VCard(card) => card,
        Entry::UnterminatedComponent(_) => return Err(VCardError::Unterminated),
        Entry::InvalidLine(line) => return Err(VCardError::Malformed(line)),
        _ => return Err(VCardError::NotAVCard),
    };

    Ok(card.entries.iter().map(from_entry).collect())
}

fn from_entry(entry: &VCardEntry) -> Property {
    Property {
        group: entry.group.clone(),
        name: entry.name.as_str().to_ascii_uppercase(),
        params: entry
            .params
            .iter()
            .map(|param| {
                (
                    param.name.as_str().to_ascii_uppercase(),
                    vec![param_text(&param.value)],
                )
            })
            .collect(),
        values: entry.values.iter().filter_map(value_text).collect(),
    }
}

/// A parameter value as the mapping reads it. Everything this crate writes is
/// text; the typed forms appear on a parsed card, where the mapping compares
/// them against `TYPE` spellings.
fn param_text(value: &VCardParameterValue) -> String {
    match value {
        VCardParameterValue::Text(text) => text.clone(),
        VCardParameterValue::Integer(number) => number.to_string(),
        VCardParameterValue::Timestamp(stamp) => stamp.to_string(),
        VCardParameterValue::Bool(true) => "TRUE".to_owned(),
        VCardParameterValue::Bool(false) => "FALSE".to_owned(),
        VCardParameterValue::ValueType(kind) => kind.as_str().to_owned(),
        VCardParameterValue::Type(kind) => kind.as_str().to_owned(),
        VCardParameterValue::Calscale(scale) => scale.as_str().to_owned(),
        VCardParameterValue::Level(level) => level.as_str().to_owned(),
        VCardParameterValue::Phonetic(system) => system.as_str().to_owned(),
        // A valueless parameter (`EMAIL;INTERNET:…`, which calcard has already
        // read as `TYPE=INTERNET` where the value names a known type), and the
        // JSCOMPS structure, which this mapping has no use for.
        _ => String::new(),
    }
}

/// A value as the mapping reads it, or `None` for one it has no text for.
///
/// Only the text forms are surfaced: the mapped property set — UID, FN, N,
/// EMAIL, TEL and the two `X-JMAP-*` lines — is text throughout, and a card
/// that also carries a `PHOTO` or a `BDAY` is read for the properties this
/// crate maps rather than re-emitted. Dropping the value of a property nothing
/// reads loses nothing: the vCard it came from is EDS's copy and stays as it
/// is, and a JSContact property this mapping never mapped is one it never
/// overwrites.
fn value_text(value: &VCardValue) -> Option<String> {
    match value {
        VCardValue::Text(text) => Some(text.clone()),
        // A comma-separated run inside one `;` component, which the mapping
        // reads as the text it was written as.
        VCardValue::Component(items) => Some(items.join(",")),
        _ => None,
    }
}

/// Append a content line, folded to [`FOLD_AT`] octets. Folds land on
/// character boundaries: a continuation that split a UTF-8 sequence would
/// make the whole vCard undecodable.
///
/// A CR or an LF in `line` is dropped, and that is a security property rather
/// than tidiness. This is the single point every content line passes through —
/// name, parameters and value alike — and a line break inside any of them does
/// not mangle the property, it *ends* the content line: everything after it is
/// read back as a property of its own. The values are not all ours to trust.
/// [`quote_param`] cannot escape its way out of the problem either, because a
/// quoted parameter value has no escape mechanism at all, so a server that
/// chooses the JSContact map key an `emails` entry is filed under — which
/// reaches `X-JMAP-KEY` verbatim — would otherwise be able to write any vCard
/// property it likes into the user's address book. A caller that means a line
/// break in a value spells it `\n`, which [`escape`] produces and this leaves
/// alone.
///
/// calcard's writer is not used for this. It folds one octet late — the `:`
/// between the parameters and the value is written without being counted, so
/// the first line of every folded property is 76 octets against RFC 2426
/// §2.6's 75 — and it encodes a CR as `\r` and a quote as `\"`, neither of
/// which vCard 3.0 defines: `EVCard` resolves an unknown escape to the
/// character itself, so the `\r` would arrive as a literal `r` and the `"`
/// would close the quoted parameter it was meant to sit inside.
fn fold_into(out: &mut String, line: &str) {
    let mut budget = FOLD_AT;
    let mut used = 0;
    for character in line.chars() {
        if character == '\r' || character == '\n' {
            continue;
        }
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
