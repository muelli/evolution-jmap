Autonomous polish session (Antigravity). Repo: this checkout, on the `antigravity` branch.

Your lane is LOW-PRIORITY, HEADLESS work, listed in `docs/AGY-TASKS.md`. Claude works
the priority items (M7 account UI, real-server/OAuth2, M9, M10) on `master`; you must
stay out of that lane so the periodic `antigravity → master` merge stays clean.

Task:
0. Read `docs/AGY-TASKS.md` (your task list) and `docs/AGY-LOG.md` (what you have already
   done). Work the PRIMARY task there — the `calcard` migration — which is a MULTI-SESSION
   effort: pick the next coherent sub-step you can finish AND verify this session (~30–90
   min). A task spanning many sessions is normal and expected — do NOT report BLOCKED just
   because it will not finish in one session; only report blocked when there is genuinely
   no sub-step you can make progress on at all.
1. STAY IN YOUR LANE. Touch only `rust/crates/jmap-vcard`, `rust/crates/jmap-ical`, and the
   files `docs/AGY-TASKS.md` names. Do NOT edit any of these (Claude's priority lane —
   editing them causes merge conflicts): `jmap-config`, `oauth2*`, `jmap-mail` / the Camel
   provider, `eds-sys`, the collection backend, anything under `.github/` or `infra/`,
   `docs/ROADMAP.md`, `docs/NIGHT-LOG.md`, or `docs/BACKLOG.md`. If `docs/AGY-TASKS.md` has
   no sub-step you can progress headlessly, print the single line
   `AGY-SHIFT: BLOCKED — <one-line reason>` and end immediately, committing nothing. Do not
   stray into priority files. The driver pauses the shift after 3 such reports in a row
   (until the maintainer adds tasks).
2. TDD: write the red test first, then make it green. Before committing:
   `cargo test` green for the crates you touched, `cargo clippy --all-targets -- -D
   warnings` clean, and SPDX `GPL-3.0-or-later` headers on any new files (reuse lint).
3. Log to `docs/AGY-LOG.md` — NOT `docs/NIGHT-LOG.md` (that is Claude's; sharing it
   guarantees a merge conflict every integration). Append: UTC date, which AGY-TASKS sub-step,
   what you changed, any calcard behaviour-difference findings, and the gates you ran. Do NOT prune `docs/BACKLOG.md` — the
   maintainer removes finished items at merge time.
4. Small commits, imperative subject, NO Co-Authored-By trailer. Commit to the
   `antigravity` branch and `git push origin HEAD`. Never force-push, never rewrite
   history. If a push is rejected, `git pull --rebase` your own branch and retry.
5. End the session after ONE increment.

You are on a branch the maintainer merges into `master` every so often. Keep each
increment small and self-contained so those merges stay trivial.
