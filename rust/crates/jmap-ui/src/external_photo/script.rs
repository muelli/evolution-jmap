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
//! The placeholder script reads the URL out of the document. It never
//! carries the URL in. A contact comes from a server, so its photo URL is text
//! this project does not control, and building a script around such text is
//! how an injection starts. The values this module does put into a script go
//! through [`js_string`]: the translated label, and in [`loaded_script`] the
//! fetched URL and its `data:` URI.
//!
//! The button's class is also not fixed text: a vCard's own fields can carry
//! raw HTML, and Evolution's contact formatter has at least one place that
//! pastes a value into the page without escaping it (the photo URI itself).
//! A forged button of the same fixed class would reach the same native click
//! handler as the real one. So each render gets its own unguessable class,
//! chosen on the Rust side and never typed into the page by anything but
//! this script; see [`super::display_ext`] for how the native side matches
//! only that one class.

/// The element id Evolution's contact formatter gives the photo.
///
/// Both of the formatter's render modes use it (`eab-contact-formatter.c`,
/// checked against 3.52.3 and 3.60.2).
pub const PHOTO_ID: &str = "__evo-contact-photo";

/// The class of the placeholder element the script inserts.
pub const PLACEHOLDER_CLASS: &str = "__jmap-external-photo";

/// The id of the "Load image" button the script inserts. Stable across
/// calls of the same render, unlike the button's class: this is how the
/// script finds an existing button again, to give it a fresh class, without
/// reading its old class back first.
const LOAD_ID: &str = "__jmap-external-photo-load-button";

/// The fixed prefix of every "Load image" button's class. [`fresh_load_class`]
/// appends a fresh unguessable token to this for every render; no class this
/// script ever writes is this prefix alone. Public so `mail_script`'s own,
/// unrelated fixed class can assert it never starts with this prefix,
/// however the token part comes out.
pub const LOAD_CLASS_PREFIX: &str = "__jmap-external-photo-load-";

/// A class for one render's "Load image" button: [`LOAD_CLASS_PREFIX`] plus
/// 128 bits of randomness, as lowercase hex.
///
/// [`super::display_ext`] calls this once per `content-loaded`, uses the
/// result as `load_class` in [`placeholder_script`], and registers the same
/// string as the one class its native click handler listens for. A vCard
/// field a forger controls is written long before this call ever runs, so it
/// cannot contain a class this function has not produced yet.
pub fn fresh_load_class() -> String {
    let mut octets = [0u8; 16];
    getrandom::fill(&mut octets).expect("the operating system's random source is unavailable");
    let mut class = String::with_capacity(LOAD_CLASS_PREFIX.len() + octets.len() * 2);
    class.push_str(LOAD_CLASS_PREFIX);
    for byte in octets {
        class.push_str(&format!("{byte:02x}"));
    }
    class
}

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

/// The script, with `label` above the URL and `action` as the text of the
/// button that loads it. `load_class` is the one class the native side
/// listens for on this render; see the module doc for why it changes every
/// time.
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
///
/// The button carries the URL as its `value` attribute, read verbatim from
/// the photo element, and nowhere does the URL enter the script text itself.
/// Evolution reads a clicked element's `value` on the native side
/// (`e_web_view_register_element_clicked`), so the handler never parses the
/// URL out of a string this script built. The native side still checks the
/// class before trusting that value at all: see [`super::display_ext`].
///
/// The button keeps `LOAD_ID`, a fixed id, across renders of the same page,
/// so a second call (one more frame of the same content, idempotent like the
/// rest of this script) finds the existing button and only refreshes its
/// class to the latest `load_class`, instead of reading the class back from
/// the page to decide anything.
pub fn placeholder_script(label: &str, action: &str, load_class: &str) -> String {
    format!(
        "(function() {{\n\
         var img = document.getElementById('{PHOTO_ID}');\n\
         if (!img || img.hasAttribute('width')) {{ return; }}\n\
         var src = (img.getAttribute('src') || '').toLowerCase();\n\
         if (src.indexOf('http://') !== 0 && src.indexOf('https://') !== 0) {{ return; }}\n\
         var load = document.getElementById('{LOAD_ID}');\n\
         if (load) {{ load.className = {load_class}; return; }}\n\
         var box = document.createElement('div');\n\
         box.className = '{PLACEHOLDER_CLASS}';\n\
         var title = document.createElement('div');\n\
         title.textContent = {label};\n\
         var url = document.createElement('div');\n\
         url.textContent = img.getAttribute('src');\n\
         url.style.wordBreak = 'break-all';\n\
         load = document.createElement('button');\n\
         load.type = 'button';\n\
         load.id = '{LOAD_ID}';\n\
         load.className = {load_class};\n\
         load.setAttribute('value', img.getAttribute('src'));\n\
         load.textContent = {action};\n\
         box.appendChild(title);\n\
         box.appendChild(url);\n\
         box.appendChild(load);\n\
         img.style.display = 'none';\n\
         img.parentNode.insertBefore(box, img);\n\
         }})();",
        label = js_string(label),
        action = js_string(action),
        load_class = js_string(load_class)
    )
}

