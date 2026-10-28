<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Genuinely Equivalent Mutants in `jmap-client`

## `src/blob.rs`

- Zero missed mutants (3 of 3 viable mutants caught by test suites, 1 unviable due to BlobCopyResponse without Default).

## `src/limits.rs`

- Zero missed mutants (14 of 14 generated mutants caught by test suites).

## `src/quota.rs`

- Zero missed mutants (8 of 8 generated mutants caught by test suites).

## `src/resolver.rs`

- Zero missed mutants (1 unviable mutant generated due to SrvTarget without Default).

## `src/snooze.rs`

- Zero missed mutants (3 of 3 generated mutants caught by test suites).

## `src/changes.rs`

- `crates/jmap-client/src/changes.rs:169:35: replace match guard disposition.updated with true in ChangeSet::classify`: in `all_changes`, an entry in `by_id` is created only when the ID appears in `created`, `updated`, or `destroyed`, setting the corresponding boolean flag to true. When `(disposition.created, disposition.destroyed)` is `(false, false)`, `disposition.updated` is guaranteed to be true, so the guard is always true and replacing it with `true` has no observable effect.
- 8 of 8 viable mutants caught by test suites, 4 unviable due to `ChangesResponse` and `ChangeSet` without `Default`, 1 genuinely equivalent.

## `src/sieve.rs`

- Zero missed mutants (9 of 9 viable mutants caught by test suites, 6 unviable due to `SieveScriptValidateResponse` without `Default`, `SetResponse::new` arity, and missing `From`/`FromIterator` implementations).

## `src/principals.rs`

- Zero missed mutants (35 of 35 viable mutants caught by test suites, 3 unviable).

## `src/contacts.rs`

- Zero missed mutants (15 of 15 viable mutants caught by test suites, 15 unviable due to `SetResponse`, `GetResponse`, and `Error` lacking `Default` implementations).

## `src/calendars.rs`

- Zero missed mutants (22 of 22 viable mutants caught by test suites, 22 unviable due to absence of `Default` and parameterless constructors on `SetResponse`, `GetResponse`, `CalendarEventParseResponse`, and `QueryResponse`).

## `src/error.rs`

- Zero missed mutants (12 of 12 viable mutants caught by test suites, 2 unviable due to absence of Default on dyn std::error::Error and Error).

## `src/url.rs`

- `crates/jmap-client/src/url.rs:98:51: replace + with - in origin_of` and `crates/jmap-client/src/url.rs:98:51: replace + with * in origin_of`: in `origin_of`, when `host_port.starts_with('[')`, `host` is extracted as the bracketed IPv6 literal `&host_port[..=end]`. Bracketed IPv6 hosts cannot end with a trailing dot per RFC 3986, so `normalized_host == host` is guaranteed to be true. Under `if normalized_host == host`, `origin_of` returns `Cow::Borrowed(&url[..authority_end])` directly without referencing `port`. Replacing `+` with `-` or `*` in the IPv6 port slice calculation produces no observable difference.
- 42 of 42 viable mutants caught by test suites, 2 genuinely equivalent.

## `src/transport.rs`

- Zero missed mutants (22 of 22 viable mutants caught by test suites, 4 unviable due to absence of Default on CancelScope, absence of Ok().finish() on HttpResponse, and let-chain syntax errors with ||).

## `src/lib.rs`

- Zero mutants generated (module declarations, doctests, and re-exports only).

## `src/client.rs`

- `crates/jmap-client/src/client.rs:304:9: replace Client::builder -> ClientBuilder with Default::default()` (previously line 258): `ClientBuilder` implements `Default`, so calling `ClientBuilder::default()` invokes `<ClientBuilder as Default>::default()`. Replacing `ClientBuilder::default()` with `Default::default()` calls the exact same trait method on the same type, resulting in semantically and binary-identical machine code.
- `crates/jmap-client/src/client.rs:466:31: replace < with <= in Client::sleep_with_cancel`: in `sleep_with_cancel`, `start.elapsed()` is evaluated against `duration`. In each loop iteration, the thread sleeps for `chunk = remaining.min(Duration::from_millis(20))`. After sleeping for at least 1ms to 20ms, the monotonic clock `Instant::now()` has advanced, so on subsequent loop evaluations `start.elapsed() > duration`, which evaluates identically under `<` and `<=`. Even in the theoretical case of evaluation at the exact nanosecond where `start.elapsed() == duration`, the subsequent iteration immediately observes `start.elapsed() > duration`, producing zero observable difference in timing, behavior, or return value.
- 124 of 124 viable mutants caught by test suites, 27 unviable due to absence of `Default` and parameterless constructors on `Credentials`, `Client`, `Session`, `Response`, `HttpResponse`, `SetResponse`, `GetResponse`, `GetRequest`, and `EventSourceSubscription`, 2 genuinely equivalent.

