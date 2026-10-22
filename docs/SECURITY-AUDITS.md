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
| 2026-10-21 | Changes since 2026-10-20, with the deferred delete of a collection. | F40 to F46 |
| 2026-10-22 | Security and logic review: server data, logic, threads and data loss. | M1 to M6 |
| 2026-10-22 | Review of unsafe code, FFI ownership and secrets. | N1 to N8 |

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
| F40 | A pending collection delete waited until the wall clock reached the stored due time. When the clock moved back, the collection stayed hidden after the info bar closed. | Fixed. The first wait sets an end on the monotonic clock, as the timer of the info bar does. `jmap-backend-collection/tests/deferred_delete.rs`, `a_clock_that_moves_back_during_the_wait_does_not_extend_it`. |
| F41 | The push (EventSource) request writes the path of `eventSourceUrl` as it is. A CR or LF in the path adds request lines, and the request has the credentials. | Fixed. The client refuses a URL with a control character or a space before it connects. `jmap-client/src/eventsource.rs`, `parse_url_refuses_a_control_character_or_a_space` and `a_url_with_an_injected_request_line_is_never_dialed`. |
| F42 | The push (EventSource) connection reads the host of `eventSourceUrl` differently from the TLS check. A URL such as `http://localhost?.example.org/` passes as loopback and connects to another name. | Fixed. The authority must be a host name, an IPv4 address or a bracketed IPv6 address, with an optional numeric port. The loopback check then reads the same host that the client dials. `jmap-client/src/eventsource.rs`, `parse_url_refuses_a_host_that_the_loopback_check_reads_differently`. |
| F43 | The id budget of L6 does not count the objects of a `/get` answer. A server can add objects that the client did not ask for, up to the size limit of each response. | Fixed. After each chunk, the client removes each object and each `notFound` id that the request did not name. It keeps one object with no id, so that the caller can report it. Thus only one response at a time can hold extra objects. `jmap-client/tests/get_answer_padding.rs`. |
| F44 | No caller uses the results that a partly applied `/set` returns. The error also hides a 401 or 429 of a later part from the retry logic. No caller sends more than one object today. | Partly fixed. The error of the failed part now decides if the error is a 401 or a rate limit, and gives its retry time. `jmap-client/src/error.rs`, `a_partial_set_reports_the_401_of_its_failed_part` and `a_partial_set_reports_the_rate_limit_of_its_failed_part`. Open: no caller uses the results of the applied parts, so a retry sends those objects again. |
| F45 | A received cancellation and an imported file can make the server send the user's answer to the organizer. The guests get no message. | Open. This needs a product decision. |
| F46 | When the calendar backend goes offline during a received change, EDS can keep the change and upload it later as a change that the user made. | Fixed. The backend marks a received change in its operation flags, not on the thread. EDS keeps these flags with a change that it queues offline and uploads the change with them. `jmap-backend-cal/tests/scheduling.rs`, `a_received_change_stays_received_on_a_thread_with_no_mark` and `the_received_mark_is_one_bit_outside_the_eds_flags`. |

### Logic review, 2026-10-21

| ID | Problem | Resolution |
|---|---|---|
| L2 | A link in a calendar event from the server could name a local file (`file:`). The client wrote that link into the iCalendar component, and Evolution and EDS then read the local file. | Fixed. The client does not write a `file:` link into the component, as it does not read one from the component. The server keeps its link. `jmap-ical/tests/hostile.rs`, `a_file_href_from_the_server_is_not_drawn_as_an_attach_line`. `jmap-cal-sync/tests/save.rs`, `a_server_file_link_is_not_loaded_and_an_edit_leaves_it_on_the_server`. |
| L3 | A calendar component that Evolution received in a mail or imported from a file was saved like a change that the user made. The server could then send meeting messages in the user's name for data from another person. | Fixed. The backend marks each change that comes through `receive_objects`. For such a change, the server schedules only the user's answer to an organizer that is not the user. The change cannot set the organizer. It cannot change the guests or the organizer of a meeting that the user organizes, or delete that meeting. While the backend is offline, it refuses received components. `jmap-cal-sync/tests/received.rs`, `a_received_request_that_names_the_user_as_organizer_is_saved_without_scheduling` and related tests. `jmap-functional/tests/calendar-received.rs`, `received_components_never_make_the_server_invite_anyone_in_the_users_name`. |
| L4 | The session document names the endpoints for requests, downloads, uploads and push. The client sent the credentials to each of them, also without TLS. The setting that refuses a host without TLS covered only the configured host. | Fixed. The client refuses a session document that names an endpoint without TLS. Two exceptions apply: an endpoint on a loopback host, and an endpoint on the configured origin when the account settings permit that origin without TLS. `jmap-client/tests/plaintext_endpoints.rs`, `a_tls_session_that_advertises_plain_http_endpoints_is_refused` and related tests. |
| L5 | The well-known session address can redirect to another origin. The client did not send the credentials there. But when that origin answered with its own session document, the client accepted it and sent the credentials to the endpoints that the document named. | Fixed. The client refuses a session document that comes from an origin other than the configured one. The error names both origins, and the user can set the account's host to the correct server. `jmap-client/tests/redirect_auth.rs`, `a_session_document_from_a_foreign_redirect_target_gets_no_credentials`. |
| L6 | Each server response has a size limit, but a sync can read many pages. The client kept all ids from all pages of a `/changes` stream or of a mailbox listing. A server could make the calendar, address book or mail process use very much memory. | Fixed. One sync keeps at most one million ids and 64 MiB of id text. A server that sends more causes a protocol error for that sync. `jmap-client/tests/changes.rs`, `a_changes_stream_past_the_id_budget_is_refused` and related tests. `jmap-mail-sync/tests/messages.rs`, `a_query_past_the_id_budget_is_refused`. |
| L7 | A shared address book or calendar had the name `<collection> (<owner>)`. The owner sets the collection name, so a shared calendar could look like a collection of the user. | Fixed. The name starts with the owner: `<owner>: <collection>`. Control characters, line separators and bidi formatting characters are removed from both parts. `jmap-collection-sync/src/children.rs`, `a_sharer_cannot_make_a_shared_child_read_as_the_users_own`. |

