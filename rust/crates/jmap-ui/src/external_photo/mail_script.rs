// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The script that puts a placeholder in place of an externally hosted
//! vCard photo in a mail's compact, attached-contact render.
//!
//! `e-mail-formatter-vcard.c` renders that view inside its own `<iframe>`, a
//! separate document from the mail body. `e_web_view_jsc_run_script_take`
//! always runs its script against the main frame, so this script walks every
//! iframe's `contentDocument` from there, the same way Evolution's own
//! `Evo.foreachIFrameDocument` (`data/webkit/e-web-view.js`) does: the
//! iframes share one security origin with the page that holds them, so
//! `contentDocument` is not refused.
//!
//! `eab-contact-formatter.c`'s `render_compact` always gives the photo a
//! `width` attribute; its `render_title_block` (the standalone address book
//! preview this crate's other placeholder, [`super::script`], already
//! covers) never does. That attribute is how this script tells the two
//! renders apart, the opposite of [`super::script::placeholder_script`]'s own
//! check.

use crate::external_photo::script::{PHOTO_ID, js_string};

/// The class of the placeholder element the script inserts.
pub const PLACEHOLDER_CLASS: &str = "__jmap-mail-vcard-photo";

/// The class of the "Load image" button the script inserts.
pub const LOAD_CLASS: &str = "__jmap-mail-vcard-photo-load";

/// The script, with `label` above the URL and `action` as the text of the
/// button that loads it.
///
/// Walks the main document and every iframe nested under it (the compact
/// vCard render is one level down from the mail body, but this does not
/// assume a fixed depth). Does nothing to a photo this crate's other
/// placeholder already covers: this one only ever acts on a photo element
/// that carries a `width` attribute.
pub fn placeholder_script(label: &str, action: &str) -> String {
    format!(
        "(function() {{\n\
         function tryPhoto(doc) {{\n\
         var img = doc.getElementById('{PHOTO_ID}');\n\
         if (!img || !img.hasAttribute('width')) {{ return; }}\n\
         var src = (img.getAttribute('src') || '').toLowerCase();\n\
         if (src.indexOf('http://') !== 0 && src.indexOf('https://') !== 0) {{ return; }}\n\
         if (doc.getElementsByClassName('{PLACEHOLDER_CLASS}').length) {{ return; }}\n\
         var box = doc.createElement('div');\n\
         box.className = '{PLACEHOLDER_CLASS}';\n\
         var title = doc.createElement('div');\n\
         title.textContent = {label};\n\
         var url = doc.createElement('div');\n\
         url.textContent = img.getAttribute('src');\n\
         url.style.wordBreak = 'break-all';\n\
         var load = doc.createElement('button');\n\
         load.type = 'button';\n\
         load.className = '{LOAD_CLASS}';\n\
         load.setAttribute('value', img.getAttribute('src'));\n\
         load.textContent = {action};\n\
         box.appendChild(title);\n\
         box.appendChild(url);\n\
         box.appendChild(load);\n\
         img.style.display = 'none';\n\
         img.parentNode.insertBefore(box, img);\n\
         }}\n\
         function walk(doc) {{\n\
         if (!doc) {{ return; }}\n\
         tryPhoto(doc);\n\
         var iframes = doc.getElementsByTagName('iframe');\n\
         for (var i = 0; i < iframes.length; i++) {{\n\
         try {{ walk(iframes[i].contentDocument); }} catch (e) {{}}\n\
         }}\n\
         }}\n\
         walk(document);\n\
         }})();",
        label = js_string(label),
        action = js_string(action)
    )
}

