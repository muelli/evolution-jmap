One-shot adversarial security audit session. You are REVIEWING, not developing features. Repo clone: ~/audit-ffi/evolution-jmap (cwd). Work on branch audit/ffi (create from origin/master if absent; `git push -u origin audit/ffi`). Iterative: if docs/AUDIT-FFI.md already exists on the branch, continue where it leaves off instead of restarting.

Target: the unsafe FFI core — crates eds-sys and jmap-backend-core, plus every `unsafe` block in jmap-backend-book, jmap-backend-cal, jmap-mail. Secondary: the hand-written parsers (jmap-ical lexer, jmap-vcard syntax) against hostile input.

Hunt specifically:
- struct layout mismatches vs the installed headers: verify bindgen output against g_type_query sizes; add runtime assertion tests where a mismatch is conceivable
- vfunc trampolines: any panic path that can cross the C boundary (catch_unwind coverage, non-abort drops), NULL/dangling pointer assumptions, arguments Camel/EDS may legally pass that the code rejects
- GObject memory: refcount errors, floating refs, g_free vs Rust Drop confusion, string ownership at the boundary (transfer full vs transfer none), GError leaks on error paths
- threading: every `unsafe impl Send/Sync` — justified by EDS's actual threading model or wishful? Which threads call which vfuncs?
- integer conversions: u16 class sizes, usize/u64/i32 casts, silent truncation
- parser robustness: malformed, oversized, deeply nested, escape-abusing iCalendar/vCard/JSCalendar input — write fuzz-style unit tests
- security regressions vs ROADMAP rules: TLS-by-default, plaintext-to-loopback-only, credentials only via ESourceAuthentication

Method: read first, then PROVE each suspected defect with a failing or asserting test (the test is the finding's evidence). Fix only clear-cut bugs, each fix its own commit referencing the finding; document design-level concerns instead of refactoring.

Output: docs/AUDIT-FFI.md on branch audit/ffi. One section per finding: severity (critical/major/minor/info), file:line, why it is wrong or exploitable, evidence (test name), fix commit or recommendation. Also record what was audited and found CLEAN — absence of findings must be distinguishable from absence of looking. When every listed area is covered, end the file with the single line: AUDIT COMPLETE

Rules: commit style per docs/ROADMAP.md (SPDX headers, no Co-Authored-By trailers, clippy/test/fmt green before push). NEVER push to master. NEVER touch ~/evolution-jmap (the roadmap shift's checkout). End the session promptly when the increment is pushed.