### Security and logic review, 2026-10-22

| ID | Problem | Resolution |
|---|---|---|
| M1 | The address book and calendar list sent one `/query` and did not read its `limit` or `total`. A server can cap a query (RFC 8620 section 5.5). EDS then removed from its cache each card or event that the partial list did not have. | Fixed. The list gets all pages of the query. It fails when the ids do not agree with the `total` of the server. `jmap-book-sync/tests/sync.rs`, `a_capped_query_still_lists_every_card` and `a_query_that_drops_cards_without_a_limit_fails_the_list`. `jmap-cal-sync/tests/sync.rs`, `a_capped_query_still_lists_every_event` and `a_query_that_drops_events_without_a_limit_fails_the_list`. |
| M2 | A save of a card or an event patched each mapped property that differed between the server copy and the local copy. Thus a local edit of one property reverted a change of another property that a different client made after the last load. | Fixed. The sync crates take the copy that the user edited as a base. With a base, the save patches only the properties that differ between the base and the local copy. A property that the user and a different client both changed follows `OnConflict`: keep the local value, keep the server value, or fail with `OutOfSync` (`E_CLIENT_ERROR_OUT_OF_SYNC`). A base that does not parse gives the old patch and does not block the save. The backends give the base and the conflict resolution of EDS: each contact or event keeps its vCard or iCalendar from the server as its extra in the EDS cache, and EDS gives the extra back on a save and on the replay of an offline edit. A received meeting change has no base, because it replaces the event. `jmap-functional/tests/book-offline-edit.rs` and `calendar-offline-edit.rs`. `jmap-book-sync/tests/save.rs` and `jmap-cal-sync/tests/save.rs`, `an_edit_of_one_property_does_not_revert_a_concurrent_remote_edit_of_another`. `tests/concurrent_edit.rs` in both crates. |
| M3 | Delete only marked the message in Camel. Expunge then destroyed it, and the message never went to the trash of the server. Empty Trash removed nothing, because no message in the trash had the mark. | Fixed. If the account has a trash mailbox, a deleted message outside the trash moves into the trash when the folder synchronizes. A message is destroyed only in the trash, and an expunge of the trash removes every message there. This is the model of IMAPX with a real trash folder. `jmap-mail/tests/expunge.rs`, `a_deleted_message_moves_into_the_trash_on_synchronize` and `expunging_the_trash_removes_every_message_in_it`. |
| M4 | A user deleted a collection, clicked Undo and deleted it again inside the first window. The thread of the first delete still waited, and it destroyed the collection at the first due time. Thus the second Undo window was too short. | Fixed. A new delete of a watched collection restarts the wait of its thread. `jmap-backend-collection/tests/deferred_delete.rs`, `a_delete_again_after_undo_waits_for_its_own_due_time`. |
| M5 | The summary rows and the message cache used the `Email` id as the key. Two servers can give the same id to different messages. After a change of the host of an account, a row kept the subject and the sender of the old message, and the cache served the body of the old message under the id of the new one. | Fixed. The summary header and the cache directory record the origin (the API URL and the account id). A refresh with a different origin removes the rows, the state and the cached messages, and then lists the mailbox. `jmap-mail/tests/server_change.rs`, `a_refresh_from_another_server_replaces_a_row_that_shares_its_uid` and `a_body_cached_from_another_server_is_not_served_for_a_new_message`. |
| M6 | Send Later used the identity with the exact sender address. If no identity had that address, it used the first identity of the account. Send uses the domain wildcard of RFC 8621 section 6 and refuses when no identity covers the sender. | Fixed. Send Later selects the identity as Send does and refuses with the same message, before the upload. `jmap-ui/tests/send_later.rs`, `a_scheduled_send_from_an_address_no_identity_covers_is_refused_before_upload` and `a_scheduled_send_uses_the_domain_wildcard_identity`. |

### Unsafe code and secrets review, 2026-10-22

| ID | Problem | Resolution |
|---|---|---|
| N1 | Code on a worker thread read the strings of an `ESource` through the borrowed getters of EDS. On an account edit, the main loop can free such a string during the copy. This is the defect of F36 in other crates. | Partly fixed. `jmap-backend-core` reads copies through the `dup_` getters. `jmap-backend-core/tests/source.rs`, `from_source_on_another_thread_copies_a_user_the_main_thread_is_replacing` and related tests. Open: `jmap-backend-collection` and `jmap-ui`. |

### FFI soundness audit, 2026-08-19

| ID | Problem | Resolution |
|---|---|---|
| Soundness 1 | Same as F9. | Fixed, see F9. |
| Soundness 2 | Three `jmap-config` vtable functions did not go through the panic guard. | Fixed. They use `trampoline::guard` like all other vtable functions. |
| Soundness 3 | A pointer from `jmap-config/src/oauth2.rs` could be used after a concurrent write freed it. | Fixed. `jmap-config/tests/oauth2.rs`, `a_pointer_handed_out_before_a_changed_write_still_reads_its_original_bytes`. |
| Soundness 4 | `insert_entries` in `jmap-config/src/backend.rs` did not check all extensions for NULL. | Fixed. A missing extension leaves its row unbound. |

## Next audits

Each new re-audit adds its findings to this page.
