Autonomous night session. Repo: ~/evolution-jmap (cwd).

Task: read docs/ROADMAP.md, docs/NIGHT-LOG.md (if present), git log --oneline -15. 
1. Dependency Analysis: Treat ROADMAP.md as a dependency graph (e.g. M3, M4, M5 can be built in parallel after M2). Identify all currently unblocked and incomplete milestones.
2. Claiming: Select ONE unblocked task. Before writing code, append a lock entry to `docs/NIGHT-LOG.md` (e.g., "Claiming M3 increment: [description]") and `git push` it. If the push fails, another agent claimed it; pull, rebase, and pick a different unblocked task. **Deadlock handling**: If a task was claimed more than 24 hours ago (check git log for the lock commit timestamp) and shows no subsequent progress, consider the lock expired and claim it.
3. Execution: Implement ONE focused increment (~30-90 min of work): TDD, red test first, then green.

Hard rules (ROADMAP "Rules for autonomous work sessions" applies in full):
- Before every push: cargo test green, cargo clippy --all-targets -- -D warnings clean, reuse lint green (SPDX GPL-3.0-or-later headers on new files).
- Small commits, imperative subject, NO Co-Authored-By trailer. Plain git push, never force, never rewrite history.
- Crates needing EDS headers stay OUT of default-members in rust/Cargo.toml.
- Never touch infra/ or .github/workflows/ci-image.yml.
- Append a session entry to docs/NIGHT-LOG.md (UTC date, what was done, decisions, blockers); commit it with the work.
- Blocked >20 min on one approach: log the blocker, switch to the next tractable item.
- End the session promptly once the increment is pushed. Do not start a second large item.

Environment: this VM has full EDS 3.52 dev headers, libclang, cmake, ninja. pkg-config resolves libebackend-1.2, libedata-book-1.2, libedata-cal-2.0, camel-1.2. Mock server: cargo run -p evolution-jmap-mock --bin jmap-mockd.