## `src/oauth.rs`

- Zero missed mutants (35 of 35 viable mutants caught by test suites, 9 unviable due to absence of `Default` implementations on `AuthorizationServer`, `ClientRegistration`, `PkceVerifier`, and `TokenResponse`).

## `src/eventsource.rs`

- `crates/jmap-client/src/eventsource.rs:321:26: replace < with <= in sleep_cancellable`: `deadline` is computed as `Instant::now() + duration`. The loop checks `while Instant::now() < deadline` and sleeps for `CANCEL_POLL_INTERVAL.min(duration)`. In `eventsource.rs`, `sleep_cancellable` is invoked only with `backoff`, which is always `>= INITIAL_BACKOFF` (500ms). After sleeping for at least `CANCEL_POLL_INTERVAL` (50ms), the monotonic clock `Instant::now()` has advanced by 50ms, so on the subsequent loop check `Instant::now() > deadline`, which evaluates identically under `<` and `<=`. Even if evaluated at the exact nanosecond where `now == deadline`, the next iteration immediately observes `now > deadline`, resulting in zero observable difference in behavior or return value.
- `crates/jmap-client/src/eventsource.rs:466:9: replace <impl Write for Conn>::flush -> io::Result<()> with Ok(())`: `Conn` implements `std::io::Write` to permit calling `conn.write_all(...)` during HTTP request transmission. `eventsource.rs` never invokes `conn.flush()`. Furthermore, for `Conn::Plain(stream)`, `std::net::TcpStream::flush` in Rust's standard library is an unconditional no-op returning `Ok(())`. Replacing `conn.flush()` with `Ok(())` produces identical behavior.
- 68 of 68 viable mutants caught by test suites, 16 unviable (absence of `Default` implementations on `EventSourceSubscription`, `EventSourceItem`, and `UrlParts`; let-chain syntax errors with `||`; and invalid type arithmetic on `Instant` and `Duration`), 2 genuinely equivalent.

## `src/mail.rs`

- Zero missed mutants (60 of 60 viable mutants caught by test suites, 2 timeouts due to infinite loop on zero `maxObjectsInGet` filter boundary mutations, 17 unviable due to absence of `Default` implementations on `EmailSubmissionSetRequest`, `GetResponse`, `SetResponse`, `QueryResponse`, `GetRequest`, `EmailParseResponse`, `UploadResponse`, and Rust 2024 let-chain syntax errors with `||`).

## `src/set.rs`

- `crates/jmap-client/src/set.rs:300:67: replace > with >= in partition_set_request`: in `partition_set_request`, before packing create components, `sort_creation_components` validates that every strongly connected component satisfies `scc.len() <= limit`. Consequently, when `current_count == 0`, `current_count + scc.len() > limit` simplifies to `scc.len() > limit`, which is guaranteed to be false. The left-hand condition of the `&&` short-circuit expression evaluates to false whenever `current_count == 0`. When `current_count > 0`, both `current_count > 0` and `current_count >= 0` evaluate to `true`. Thus, replacing `>` with `>=` produces identical boolean evaluations across all possible inputs and states, making the mutant genuinely equivalent.
- 69 of 69 viable mutants caught by test suites (68 caught directly, 1 caught via infinite loop timeout on line 436 `delete ! in send_chunks`), 12 unviable (due to absence of `Default` implementations on `Error`, generic `T`, and `SetChunk`, parameterless constructors on `SetResponse`, and let-chain syntax errors with `||`), 1 genuinely equivalent.
