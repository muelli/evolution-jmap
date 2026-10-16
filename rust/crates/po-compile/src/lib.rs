// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles a UTF-8-only subset of `.po` into `.mo` data.

use std::collections::BTreeMap;
use std::fmt;

/// Compiles `.po` text into `.mo` bytes sorted by `msgid`.
pub fn compile(po: &str) -> Result<Vec<u8>, Error> {
    emit(&parse(po)?)
}

/// Compilation failures with 1-based line numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Missing empty `msgid` header entry.
    NoHeader,
    /// Header charset is missing or unsupported.
    Charset { declared: String },
    /// Two entries translate the same message.
    Duplicate { msgid: String, line: usize },
    /// Unsupported `.po` construct.
    Unsupported { construct: String, line: usize },
    /// A non-comment line this parser cannot classify.
    Unparsed { line: usize, text: String },
    /// A backslash escape this parser does not accept.
    UnknownEscape { line: usize, escape: char },
    /// A `msgid` whose entry ended before its `msgstr`.
    NoMsgstr { line: usize, msgid: String },
    /// Catalogue size exceeds `.mo` 32-bit offsets.
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

#[derive(Default)]
struct Pending {
    line: usize,
    fuzzy: bool,
    msgid: String,
    msgstr: Option<String>,
}

enum Reading {
    Nothing,
    Msgid,
    Msgstr,
}

fn parse(po: &str) -> Result<BTreeMap<String, String>, Error> {
    let mut entries = BTreeMap::new();
    let mut pending: Option<Pending> = None;
    let mut reading = Reading::Nothing;
    let mut fuzzy = false;

    for (index, raw) in po.lines().enumerate() {
        let line = index + 1;
        let text = raw.trim();

        // Blank and `#~` obsolete lines end the current entry.
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

    // Keep the fuzzy header so `charset=` stays available.
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
        // Trailing backslash means the quoted string never closed.
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
            // Reject escapes this subset does not implement.
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

fn emit(entries: &BTreeMap<String, String>) -> Result<Vec<u8>, Error> {
    let count = u32::try_from(entries.len()).map_err(|_| Error::TooLarge {
        bytes: entries.len(),
    })?;
    // Header, two `(length, offset)` tables, then one string blob.
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
    // `.mo` table lengths exclude the trailing NUL.
    blob.push(0);
    Ok(())
}
