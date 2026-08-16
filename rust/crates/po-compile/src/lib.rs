// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A translator's `.po` turned into the `.mo` gettext opens at run time.
//!
//! This is `msgfmt`, restricted to what this project's catalogues contain, and
//! written here rather than shelled out to.
//!
//! ## Why not `msgfmt`
//!
//! Because the build would then need the gettext tools installed, and the CI
//! image does not have them — the extraction step that produces
//! `po/evolution-jmap.pot` is deliberately a developer's command whose *output*
//! is committed, so nothing in CI runs `xgettext`. Compiling is different: it
//! has to happen at build time, once per language, on every machine that builds
//! the project, because a `.mo` is a binary artefact that has no business being
//! in the tree. Making that a build dependency on gettext would either add a
//! package to an image this repository's autonomous sessions must not change,
//! or make catalogues quietly optional — and "quietly optional" is how a
//! translated build ships in English.
//!
//! The format is small and frozen. `jmap-backend-core`'s `tests/catalogue.rs`
//! already writes one by hand for the same reason; this is that code grown a
//! `.po` parser and a set of refusals.
//!
//! ## What it refuses, and why refusing is the point
//!
//! A compiler for a subset has two ways to meet something outside the subset:
//! drop it, or stop. Dropping loses a translation with no diagnostic — the
//! build stays green and a user reads English — so everything here stops:
//! `msgctxt`, plural forms, an escape it does not know, a duplicate message, a
//! line it cannot place, a catalogue that declares a charset other than UTF-8.
//! Every one of those names its line, because a translator's file is edited by
//! someone who is not reading this code.
//!
//! Two things are dropped rather than refused, because dropping is what they
//! *mean*: an entry marked `fuzzy` (a translator's guess, not yet a
//! translation) and an entry with an empty `msgstr` (untranslated). gettext
//! falls back to the English msgid for both, which is the intended result. The
//! header entry is exempt from the fuzzy rule — `msginit` marks it fuzzy in
//! every new catalogue, and dropping it would take the charset declaration with
//! it.

use std::collections::BTreeMap;
use std::fmt;

/// Compiles the text of a `.po` file into the bytes of a `.mo` file.
///
/// The entries come out sorted by msgid, which the format requires of a
/// catalogue with no hash table: gettext binary-searches the table of
/// originals, so an unsorted catalogue is one whose lookups miss without
/// saying so.
pub fn compile(po: &str) -> Result<Vec<u8>, Error> {
    emit(&parse(po)?)
}

