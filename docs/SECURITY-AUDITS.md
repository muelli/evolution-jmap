<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Security audits

This page lists the FFI and security audits of this code. It gives one line for
each finding. Each line tells you how the finding was fixed or why it was
accepted. The full audit reports are not in this repository.

## Threat model

All audits use the same threat model:

- The JMAP server is not trusted. The server chooses every string, object key,
  count, id and date that this code receives.
- This code gives the server data to Camel, to `EVCard`, to libical and to
  `evolution-source-registry`. These processes also serve the other accounts
  of the user. A crash or memory error in them is a failure for all accounts.
- The EDS, Camel and Evolution headers describe the real ABI. The struct-layout
  tests in `rust/crates/eds-sys/tests/` check this.

## Audits

Each re-audit examines the code that changed since the previous audit. Each
re-audit also runs the regression test of every earlier finding again.

| Date | Scope | Findings |
|---|---|---|
| 2026-08-08 | First FFI audit: the JSContact and JSCalendar mapping, mail folder sync, struct layouts and GObject glue of the EDS backends. | F1 to F10 |
| 2026-08-10 | The Camel mail provider, the collection backend and the account setup module. | F11 to F15 |
| 2026-08-19 | FFI soundness of every `extern "C"` function: panic guards, ownership transfer, NULL checks and `GCancellable`. | Soundness 1 to 4 |
| 2026-08-19 | Inventory of every `unsafe` site, to make each one smaller and to share repeated code. | No defects. The shared helpers in `jmap-backend-core` came from it. |
| 2026-08-28 | The OAuth 2.0 service of the account setup module. | F16 to F19 |
| 2026-09-01 | Changes since 2026-08-28. | F20, F21 |
| 2026-09-11 | Changes since 2026-09-01, with server-side mail search. | F22 to F26 |
| 2026-09-19 | Changes since 2026-09-11, with the `jmap-ui` module. | F27 |
| 2026-09-28 | Changes since 2026-09-19. | None |
| 2026-10-03 | Changes since 2026-09-28. | None |
| 2026-10-04 | Changes since 2026-10-03. | F28 |
| 2026-10-05 | Changes since 2026-10-04, with principal lookup. | F29 |
| 2026-10-06 | Changes since 2026-10-05. | F30 |
| 2026-10-08 | Changes since 2026-10-06. | None |
| 2026-10-10 | Changes since 2026-10-08, with the SRV resolver thread. | F31 |
| 2026-10-13 | Changes since 2026-10-10. | F31 fixed |
| 2026-10-15 | Changes since 2026-10-13. | F32 |
| 2026-10-17 | Changes since 2026-10-15. | F33, F32 fixed |
| 2026-10-20 | Changes since 2026-10-17, with meeting guests and shared collections. | F34 to F39 |
| 2026-10-21 | Logic review with a hostile server or a hostile sender of mail. | L1 to L7 |

## Findings

A test name below is the regression test that keeps the finding fixed. All
paths are relative to `rust/crates/`.

