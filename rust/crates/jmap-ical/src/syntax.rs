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
//! **Content lines are [`calcard`](https://github.com/stalwartlabs/calcard)'s
//! to read**, the text layer of Stalwart's CalDAV/CardDAV stack: unfolding,
//! unescaping, parameter quoting, the case rules, and the typed values —
//! `DTSTART` is parsed as a date-time and `RRULE` as a rule rather than left as
//! text to be picked apart later. That is the side hostile input arrives on,
//! and it is not a liability worth carrying ourselves.
//!
//! **Structure is still checked here**, by `check_structure`, and
//! **writing is still ours**, by [`Component::to_ics`] — see `fold_into` and
//! `quote_param` for why.
//!
//! [RFC 5545]: https://www.rfc-editor.org/rfc/rfc5545

use calcard::common::{IanaString, PartialDateTime};
use calcard::icalendar::{
    ICalendarComponent, ICalendarEntry, ICalendarParameterValue, ICalendarValue,
    ICalendarValueType, Uri,
};
use calcard::{Entry, Parser};

use crate::error::ICalError;

/// Fold limit in octets, excluding the line break (RFC 5545 §3.1).
const FOLD_AT: usize = 75;

/// How deeply components may nest, `VCALENDAR` counted.
///
/// calcard's own tree is flat — a `Vec` of components addressed by index — so
/// the *parse* is not what needs a limit; the tree built from it is what does. A
/// [`Component`] owns a `Vec<Component>`,
/// so the drop glue recurses once per level, and so does
/// `Component::write_into`. A document nested a hundred thousand
/// deep therefore aborts the process — "thread has overflowed its stack" — on a
/// path with no `unsafe` in it at all, and in the calendar factory that takes
/// every other calendar the user has down with it. Refusing the document is the
/// only answer that stays inside the error type.
///
/// The number is far above what the format uses: RFC 5545's deepest nesting is
/// `VCALENDAR` > `VTIMEZONE` > `STANDARD`, or `VCALENDAR` > `VEVENT` >
/// `VALARM`, which is three.
pub const MAX_DEPTH: usize = 32;

/// One iCalendar content line.
///
/// The values are held decoded — unescaped and unfolded — because that is what
/// the mapping wants; the escaping is applied on the way out, by
/// [`Component::to_ics`], and only to the TEXT-typed ones. A `,`-separated
/// value list such as `CATEGORIES` keeps its parts separate, so that a comma
/// *inside* a value cannot be mistaken for the separator: use
/// [`Property::text`], [`Property::texts`] or [`Property::raw_value`] to read
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Upper-cased property name.
    pub name: String,
    params: Vec<(String, Vec<String>)>,
    /// The `,`-separated values of the property; a plain property has one.
    values: Vec<String>,
    /// Whether the values are TEXT, and so have to be escaped when written.
    /// A parsed property is TEXT exactly when calcard read it as text.
    text: bool,
}

