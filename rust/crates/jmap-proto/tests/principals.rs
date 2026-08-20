// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for the RFC 9670 `Principal` query filter's convenience constructor.

#![cfg(feature = "principals")]

use jmap_proto::principals::PrincipalQueryFilter;

#[test]
fn principal_query_filter_email_sets_only_that_field() {
    let filter = PrincipalQueryFilter::email("alice@example.com");
    assert_eq!(filter.email.as_deref(), Some("alice@example.com"));
    assert_eq!(filter.name, None);
    assert_eq!(filter.text, None);
}