| ID | Problem | Resolution |
|---|---|---|
| F1 | A CR, LF or quote in a JSContact map key from the server added properties to the vCard. | Fixed. `jmap-vcard/tests/hostile.rs`, `a_crlf_in_a_map_key_cannot_add_a_property_to_the_card` and related tests. |
| F2 | A CR or LF in a JSCalendar `duration`, `frequency` or `timeZone` from the server added properties to the iCalendar text. | Fixed. `jmap-ical/tests/hostile.rs`, `a_crlf_in_the_duration_cannot_add_a_property` and related tests. |
| F3 | A long mailbox `parentId` chain from the server overflowed the stack. | Fixed with a depth limit of 64. `jmap-mail-sync/tests/hostile.rs`, `a_chain_past_the_limit_is_cut_rather_than_dropped_or_refused`. |
| F4 | Deeply nested iCalendar components overflowed the stack. | Fixed with a depth limit of 32. `jmap-ical/tests/hostile.rs`, `a_document_nested_past_the_limit_is_refused_rather_than_parsed`. |
| F5 | The backend factory and `EContact` struct layouts were not checked against the GType system. | No defect. `eds-sys/tests/layout.rs`, `backend_factory_layouts_match_the_gtype_system`. |
| F6 | A failure after the sync tag was written could leak that tag. The path could not be reached. | Fixed. The book and calendar backends write all out-parameters in one step after every allocation (`ops.rs`). |
| F7 | Nothing checked that the connection a backend instance holds can be shared across threads. | Fixed. `jmap-backend-book/tests/hostile.rs` and `jmap-backend-cal/tests/hostile.rs`, `the_connection_an_instance_holds_is_shareable_across_threads`. See also F20. |
| F8 | Nothing checked that `camel_provider_register` does not write into the provider struct. | No defect. `eds-sys/tests/camel.rs`, `registering_a_provider_does_not_write_back_into_the_struct`. |
| F9 | `set_raw_gerror` checked for an already set `GError` only in debug builds. | Fixed. It keeps the first error, frees the second and logs a critical. `jmap-backend-core/tests/error.rs`, `overwriting_an_already_set_gerror_keeps_the_first_and_frees_the_second`. |
| F10 | A save of recurrence instances without the master instance could lose data. | Accepted. The calendar backend refuses such a save, and `jmap-ical` now maps `recurrenceOverrides`. |
| F11 | A year from the server overflowed the date arithmetic in mail sync. | Fixed. `jmap-mail-sync/src/date.rs`, `a_year_outside_the_four_digit_grammar_is_not_a_date`. |
| F12 | The checks for F5 and F8, and the loopback address check, did not reach the main branch. | Fixed. The checks were restored. `jmap-backend-core/src/source.rs`, `only_loopback_addresses_count_as_local`. |
| F13 | Nothing checked that the mail provider's instance state can be shared across threads. | Fixed. `jmap-mail/tests/hostile.rs`, `everything_an_instance_holds_is_shareable_across_threads`. |
| F14 | Ids from the server went into URL templates without percent-encoding. | Fixed. `jmap-client/src/url.rs`, `encode_template_value_never_panics_on_hostile_input` and related tests. |
| F15 | A limit from a dependency, not from this project, set the maximum response size. | Fixed. The client sets its own limit. `jmap-client/tests/response_size.rs`. |
| F16 | The OAuth 2.0 service ran the EDS default `prepare_*` functions a second time. | Fixed. `jmap-config/tests/oauth2_service.rs`, `the_authentication_query_slot_adds_only_this_services_own_keys` and `the_token_form_slots_add_only_this_services_own_keys`. |
| F17 | A PKCE challenge was sent without a stored verifier, and the failure was logged only at debug level. | Fixed. `jmap-config/tests/oauth2_service.rs`, `a_missing_pkce_verifier_at_redemption_is_logged_at_warn_level`. |
| F18 | PKCE verifiers stayed in memory for the life of the process and were not wiped. | Fixed. Verifiers are wiped when they are replaced, redeemed or older than 30 minutes. `jmap-config/src/oauth2_service.rs`, `a_verifier_is_redeemed_once_as_a_wiping_secret` and related tests. |
| F19 | A failed OAuth 2.0 token fetch in the mail provider was not traced. | Fixed. `jmap-mail/src/oauth2.rs`, `a_missing_consent_is_traced_as_escalating` and related tests. |
| F20 | `Slot<T>` was `Sync` for a `T` that must stay on its own thread. | Fixed. A `compile_fail` doctest in `jmap-backend-core/src/instance.rs` checks it. |
| F21 | A normal credentials lookup result was logged as a critical fault. | Fixed. `jmap-backend-collection/src/create_resource.rs`, `eds_having_no_password_for_an_account_is_not_a_fault` and `a_cancelled_lookup_is_not_a_fault`. |
| F22 | The Camel search expression parser had no recursion limit. | Fixed. `jmap-mail/src/search_sexp.rs`, `a_run_of_open_parens_is_refused_rather_than_overflowing_the_stack`. |
| F23 | Server search results were filtered in quadratic time. | Fixed. `jmap-mail/src/folder.rs`, `narrowing_a_folder_sized_match_set_does_not_take_seconds`. |
| F24 | The F13 test did not name every `Slot` type of the mail provider. | Fixed. The F13 test covers every `Slot` type of the mail provider. |
| F25 | The snooze handler for the message window had the wrong signal signature. | Fixed. `jmap-ui/tests/signals.rs`, `a_uint_carrying_signal_puts_its_argument_before_the_user_data`. |
| F26 | The unsafe-count check in `ci/checks.sh` failed for days, and no one saw it. | Fixed. The baseline was corrected, and CI runs the full `ci/checks.sh`. |
| F27 | The newer-EDS CI job did not build `jmap-ui`. | Fixed. `ci/eds-matrix.sh` builds `jmap-ui`. |
| F28 | The live transport test client leaked a `CamelService`. Only a test program was affected. | Fixed in `tests/functional/transport-live-client.c`. |
| F29 | The principal lookup could accept a decoy principal that was not verified. | Fixed. `jmap-client/tests/principals.rs`, `current_user_principal_does_not_accept_an_unverified_decoy_candidate`. |
| F30 | `CalendarEvent/parse` and `ContactCard/parse` dropped every item after the first. | Fixed. A result with more than one item is reported as not parsable. `jmap-client/tests/stalwart_tolerances.rs`, `finding_13_calendar_event_parse_reports_multi_event_blob_as_not_parsable`. |
| F31 | One slow SRV lookup blocked the SRV lookups of all other accounts. | Fixed with a pool of resolver threads. `jmap-backend-core/src/resolver_thread.rs`, `a_slow_job_on_one_worker_does_not_delay_a_job_on_another`. |
| F32 | A nested log capture in the test helper lost the outer capture. Only tests were affected. | Fixed. `jmap-tracing-capture/tests/isolation.rs`, `a_nested_capture_hands_the_scope_back_to_the_outer_one`. |
| F33 | Unsigned JSContact integers from the server wrapped instead of being rejected. | Fixed. `jmap-proto/tests/contacts.rs`, `an_out_of_range_unsigned_int_is_rejected_not_wrapped`. |
| F34 | A `"` in an iCalendar parameter, such as the name of a meeting guest, was written with a backslash. libical kept the backslash, and the next save sent a damaged name to the server. | Fixed. Parameter values use the RFC 6868 caret escapes in both directions. `jmap-ical/tests/event.rs`, `a_quote_in_a_guest_name_is_written_with_the_rfc_6868_caret_escape` and `a_caret_escape_in_a_guest_name_reads_back_as_the_character_it_stands_for`. |
| F35 | The client followed a redirect from HTTPS to plain HTTP. A session document from the plain connection could then send the credentials to a different host. | Fixed. The client refuses a redirect that leaves TLS. `jmap-client/src/url.rs`, `a_redirect_from_https_to_plain_http_is_a_downgrade`. |
| F36 | The account setup lookup read the collection source on a worker thread through borrowed strings. The main thread could free a string during the copy. | Fixed. The lookup holds a reference to the source and reads copies. `jmap-config/tests/account.rs`, `a_read_on_another_thread_copies_strings_the_main_thread_is_replacing`. |
| F37 | A `"` in a vCard parameter value from the server, such as a JSContact map key, is written with a backslash. EDS then drops the complete property. | Open. The fix is the same as F34. |
| F38 | The push (EventSource) reader reads a line with no length limit. A server can use all memory with one long line. | Fixed. The reader refuses a line longer than 1 MiB and connects again. `jmap-client/src/eventsource.rs`, `an_endless_line_ends_the_connection_and_the_next_one_delivers` and `stream_events_refuses_a_data_line_longer_than_the_cap`. |
| F39 | When a `/set` request is split into parts, a creation id is replaced by a text match, and the result of a part is lost when a later part fails. No caller sends more than one object today. | Fixed. Each part sends the earlier creation ids as `createdIds`, and the server resolves the references. The client does not change the objects. A failed later part keeps the results of the earlier parts. `jmap-client/src/set.rs`, `a_patch_key_that_starts_with_a_shorter_creation_id_is_sent_unchanged` and `a_failing_later_chunk_keeps_the_results_of_the_applied_chunks`. |