/// Why a `.po` could not be compiled.
///
/// Every variant that can name a line does, counting from 1, so the message
/// reads like the compiler diagnostic it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No entry with an empty msgid, so nothing declares the file's charset.
    NoHeader,
    /// The header declares a charset this compiler does not read. Empty when
    /// the header has no `Content-Type` charset at all.
    Charset { declared: String },
    /// Two entries translate the same message.
    Duplicate { msgid: String, line: usize },
    /// A `.po` construct this compiler does not implement.
    Unsupported { construct: String, line: usize },
    /// A line that is not a comment, a keyword, or the continuation of a
    /// string.
    Unparsed { line: usize, text: String },
    /// A backslash escape that is not one of the ones a catalogue uses.
    UnknownEscape { line: usize, escape: char },
    /// A `msgid` whose entry ended before its `msgstr`.
    NoMsgstr { line: usize, msgid: String },
    /// The catalogue is larger than the format's 32-bit offsets can address.
    TooLarge { bytes: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHeader => write!(
                f,
                "the catalogue has no header entry (a msgid \"\"), so nothing \
                 states what charset its translations are in"
            ),
            Self::Charset { declared } if declared.is_empty() => write!(
                f,
                "the header entry declares no charset; it must declare UTF-8"
            ),
            Self::Charset { declared } => write!(
                f,
                "the catalogue is in {declared}; this compiler reads UTF-8 only"
            ),
            Self::Duplicate { msgid, line } => {
                write!(f, "line {line}: {msgid:?} is already translated above")
            }
            Self::Unsupported { construct, line } => write!(
                f,
                "line {line}: {construct} is not implemented by this compiler"
            ),
            Self::Unparsed { line, text } => write!(f, "line {line}: cannot parse {text:?}"),
            Self::UnknownEscape { line, escape } => {
                write!(f, "line {line}: unknown escape \\{escape}")
            }
            Self::NoMsgstr { line, msgid } => {
                write!(f, "line {line}: {msgid:?} has no msgstr")
            }
            Self::TooLarge { bytes } => write!(
                f,
                "the catalogue would be {bytes} bytes, past what a .mo file's \
                 32-bit offsets can address"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// An entry being read, from its `msgid` line until the entry ends.
#[derive(Default)]
struct Pending {
    line: usize,
    fuzzy: bool,
    msgid: String,
    msgstr: Option<String>,
}

/// Which string a bare `"…"` continuation line belongs to.
enum Reading {
    Nothing,
    Msgid,
    Msgstr,
}

/// The entries of `po`, keyed by msgid — which is also the order the `.mo`
/// needs them in, `BTreeMap` sorting by the bytes of the key.
fn parse(po: &str) -> Result<BTreeMap<String, String>, Error> {
    let mut entries = BTreeMap::new();
    let mut pending: Option<Pending> = None;
    let mut reading = Reading::Nothing;
    let mut fuzzy = false;

    for (index, raw) in po.lines().enumerate() {
        let line = index + 1;
        let text = raw.trim();

        // An obsolete entry — one `msgmerge` kept for its translation's sake
        // after the message left the sources. It is commented out, and stays
        // out.
        if text.is_empty() || text.starts_with("#~") {
            take(&mut pending, &mut entries)?;
            reading = Reading::Nothing;
            if text.is_empty() {
                fuzzy = false;
            }
            continue;
        }

        if let Some(flags) = text.strip_prefix("#,") {
            fuzzy = flags.split(',').any(|flag| flag.trim() == "fuzzy");
            continue;
        }
        if text.starts_with('#') {
            continue;
        }

        for construct in ["msgctxt", "msgid_plural", "msgstr["] {
            if text.starts_with(construct) {
                return Err(Error::Unsupported {
                    construct: construct.to_owned(),
                    line,
                });
            }
        }

        if let Some(rest) = text.strip_prefix("msgid") {
            take(&mut pending, &mut entries)?;
            pending = Some(Pending {
                line,
                fuzzy,
                msgid: unquote(rest, line)?,
                msgstr: None,
            });
            fuzzy = false;
            reading = Reading::Msgid;
            continue;
        }

        if let Some(rest) = text.strip_prefix("msgstr") {
            let Some(entry) = pending.as_mut() else {
                return Err(Error::Unparsed {
                    line,
                    text: text.to_owned(),
                });
            };
            entry.msgstr = Some(unquote(rest, line)?);
            reading = Reading::Msgstr;
            continue;
        }

        if text.starts_with('"') {
            let piece = unquote(text, line)?;
            match (&reading, pending.as_mut()) {
                (Reading::Msgid, Some(entry)) => entry.msgid.push_str(&piece),
                (Reading::Msgstr, Some(entry)) => {
                    entry.msgstr.get_or_insert_default().push_str(&piece);
                }
                _ => {
                    return Err(Error::Unparsed {
                        line,
                        text: text.to_owned(),
                    });
                }
            }
            continue;
        }

        return Err(Error::Unparsed {
            line,
            text: text.to_owned(),
        });
    }

    take(&mut pending, &mut entries)?;

    match entries.get("") {
        None => Err(Error::NoHeader),
        Some(header) => {
            let declared = charset(header);
            if declared.eq_ignore_ascii_case("UTF-8") {
                Ok(entries)
            } else {
                Err(Error::Charset { declared })
            }
        }
    }
}

/// Files the entry that has just ended, or drops it if it is not a
/// translation.
fn take(
    pending: &mut Option<Pending>,
    entries: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    let Some(entry) = pending.take() else {
        return Ok(());
    };

    let Some(msgstr) = entry.msgstr else {
        return Err(Error::NoMsgstr {
            line: entry.line,
            msgid: entry.msgid,
        });
    };

    // The header is exempt: `msginit` marks it fuzzy in every new catalogue,
    // and it is where the charset is declared.
    let header = entry.msgid.is_empty();
    if msgstr.is_empty() || (entry.fuzzy && !header) {
        return Ok(());
    }

    if entries.insert(entry.msgid.clone(), msgstr).is_some() {
        return Err(Error::Duplicate {
            msgid: entry.msgid,
            line: entry.line,
        });
    }
    Ok(())
}

/// The charset the header entry declares, or an empty string if it declares
/// none.
///
/// The header is a set of `Name: value` lines; the one that matters here is
/// `Content-Type: text/plain; charset=UTF-8`. Only the value after `charset=`
/// is looked for, because that is the only part of the header this compiler
/// acts on — everything else in it is for translators and their tools.
fn charset(header: &str) -> String {
    let Some(at) = header.find("charset=") else {
        return String::new();
    };
    header[at + "charset=".len()..]
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The contents of a `"…"` string, with its escapes decoded.
///
/// `text` is everything after the keyword, or the whole line for a
/// continuation. Anything outside the quotes is whitespace — a `.po` has no
/// trailing comments — so this insists on that rather than skipping over what
/// it does not recognise.
fn unquote(text: &str, line: usize) -> Result<String, Error> {
    let trimmed = text.trim();
    let unparsed = || Error::Unparsed {
        line,
        text: text.trim().to_owned(),
    };
    if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
        return Err(unparsed());
    }

    let mut decoded = String::with_capacity(trimmed.len());
    let mut characters = trimmed[1..trimmed.len() - 1].chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        // A backslash with nothing after it means the closing quote this
        // function found was itself escaped: the string never ended.
        let escaped = characters.next().ok_or_else(unparsed)?;
        decoded.push(match escaped {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'a' => '\x07',
            'b' => '\x08',
            'f' => '\x0c',
            'v' => '\x0b',
            '"' => '"',
            '\\' => '\\',
            // Octal and hexadecimal escapes are legal in a `.po` and are not
            // implemented, so they arrive here: refused, rather than decoded
            // into the wrong character.
            other => {
                return Err(Error::UnknownEscape {
                    line,
                    escape: other,
                });
            }
        });
    }
    Ok(decoded)
}

/// The entries as the bytes of a `.mo` file.
///
/// Little-endian throughout — the byte order of the magic number is the byte
/// order of every other field, and gettext reads either — and with no hash
/// table, which the format allows and gettext handles by binary-searching the
/// (sorted) table of originals.
fn emit(entries: &BTreeMap<String, String>) -> Result<Vec<u8>, Error> {
    let count = u32::try_from(entries.len()).map_err(|_| Error::TooLarge {
        bytes: entries.len(),
    })?;
    // The header, then the two tables of (length, offset), then the strings.
    let strings_at = 28 + 16 * u64::from(count);

    let mut blob = Vec::new();
    let mut originals = Vec::new();
    let mut translations = Vec::new();
    for original in entries.keys() {
        record(&mut originals, &mut blob, strings_at, original)?;
    }
    for translation in entries.values() {
        record(&mut translations, &mut blob, strings_at, translation)?;
    }

    let mut mo = Vec::with_capacity(blob.len() + originals.len() + translations.len() + 28);
    mo.extend_from_slice(&0x9504_12deu32.to_le_bytes());
    mo.extend_from_slice(&0u32.to_le_bytes()); // format revision
    mo.extend_from_slice(&count.to_le_bytes());
    mo.extend_from_slice(&28u32.to_le_bytes()); // where the originals table is
    mo.extend_from_slice(&(28 + 8 * count).to_le_bytes()); // and the translations
    mo.extend_from_slice(&0u32.to_le_bytes()); // hash table size: none
    mo.extend_from_slice(&0u32.to_le_bytes()); // hash table offset
    mo.extend_from_slice(&originals);
    mo.extend_from_slice(&translations);
    mo.extend_from_slice(&blob);
    Ok(mo)
}

/// One row of a table — where its string is and how long it is — with the
/// string appended to the blob the offsets point into.
fn record(table: &mut Vec<u8>, blob: &mut Vec<u8>, strings_at: u64, s: &str) -> Result<(), Error> {
    let at = strings_at + blob.len() as u64;
    let end = at + s.len() as u64;
    let (Ok(length), Ok(offset)) = (u32::try_from(s.len()), u32::try_from(at)) else {
        return Err(Error::TooLarge {
            bytes: end as usize,
        });
    };
    table.extend_from_slice(&length.to_le_bytes());
    table.extend_from_slice(&offset.to_le_bytes());
    blob.extend_from_slice(s.as_bytes());
    // The recorded length excludes it, but gettext hands the pointer out as a
    // C string all the same.
    blob.push(0);
    Ok(())
}