/// The script that shows the photo fetched from `url`, with `data_uri` as
/// its new `src`.
///
/// The web view can load another contact while the fetch runs. So the script
/// does nothing unless the photo element still has `url` as its source. The
/// URL goes in only through [`js_string`], and only for this comparison.
///
/// Idempotent, like [`placeholder_script`]: after the first run the source
/// is the `data:` URI, so a second run does nothing.
pub fn loaded_script(url: &str, data_uri: &str) -> String {
    format!(
        "(function() {{\n\
         var img = document.getElementById('{PHOTO_ID}');\n\
         if (!img || img.getAttribute('src') !== {url}) {{ return; }}\n\
         img.src = {data_uri};\n\
         img.style.display = '';\n\
         var box = document.getElementsByClassName('{PLACEHOLDER_CLASS}')[0];\n\
         if (box) {{ box.remove(); }}\n\
         }})();",
        url = js_string(url),
        data_uri = js_string(data_uri)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_load_classes_start_with_the_prefix_and_never_repeat() {
        let a = fresh_load_class();
        let b = fresh_load_class();
        assert!(a.starts_with(LOAD_CLASS_PREFIX), "{a}");
        assert!(b.starts_with(LOAD_CLASS_PREFIX), "{b}");
        assert_ne!(a, b, "two renders must not share a class");
    }

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
    fn the_script_names_the_photo_the_placeholder_and_the_button() {
        let script = placeholder_script("Externally hosted image", "Load image", "tok-1");
        assert!(script.contains(PHOTO_ID), "the script lost the photo id");
        assert!(
            script.contains(PLACEHOLDER_CLASS),
            "the script lost the placeholder class"
        );
        assert!(
            script.contains(LOAD_ID),
            "the script lost the load button id"
        );
        assert!(script.contains("'tok-1'"), "the script lost the load class");
        assert!(
            script.contains("'Externally hosted image'"),
            "the script lost the label"
        );
        assert!(
            script.contains("'Load image'"),
            "the script lost the button text"
        );
    }

    /// The script reads the URL from the document, so no URL is ever built
    /// into it. This is the property that keeps a server-chosen photo URL out
    /// of the parsed script text, and the reason the function takes no URL.
    #[test]
    fn the_script_reads_the_url_from_the_document() {
        let script = placeholder_script("Externally hosted image", "Load image", "tok-1");
        assert!(
            script.contains("img.getAttribute('src')"),
            "the script no longer reads the URL from the document"
        );
    }

    /// The button carries the URL as its own `value`, read from the photo
    /// element, not typed into the script as a literal.
    #[test]
    fn the_button_carries_the_url_as_its_value_from_the_document() {
        let script = placeholder_script("Externally hosted image", "Load image", "tok-1");
        assert!(
            script.contains("load.setAttribute('value', img.getAttribute('src'))"),
            "the button no longer carries the photo URL as its value"
        );
    }

    /// The button never carries a fixed class: a vCard field a forger
    /// controls could otherwise place a matching element anywhere on the
    /// page and reach the native click handler without ever touching this
    /// script. Each render passes its own class in.
    #[test]
    fn the_load_class_comes_from_the_caller_not_a_constant() {
        let a = placeholder_script("Externally hosted image", "Load image", "token-a");
        let b = placeholder_script("Externally hosted image", "Load image", "token-b");
        assert!(a.contains("'token-a'"), "the script lost its load class");
        assert!(b.contains("'token-b'"), "the script lost its load class");
        assert!(
            !a.contains("token-b") && !b.contains("token-a"),
            "a render's script must not carry another render's class"
        );
    }

    /// A second call for the same page (one more frame, same content) must
    /// find the existing button by its stable id and only refresh its class,
    /// not read the class back from the page to decide what to do.
    #[test]
    fn a_second_call_finds_the_existing_button_by_id_to_reclass_it() {
        let script = placeholder_script("Externally hosted image", "Load image", "tok-2");
        assert!(
            script.contains(&format!("document.getElementById('{LOAD_ID}')")),
            "the script no longer looks the button up by its stable id: {script}"
        );
        assert!(
            script.contains("load.className = 'tok-2'; return;"),
            "a found button is not just reclassed and left alone: {script}"
        );
    }

    /// A label that tries to close the literal and run its own code stays
    /// one string.
    #[test]
    fn a_hostile_label_stays_one_string() {
        let script = placeholder_script("'; alert(1); var x='", "Load image", "tok-1");
        assert!(
            script.contains(r"'\'; alert(1); var x=\''"),
            "the label escaped its literal: {script}"
        );
    }

    /// A load class that tries to close the literal stays one string, the
    /// same discipline as the label and the action, even though this module
    /// only ever receives a token this crate generated itself.
    #[test]
    fn a_hostile_load_class_stays_one_string() {
        let script = placeholder_script("Externally hosted image", "Load image", "'; x='");
        assert!(
            script.contains(r"'\'; x=\''"),
            "the load class escaped its literal: {script}"
        );
    }

    /// The web view can show another contact while the fetch runs. The photo
    /// goes in only if the document still shows the URL that was fetched.
    #[test]
    fn the_loaded_script_applies_only_to_the_url_it_fetched() {
        let script = loaded_script("https://a.example/p.png", "data:image/png;base64,AAAA");
        assert!(
            script.contains("img.getAttribute('src') !== 'https://a.example/p.png'"),
            "the script does not check the photo URL: {script}"
        );
    }

    /// A URL that tries to close the literal and run its own code stays one
    /// string.
    #[test]
    fn a_hostile_url_stays_one_string() {
        let script = loaded_script("https://a.example/'; alert(1); '", "data:,");
        assert!(
            script.contains(r"'https://a.example/\'; alert(1); \''"),
            "the URL escaped its literal: {script}"
        );
    }

    #[test]
    fn the_loaded_script_names_the_photo_and_the_placeholder() {
        let script = loaded_script("https://a.example/p.png", "data:image/png;base64,AAAA");
        assert!(script.contains(PHOTO_ID), "the script lost the photo id");
        assert!(
            script.contains(PLACEHOLDER_CLASS),
            "the script lost the placeholder class"
        );
        assert!(
            script.contains("'data:image/png;base64,AAAA'"),
            "the script lost the data URI"
        );
    }
}
