// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The script that puts a placeholder in place of an externally hosted
//! contact photo.
//!
//! Evolution's contact formatter writes the photo as
//! `<img id="__evo-contact-photo" border="1" src="<the URL>">`, with the URL
//! of the vCard copied in as it is. The script finds that element, hides it,
//! and puts a placeholder before it. The placeholder names the URL, so the
//! user can read where the image is before anything asks for it.
//!
//! The script reads the URL out of the document. It never carries the URL in.
//! A contact comes from a server, so its photo URL is text this project does
//! not control, and building a script around such text is how an injection
//! starts. The one value this module does put into the script is the
//! translated label, through [`js_string`].

/// The element id Evolution's contact formatter gives the photo.
///
/// Both of the formatter's render modes use it (`eab-contact-formatter.c`,
/// checked against 3.52.3 and 3.60.2).
pub const PHOTO_ID: &str = "__evo-contact-photo";

/// The class of the placeholder element the script inserts.
pub const PLACEHOLDER_CLASS: &str = "__jmap-external-photo";

/// `text` as a JavaScript string literal, quotes included.
///
/// Single quotes and backslashes get a backslash. Every other character a
/// JavaScript parser treats as the end of a line becomes a `\u` escape:
/// U+2028 and U+2029 end a line in JavaScript but not in Rust, so a label
/// that carries one would otherwise cut the script in half.
pub fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for character in text.chars() {
        match character {
            '\'' | '\\' => {
                out.push('\\');
                out.push(character);
            }
            '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04X}", character as u32));
            }
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The script, with `label` as the text above the URL.
///
/// The script does nothing at all unless the document holds a photo element
/// whose source is an `http` or `https` URL. It leaves three other cases
/// alone:
///
/// - An inline photo. The formatter writes it as a `data:` URL, which this
///   project already has the bytes of.
/// - A local photo. The formatter prefixes a `file://` URL with `evo-`, which
///   Evolution answers itself.
/// - The compact render of a vCard attached to a mail. That render is the
///   only one that gives the photo a `width` attribute, and it is a different
///   context that the operator has not decided about yet.
pub fn placeholder_script(label: &str) -> String {
    format!(
        "(function() {{\n\
         var img = document.getElementById('{PHOTO_ID}');\n\
         if (!img || img.hasAttribute('width')) {{ return; }}\n\
         var src = (img.getAttribute('src') || '').toLowerCase();\n\
         if (src.indexOf('http://') !== 0 && src.indexOf('https://') !== 0) {{ return; }}\n\
         if (document.getElementsByClassName('{PLACEHOLDER_CLASS}').length) {{ return; }}\n\
         var box = document.createElement('div');\n\
         box.className = '{PLACEHOLDER_CLASS}';\n\
         var title = document.createElement('div');\n\
         title.textContent = {label};\n\
         var url = document.createElement('div');\n\
         url.textContent = img.getAttribute('src');\n\
         url.style.wordBreak = 'break-all';\n\
         box.appendChild(title);\n\
         box.appendChild(url);\n\
         img.style.display = 'none';\n\
         img.parentNode.insertBefore(box, img);\n\
         }})();",
        label = js_string(label)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_label_is_quoted_as_it_is() {
        assert_eq!(
            js_string("Externally hosted image"),
            "'Externally hosted image'"
        );
    }

    #[test]
    fn a_quote_and_a_backslash_cannot_end_the_literal() {
        assert_eq!(js_string(r"it's a \ backslash"), r"'it\'s a \\ backslash'");
    }

    #[test]
    fn the_javascript_line_terminators_become_escapes() {
        assert_eq!(js_string("a\u{2028}b\u{2029}c"), "'a\\u2028b\\u2029c'");
        assert_eq!(js_string("a\nb"), "'a\\u000Ab'");
    }

    #[test]
    fn a_translated_label_keeps_its_own_characters() {
        assert_eq!(js_string("Bild von außen"), "'Bild von außen'");
    }

    #[test]
    fn the_script_names_the_photo_and_the_placeholder() {
        let script = placeholder_script("Externally hosted image");
        assert!(script.contains(PHOTO_ID), "the script lost the photo id");
        assert!(
            script.contains(PLACEHOLDER_CLASS),
            "the script lost the placeholder class"
        );
        assert!(
            script.contains("'Externally hosted image'"),
            "the script lost the label"
        );
    }

    /// The script reads the URL from the document, so no URL is ever built
    /// into it. This is the property that keeps a server-chosen photo URL out
    /// of the parsed script text, and the reason the function takes no URL.
    #[test]
    fn the_script_reads_the_url_from_the_document() {
        let script = placeholder_script("Externally hosted image");
        assert!(
            script.contains("img.getAttribute('src')"),
            "the script no longer reads the URL from the document"
        );
    }

    /// A label that tries to close the literal and run its own code stays
    /// one string.
    #[test]
    fn a_hostile_label_stays_one_string() {
        let script = placeholder_script("'; alert(1); var x='");
        assert!(
            script.contains(r"'\'; alert(1); var x=\''"),
            "the label escaped its literal: {script}"
        );
    }
}