/// The script that shows the photo fetched from `url`, with `data_uri` as
/// its new `src`. Walks the same way [`placeholder_script`] does, to find
/// whichever nested document still shows that photo.
pub fn loaded_script(url: &str, data_uri: &str) -> String {
    format!(
        "(function() {{\n\
         function tryLoaded(doc) {{\n\
         var img = doc.getElementById('{PHOTO_ID}');\n\
         if (!img || img.getAttribute('src') !== {url}) {{ return; }}\n\
         img.src = {data_uri};\n\
         img.style.display = '';\n\
         var box = doc.getElementsByClassName('{PLACEHOLDER_CLASS}')[0];\n\
         if (box) {{ box.remove(); }}\n\
         }}\n\
         function walk(doc) {{\n\
         if (!doc) {{ return; }}\n\
         tryLoaded(doc);\n\
         var iframes = doc.getElementsByTagName('iframe');\n\
         for (var i = 0; i < iframes.length; i++) {{\n\
         try {{ walk(iframes[i].contentDocument); }} catch (e) {{}}\n\
         }}\n\
         }}\n\
         walk(document);\n\
         }})();",
        url = js_string(url),
        data_uri = js_string(data_uri)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_names_the_photo_the_placeholder_and_the_button() {
        let script = placeholder_script("Externally hosted image", "Load image");
        assert!(script.contains(PHOTO_ID), "the script lost the photo id");
        assert!(
            script.contains(PLACEHOLDER_CLASS),
            "the script lost the placeholder class"
        );
        assert!(
            script.contains(LOAD_CLASS),
            "the script lost the load button class"
        );
        assert!(
            script.contains("'Externally hosted image'"),
            "the script lost the label"
        );
        assert!(
            script.contains("'Load image'"),
            "the script lost the button text"
        );
    }

    /// The opposite check from the address-book placeholder: this one acts
    /// only on a photo that carries a `width` attribute.
    #[test]
    fn the_script_requires_the_width_attribute() {
        let script = placeholder_script("Externally hosted image", "Load image");
        assert!(
            script.contains("!img.hasAttribute('width')"),
            "the script no longer requires the width attribute: {script}"
        );
    }

    /// The compact render lives in a nested iframe, so the script must
    /// recurse into every iframe it finds, not stop at the main document.
    #[test]
    fn the_script_walks_nested_iframes() {
        let script = placeholder_script("Externally hosted image", "Load image");
        assert!(
            script.contains("iframes[i].contentDocument"),
            "the script does not walk into nested iframes: {script}"
        );
        assert!(
            script.contains("walk(document)"),
            "the script does not start walking from the top document: {script}"
        );
    }

    #[test]
    fn the_script_reads_the_url_from_the_document() {
        let script = placeholder_script("Externally hosted image", "Load image");
        assert!(
            script.contains("img.getAttribute('src')"),
            "the script no longer reads the URL from the document"
        );
    }

    #[test]
    fn a_hostile_label_stays_one_string() {
        let script = placeholder_script("'; alert(1); var x='", "Load image");
        assert!(
            script.contains(r"'\'; alert(1); var x=\''"),
            "the label escaped its literal: {script}"
        );
    }

    #[test]
    fn the_loaded_script_applies_only_to_the_url_it_fetched() {
        let script = loaded_script("https://a.example/p.png", "data:image/png;base64,AAAA");
        assert!(
            script.contains("img.getAttribute('src') !== 'https://a.example/p.png'"),
            "the script does not check the photo URL: {script}"
        );
    }

    #[test]
    fn the_loaded_script_walks_nested_iframes() {
        let script = loaded_script("https://a.example/p.png", "data:image/png;base64,AAAA");
        assert!(
            script.contains("iframes[i].contentDocument"),
            "the loaded script does not walk into nested iframes: {script}"
        );
    }

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

    /// The two placeholders must never collide: a mail that also opens the
    /// standalone address book preview for the same contact must not have
    /// one placeholder's script hide or replace the other's elements. The
    /// address book's load class is a fresh token per render, not a fixed
    /// string (see `script::LOAD_CLASS_PREFIX`), so this checks the one
    /// fixed thing that stays true for every token it could ever produce.
    #[test]
    fn the_classes_differ_from_the_address_book_placeholder() {
        use crate::external_photo::script;
        assert_ne!(PLACEHOLDER_CLASS, script::PLACEHOLDER_CLASS);
        assert!(
            !LOAD_CLASS.starts_with(script::LOAD_CLASS_PREFIX),
            "the mail placeholder's fixed class could collide with an address \
             book placeholder's token class"
        );
    }
}
