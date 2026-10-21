// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// bindgen formats its output with rustfmt only when rustfmt is installed.
// Without rustfmt, the tokens have a space between them. The marker test of
// build.rs must give the same answer for the two forms.

#[path = "../feature_marker.rs"]
mod feature_marker;

use feature_marker::has_marker;

#[test]
fn a_field_marker_matches_formatted_bindings() {
    let bindings = "pub struct _CamelProvider {\n    pub url_hash: GHashFunc,\n}";
    assert!(has_marker(bindings, "pub url_hash: "));
}

#[test]
fn a_field_marker_matches_bindings_that_rustfmt_did_not_format() {
    let bindings = "pub struct _CamelProvider { pub url_hash : GHashFunc , }";
    assert!(has_marker(bindings, "pub url_hash: "));
}

#[test]
fn a_field_marker_does_not_match_a_function_with_the_name_as_a_suffix() {
    let formatted = "pub fn camel_url_hash(u: *const CamelURL) -> guint;";
    let unformatted = "pub fn camel_url_hash (u : * const CamelURL) -> guint ;";
    assert!(!has_marker(formatted, "pub url_hash: "));
    assert!(!has_marker(unformatted, "pub url_hash: "));
}

#[test]
fn an_identifier_marker_matches_both_forms() {
    let formatted = "pub type CamelProviderAutoDetectFunc = ::std::option::Option<()>;";
    let unformatted = "pub type CamelProviderAutoDetectFunc = :: std :: option :: Option < () > ;";
    assert!(has_marker(formatted, "CamelProviderAutoDetectFunc"));
    assert!(has_marker(unformatted, "CamelProviderAutoDetectFunc"));
}
