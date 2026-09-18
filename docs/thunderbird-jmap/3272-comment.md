Draft comment for https://github.com/thunderbird/thunderbird-android/issues/3272
Status: DRAFT, not posted. The operator posts; agents never post to GitHub.
Last updated: 2026-09-15.

---

I spent some time this week assessing what it would take to finish the
`backend/jmap` module (added in #4459, milestone #26) and get JMAP support
into a usable state. Posting the findings here in case they save someone
duplicate work, and because #26's items are still open six years on.

**The module itself is in better shape than I expected.** At current `main`
it compiles against the `Backend` interface with no drift, and its 14 tests
pass. Folder list sync, message list sync (`Email/query` + `queryChanges`),
flags, move and delete, and blob upload are implemented. What's missing is
everything that makes an account usable end to end: a `BackendFactory` and
DI registration (there is none today; the module is listed in
`settings.gradle.kts` but nothing depends on it), the account setup screen
(#4566, #4573), sending (#4564), on-demand download and search (#4565,
#4567), push, and about twenty places in setup/validation/export that
assume IMAP, POP3 or SMTP.

I also found one correctness issue worth flagging regardless of what
happens with the rest: `CommandDelete.deleteMessages` does a JMAP
`Email/set destroy`, which removes the message from every mailbox it
belongs to, not just the one being synced. On a server that models labels
as multiple mailbox membership (Fastmail, Stalwart), deleting from one
folder view would delete everywhere.

I've written up a fuller phased plan (roadmap, effort estimate, the
account-model decision, the library situation) and I'm working through it
on a personal fork, starting from a `jmap-client` version bump (I noticed
#9787 was closed with "ignore this dependency" — happy to discuss what
would make that dependency acceptable, or to scope a smaller in-house
client if the Guava/Gson footprint is the concern).

If there's interest, I'm glad to send small, focused PRs following
`AGENTS.md` as the pieces land (starting with the dependency bump and a
couple of correctness fixes to the existing module, which don't depend on
any of the UI work). Would also value a steer on priority given "JMAP
Support Exploration" is now on the 2026 Android roadmap.
