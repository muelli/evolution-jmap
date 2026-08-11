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
use calcard::vcard::{VCardEntry, VCardParameterValue, VCardValue, VCardValueType};
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
    /// The parts of the value, separated by [`Self::separator`] on the wire; a
    /// plain property has one.
    values: Vec<String>,
    /// The value of a line whose bytes are not text, already decoded — see
    /// [`Self::binary`]. Empty for everything this crate writes.
    binary: Option<Vec<u8>>,
    /// The character that separates the values when the line is written: `;`
    /// for a structured value, `,` for a `text-list`. A reader does not need
    /// it — calcard has already split the value by whichever one its property's
    /// kind gives it — but a writer does, and which kind a property is belongs
    /// to the mapping.
    separator: char,
}

/// The separator RFC 2426 §3.1.2 puts between the components of a structured
/// value.
const COMPONENT: char = ';';
/// The separator RFC 2425 §5.8.4 puts between the items of a `text-list`.
const LIST_ITEM: char = ',';

impl Property {
    /// A property with a single text value.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: vec![value.to_owned()],
            binary: None,
            separator: COMPONENT,
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
            binary: None,
            separator: COMPONENT,
        }
    }

    /// A property whose value is a `,`-separated `text-list` (RFC 2425 §5.8.4),
    /// such as `CATEGORIES`.
    ///
    /// The difference from [`Self::structured`] is only which separator goes
    /// between the values: [`escape`] escapes both of them inside a value
    /// either way, so an item holding a comma states one item rather than two.
    pub fn list<I, S>(name: &str, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            separator: LIST_ITEM,
            ..Self::structured(name, items)
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

    /// The value of a `text-list` property (RFC 2425 §5.8.4) as text: the
    /// items rejoined on the comma that separated them.
    ///
    /// calcard reads such a property — `NICKNAME`, `CATEGORIES` — as one value
    /// per list item, splitting on an unescaped comma and never on a
    /// semicolon, so [`Self::text`] would rejoin them on a semicolon and state
    /// something the line never said. Which of the two kinds a property is
    /// belongs to the mapping rather than to this layer, so this is a second
    /// method instead of a rule applied to every value here.
    ///
    /// Rejoining rather than surfacing the items is also what EDS does with
    /// the same line: measured against libebook-contacts 3.52, it hands the
    /// whole value back as one string, honours an escaped comma and an escaped
    /// semicolon, and re-escapes both on the way out.
    pub fn text_list(&self) -> String {
        self.values.join(",")
    }

    /// The value split into its `;`-separated components.
    pub fn components(&self) -> Vec<String> {
        self.values.clone()
    }

    /// The value of a line carrying bytes rather than text — a `PHOTO` under
    /// `ENCODING=b` — with the transfer encoding already undone.
    ///
    /// `None` where the value *is* text, which for such a line means the bytes
    /// happened to be valid UTF-8: calcard decodes the base64 either way and
    /// surfaces bytes only when they are not a string. A picture that is text —
    /// an SVG — therefore arrives through [`Self::text`], and a reader of a line
    /// that may carry either has to take both paths.
    pub fn binary(&self) -> Option<&[u8]> {
        self.binary.as_deref()
    }

    /// The items of a `text-list` property (RFC 2425 §5.8.4), one per
    /// unescaped comma the line held.
    ///
    /// The body is [`Self::components`]', because calcard splits either kind of
    /// value into the same list and the separator that applied is a fact about
    /// the property rather than about the parse. They are two methods because
    /// the mapping should have to say which kind it means — the same reason
    /// [`Self::text_list`] is not [`Self::text`].
    pub fn items(&self) -> Vec<String> {
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
        line.push_str(&escaped.join(&self.separator.to_string()));
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
        binary: entry.values.iter().find_map(|value| match value {
            VCardValue::Binary(data) => Some(data.data.clone()),
            _ => None,
        }),
        // A parsed property is read, never written back out — the mapping
        // builds the vCard it emits from scratch — so this only has to be
        // something: the values are already split.
        separator: COMPONENT,
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
/// Text and dates are surfaced, which between them cover every mapped property
/// whose value is text — UID, FN, N, EMAIL, TEL, `BDAY` and the `X-` lines. A
/// `PHOTO`'s bytes are the one mapped value that is not, and they are carried
/// beside these by [`Property::binary`] rather than turned into a string. A
/// value of any other shape belongs to a property nothing here reads (a `GEO`'s
/// floats), and dropping it loses nothing: the vCard it came from is EDS's copy
/// and stays as it is, and a JSContact property this mapping never mapped is one
/// it never overwrites.
fn value_text(value: &VCardValue) -> Option<String> {
    match value {
        VCardValue::Text(text) => Some(text.clone()),
        // A comma-separated run inside one `;` component, which the mapping
        // reads as the text it was written as.
        VCardValue::Component(items) => Some(items.join(",")),
        // A date line, which calcard has already read into its parts. Written
        // back out as the date text it was, rather than surfaced as parts,
        // because this layer deals in text and the mapping is what decides
        // which dates it can carry.
        VCardValue::PartialDateTime(date) => {
            let mut text = String::new();
            date.format_as_vcard(&mut text, &VCardValueType::DateAndOrTime)
                .ok()?;
            Some(text)
        }
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