impl Property {
    /// A property with a single TEXT value, escaped on the way out.
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: vec![value.to_owned()],
            text: true,
        }
    }

    /// A property holding a `,`-separated list of TEXT values — `CATEGORIES`,
    /// `RESOURCES` — each escaped on the way out, so a comma inside one value
    /// cannot be read back as the separator between two.
    pub fn list<I, S>(name: &str, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: values
                .into_iter()
                .map(|value| value.as_ref().to_owned())
                .collect(),
            text: true,
        }
    }

    /// A property whose value is not TEXT — `DTSTART`, `DURATION`, `RRULE`,
    /// `TRIGGER` — kept verbatim, because its separators are structure.
    pub fn raw(name: &str, value: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            values: vec![value.to_owned()],
            text: false,
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

    /// The value as text, with escapes resolved. A value list reads back as its
    /// parts rejoined on `,`, which is how it looked on the wire.
    pub fn text(&self) -> String {
        self.values.join(",")
    }

    /// The value split into its `,`-separated parts: a TEXT list such as
    /// `CATEGORIES`.
    pub fn texts(&self) -> Vec<String> {
        self.values.clone()
    }

    /// The value in its iCalendar spelling, escapes resolved.
    ///
    /// For the typed values — `DTSTART`, `DURATION`, `RRULE` — that is the
    /// on-the-wire form, since none of them contains anything escapable; the
    /// mapping reads those as text. A parsed one is *re-rendered* from what
    /// calcard understood rather than sliced out of the input, so an `RRULE`'s
    /// parts may come back in a different order than they were written in.
    pub fn raw_value(&self) -> String {
        self.values.join(",")
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
        let values: Vec<String> = match self.text {
            true => self.values.iter().map(|value| escape(value)).collect(),
            false => self.values.clone(),
        };
        line.push_str(&values.join(","));
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
///
/// A content line calcard cannot read is *skipped*, not fatal: that is the
/// policy the semantic mapping already states for a value it cannot map, and a
/// calendar that refuses to open over one bad line loses every event in it.
/// What is still fatal is a document that is not a calendar, one that is
/// truncated, and one nested deeper than [`MAX_DEPTH`].
pub fn parse(text: &str) -> Result<Component, ICalError> {
    check_structure(text)?;

    let mut parser = Parser::new(text);
    let calendar = match parser.entry() {
        Entry::ICalendar(calendar) => calendar,
        _ => return Err(ICalError::NotACalendar),
    };
    // A second object in the same string is not something this layer is allowed
    // to drop silently: it would lose whole events.
    match parser.entry() {
        Entry::Eof => {}
        Entry::InvalidLine(line) => return Err(ICalError::Trailing(line)),
        _ => return Err(ICalError::Trailing("BEGIN:VCALENDAR".to_owned())),
    }

    let components = &calendar.components;
    let root = components.first().ok_or(ICalError::NotACalendar)?;
    if !root
        .component_type
        .as_str()
        .eq_ignore_ascii_case("VCALENDAR")
    {
        return Err(ICalError::NotACalendar);
    }
    check_depth(components)?;

    Ok(from_component(components, 0))
}

/// Whether the document opens and closes its components properly.
///
/// This check is ours rather than calcard's for a reason worth writing down:
/// calcard 0.3.9's lenient mode — the only mode that works — reads a truncated
/// document as a whole one, and its strict mode cannot be used at all, because
/// it returns `InvalidLine("BEGIN")` for every nested component and so rejects
/// every real calendar. Handing the mapping a truncated event would mean the
/// next save writing the fragment back over the whole one, so the `BEGIN`/`END`
/// pairing is checked here before calcard is asked for the content.
///
/// Only the two keywords are looked at, and only up to the first `:`; a
/// parameter on a `BEGIN` line is not legal iCalendar, and one carrying a
/// quoted `:` yields a component name that matches no `END` — refusing the
/// document, which is the safe direction.
fn check_structure(text: &str) -> Result<(), ICalError> {
    let mut open: Vec<String> = Vec::new();
    for line in unfold(text.strip_prefix('\u{feff}').unwrap_or(text)) {
        let Some((keyword, name)) = line.split_once(':') else {
            continue;
        };
        // Parameters are not legal on BEGIN/END, but a line that carries one
        // must not be read as the bare keyword either.
        let name = name.trim().to_ascii_uppercase();
        if name.is_empty() {
            continue;
        }
        if keyword.eq_ignore_ascii_case("BEGIN") {
            open.push(name);
        } else if keyword.eq_ignore_ascii_case("END") {
            match open.pop() {
                Some(expected) if expected == name => {}
                // An END that closes nothing is a stray line, which calcard
                // skips; one that closes the wrong component is a document
                // whose structure cannot be trusted.
                Some(expected) => {
                    return Err(ICalError::Mismatched {
                        expected,
                        found: name,
                    });
                }
                None => {}
            }
        }
    }
    match open.pop() {
        Some(name) => Err(ICalError::Unterminated(name)),
        None => Ok(()),
    }
}

/// Refuse a document nested past [`MAX_DEPTH`].
///
/// calcard's own tree is flat — a `Vec` of components addressed by index — so
/// the parse survives any depth; it is [`from_component`]'s recursion, and the
/// drop glue of the [`Component`] tree it builds, that would run off the end of
/// the stack. So the depth is measured here, iteratively, before anything
/// recurses over it.
fn check_depth(components: &[ICalendarComponent]) -> Result<(), ICalError> {
    let mut pending = vec![(0usize, 1usize)];
    while let Some((index, depth)) = pending.pop() {
        let Some(component) = components.get(index) else {
            continue;
        };
        if depth > MAX_DEPTH {
            return Err(ICalError::TooDeep(
                component.component_type.as_str().to_ascii_uppercase(),
            ));
        }
        for child in &component.component_ids {
            pending.push((*child as usize, depth + 1));
        }
    }
    Ok(())
}

/// Build our component tree from calcard's flat one. Bounded by
/// [`check_depth`], which has already run.
fn from_component(components: &[ICalendarComponent], index: usize) -> Component {
    let component = &components[index];
    Component {
        name: component.component_type.as_str().to_ascii_uppercase(),
        properties: component.entries.iter().map(from_entry).collect(),
        children: component
            .component_ids
            .iter()
            .filter(|child| (**child as usize) < components.len())
            .map(|child| from_component(components, *child as usize))
            .collect(),
    }
}

fn from_entry(entry: &ICalendarEntry) -> Property {
    let values: Vec<(String, bool)> = entry.values.iter().filter_map(value_text).collect();
    Property {
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
        // A property is TEXT only if every value calcard produced was; the
        // typed ones are rendered in their iCalendar spelling, where a `;` or a
        // `,` is structure and escaping it would corrupt the value.
        text: !values.is_empty() && values.iter().all(|(_, text)| *text),
        values: values.into_iter().map(|(value, _)| value).collect(),
    }
}

/// A value as the mapping reads it, and whether it is TEXT, or `None` for one
/// there is no text for.
///
/// The mapped property set — UID, SUMMARY, DESCRIPTION, DTSTART, DURATION,
/// STATUS and RRULE — is covered; a value calcard read as binary (an inline
/// `ATTACH`) has no text spelling short of re-encoding it, and dropping it
/// loses nothing, because the iCalendar it came from is EDS's copy and stays as
/// it is.
fn value_text(value: &ICalendarValue) -> Option<(String, bool)> {
    let typed = |value: String| Some((value, false));
    match value {
        ICalendarValue::Text(text) => Some((text.clone(), true)),
        ICalendarValue::PartialDateTime(stamp) => typed(date_time_text(stamp)),
        ICalendarValue::Duration(duration) => typed(duration.to_string()),
        ICalendarValue::RecurrenceRule(rule) => typed(rule.to_string()),
        ICalendarValue::Period(period) => typed(period.to_string()),
        ICalendarValue::Uri(Uri::Location(uri)) => typed(uri.clone()),
        ICalendarValue::Integer(number) => typed(number.to_string()),
        ICalendarValue::Float(number) => typed(number.to_string()),
        ICalendarValue::Boolean(true) => typed("TRUE".to_owned()),
        ICalendarValue::Boolean(false) => typed("FALSE".to_owned()),
        ICalendarValue::CalendarScale(scale) => typed(scale.as_str().to_owned()),
        ICalendarValue::Method(method) => typed(method.as_str().to_owned()),
        ICalendarValue::Classification(class) => typed(class.as_str().to_owned()),
        ICalendarValue::Status(status) => typed(status.as_str().to_owned()),
        ICalendarValue::Transparency(transparency) => typed(transparency.as_str().to_owned()),
        ICalendarValue::Action(action) => typed(action.as_str().to_owned()),
        ICalendarValue::BusyType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::ParticipantType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::ResourceType(kind) => typed(kind.as_str().to_owned()),
        ICalendarValue::Proximity(proximity) => typed(proximity.as_str().to_owned()),
        // An inline binary value, and a URI calcard read as a data: payload.
        ICalendarValue::Binary(_) | ICalendarValue::Uri(Uri::Data(_)) => None,
    }
}

/// A date-time in its iCalendar spelling: `20260115T130000`, with a `Z` when it
/// was written as a UTC instant, and no time at all for a `VALUE=DATE`.
fn date_time_text(stamp: &PartialDateTime) -> String {
    let kind = match stamp.hour.is_some() {
        true => ICalendarValueType::DateTime,
        false => ICalendarValueType::Date,
    };
    let mut out = String::new();
    // Writing into a String cannot fail.
    let _ = stamp.format_as_ical(&mut out, &kind);
    out
}

/// A parameter value as the mapping reads it. Everything this crate writes is
/// text; the typed forms appear on a parsed object, where the mapping compares
/// them against known spellings.
fn param_text(value: &ICalendarParameterValue) -> String {
    match value {
        ICalendarParameterValue::Text(text) => text.clone(),
        ICalendarParameterValue::Integer(number) => number.to_string(),
        ICalendarParameterValue::Bool(true) => "TRUE".to_owned(),
        ICalendarParameterValue::Bool(false) => "FALSE".to_owned(),
        ICalendarParameterValue::Uri(Uri::Location(uri)) => uri.clone(),
        ICalendarParameterValue::Cutype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Fbtype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Partstat(status) => status.as_str().to_owned(),
        ICalendarParameterValue::Related(related) => related.as_str().to_owned(),
        ICalendarParameterValue::Reltype(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Role(role) => role.as_str().to_owned(),
        ICalendarParameterValue::ScheduleAgent(agent) => agent.as_str().to_owned(),
        ICalendarParameterValue::ScheduleForceSend(send) => send.as_str().to_owned(),
        ICalendarParameterValue::Value(kind) => kind.as_str().to_owned(),
        ICalendarParameterValue::Display(display) => display.as_str().to_owned(),
        ICalendarParameterValue::Feature(feature) => feature.as_str().to_owned(),
        ICalendarParameterValue::Duration(duration) => duration.to_string(),
        ICalendarParameterValue::Linkrel(relation) => relation.as_str().to_owned(),
        // A binary payload, and a parameter written without a value.
        ICalendarParameterValue::Uri(Uri::Data(_)) | ICalendarParameterValue::Null => String::new(),
    }
}

/// Undo line folding and drop blank lines, yielding logical content lines.
///
/// This exists for [`check_structure`] alone — the content of a line is
/// calcard's business, and it unfolds again on its own. A `BEGIN` or `END`
/// keyword is not something anyone folds, but the check has to see every
/// *other* line as one line, or a folded value's continuation could be read as
/// a keyword of its own.
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

/// Append a content line, folded to [`FOLD_AT`] octets. Folds land on
/// character boundaries: a continuation that split a UTF-8 sequence would make
/// the whole calendar undecodable.
///
/// A CR or an LF in `line` is dropped, and that is a security property rather
/// than tidiness. This is the single point every content line passes through —
/// name, parameters and value alike — and a line break inside any of them does
/// not mangle the property, it *ends* the content line: everything after it is
/// read back by libical as a property of its own. The values are not all ours
/// to trust, and the two shapes that skip [`escape`] are exactly the ones a
/// server fills in: [`Property::raw`] keeps `DURATION` and an `RRULE`'s
/// `FREQ` verbatim, and a quoted parameter value — `DTSTART;TZID=` — has no
/// escape mechanism to sanitise with. Without this a server could write any
/// iCalendar property it liked into the event Evolution stores. A caller that
/// means a line break in a TEXT value spells it `\n`, which [`escape`]
/// produces and this leaves alone.
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
