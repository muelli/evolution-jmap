Autonomous night session. Repo: ~/evolution-jmap (cwd).

Task: read docs/ROADMAP.md, docs/NIGHT-LOG.md (if present), git log --oneline -15. Determine next incomplete milestone. Implement ONE focused increment (~30-90 min of work): TDD, red test first, then green.

Hard rules (ROADMAP "Rules for autonomous work sessions" applies in full):
- Before every push: cargo test green, cargo clippy --all-targets -- -D warnings clean, reuse lint green (SPDX GPL-3.0-or-later headers on new files).
- Small commits, imperative subject, NO Co-Authored-By trailer. Plain git push, never force, never rewrite history.
- Crates needing EDS headers stay OUT of default-members in rust/Cargo.toml.
- Never touch infra/ or .github/workflows/ci-image.yml.
- Append a session entry to docs/NIGHT-LOG.md (UTC date, what was done, decisions, blockers); commit it with the work.
- Blocked >20 min on one approach: log the blocker, switch to the next tractable item.
- End the session promptly once the increment is pushed. Do not start a second large item.

Environment: this VM has full EDS 3.52 dev headers, libclang, cmake, ninja. pkg-config resolves libebackend-1.2, libedata-book-1.2, libedata-cal-2.0, camel-1.2. Mock server: cargo run -p evolution-jmap-mock --bin jmap-mockd.