### Logic review, 2026-10-21

| ID | Problem | Resolution |
|---|---|---|
| L3 | A calendar component that Evolution received in a mail or imported from a file was saved like a change that the user made. The server could then send meeting messages in the user's name for data from another person. | Fixed. The backend marks each change that comes through `receive_objects`. For such a change, the server schedules only the user's answer to an organizer that is not the user. The change cannot set the organizer. It cannot change the guests or the organizer of a meeting that the user organizes, or delete that meeting. While the backend is offline, it refuses received components. `jmap-cal-sync/tests/received.rs`, `a_received_request_that_names_the_user_as_organizer_is_saved_without_scheduling` and related tests. `jmap-functional/tests/calendar-received.rs`, `received_components_never_make_the_server_invite_anyone_in_the_users_name`. |

### FFI soundness audit, 2026-08-19

| ID | Problem | Resolution |
|---|---|---|
| Soundness 1 | Same as F9. | Fixed, see F9. |
| Soundness 2 | Three `jmap-config` vtable functions did not go through the panic guard. | Fixed. They use `trampoline::guard` like all other vtable functions. |
| Soundness 3 | A pointer from `jmap-config/src/oauth2.rs` could be used after a concurrent write freed it. | Fixed. `jmap-config/tests/oauth2.rs`, `a_pointer_handed_out_before_a_changed_write_still_reads_its_original_bytes`. |
| Soundness 4 | `insert_entries` in `jmap-config/src/backend.rs` did not check all extensions for NULL. | Fixed. A missing extension leaves its row unbound. |

## Next audits

Each new re-audit adds its findings to this page.
