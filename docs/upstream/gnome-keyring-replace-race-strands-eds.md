# `gnome-keyring-daemon --replace` strands every client that already connected

**Status:** draft, NOT filed, and **scope corrected 2026-08-29 — read this
first.** The `--replace` in the capture below is *self-inflicted*: a test-VM
autostart the maintainer added on 2026-08-23
(`~/bin/unlock-keyring.sh`, `echo -n "password" | gnome-keyring-daemon
--replace --unlock --components=secrets,pkcs11`) to unlock the login keyring on
an autologin session. Stock GNOME does not do this. An earlier revision of this
document blamed `gnome-session` for launching the replacement; that was wrong
and is retracted.

What survives the correction, and is the only thing worth filing, is what
happens *when* a replacement occurs: `--replace` is a documented, supported
option, and using it strands already-connected clients for the life of their
processes, with libsecret logging GLib's own detection of an unreturned
`GTask`.

**Candidate owners:** libsecret (primary), gnome-keyring (secondary).

## Summary

`gnome-keyring-daemon --replace` **replaces** the running keyring daemon.
Clients that connected before the replacement are not migrated and have no way
to notice.

Every client that connected in those few seconds keeps a D-Bus proxy bound to
the replaced daemon's *unique* name. Because unique names cannot be
auto-activated, subsequent secret lookups either fail with

```
The name :1.4 was not provided by any .service files
```

or hang until the GDBus default timeout of 25 seconds. The client is stranded
for the lifetime of its process. Restarting the client fixes it completely.

For evolution-data-server this means stored OAuth 2.0 tokens become unreadable,
so Evolution shows a fresh consent window for an account whose refresh token is
sitting in the keyring, unlocked, the whole time.

## Observed timeline (one boot, one session)

All from `journalctl --user -b`. `:1.4` is `gnome-keyring-daemon` PID 1173,
proven by the bus's own activation record:

```
20:43:33  dbus-daemon: Activating service name='org.gnome.keyring.SystemPrompter'
          requested by ':1.4' (uid=1000 pid=1173 comm="/usr/bin/gnome-keyring-daemon ...")
```

| Time | Event |
|---|---|
| 20:43:32 | `evolution-source-registry` starts (PID 1496) |
| 20:43:33 | `evolution-calendar-factory` (1736), `evolution-addressbook-factory` (1760) start |
| 20:43:33 | registry: `fetching OAuth 2.0 access token via EDS` |
| 20:43:33 | calendar factory: `fetching OAuth 2.0 access token via EDS` |
| 20:43:34 | `gnome-keyring-daemon[1173]: couldn't prompt for password: The operation was cancelled` |
| 20:43:34 | registry reports the store locked, declines to escalate |
| 20:43:36 | `gcr-prompter: caller vanished for callback /org/gnome/keyring/Prompt/p3@:1.4` |
| 20:43:36 | `evolution-sourc[1496]: GTask secret_service_real_prompt_async (...) finalized without ever returning (using g_task_return_*()). This potentially indicates a bug in the program.` |
| 20:43:36 | `Started app-gnome-unlock\x2dkeyring-1983.scope` (the test VM's own autostart, see the status note) |
| 20:43:36 | `gnome-keyring-daemon[2004]: Replacing daemon, using directory: /run/user/1000/keyring` |
| 20:43:58 | calendar factory: `failed to obtain OAuth 2.0 access token` (**25 s** after its 20:43:33 call) |

The three EDS daemons all started 3 to 4 seconds *before* the replacement. The
keyring that replaced it is unlocked and stays unlocked:

```
$ busctl --user call org.freedesktop.secrets \
    /org/freedesktop/secrets/collection/login \
    org.freedesktop.DBus.Properties Get ss \
    org.freedesktop.Secret.Collection Locked
v b false
```

and the token is present in it, as a single item labelled
`Evolution Data Source - JMAP[...]`. Nothing is missing and nothing is locked.

Half an hour later, with the user driving the GUI, the same stall repeats
against the same stranded daemons:

| Time | Event |
|---|---|
| 21:10:06 | user starts `evolution` (PID 2727) |
| 21:10:08 | `prepared OAuth 2.0 authentication uri query` — consent window shown |
| 21:10:55 | user completes consent; `prepared OAuth 2.0 get token form` |
| 21:10:56 | `gnome-keyring-daemon: asked to register item .../login/21, but it's already registered` |
| 21:10:56 | calendar factory: `fetching OAuth 2.0 access token via EDS` |
| 21:11:21 | calendar factory: `failed to obtain OAuth 2.0 access token` (**25 s** again) |

Two independent stalls, both exactly 25 seconds, the GDBus default timeout.
The user consented successfully and it changed nothing, because the process
that needed the secret could no longer reach any keyring.

## Proof by restart

Killing only `evolution-source-registry` and letting D-Bus reactivate it, with
no other change — keyring untouched, no new consent, same stored token:

```
21:15:55  evolution-source-registry[3741]: fetching OAuth 2.0 access token via EDS
21:15:55  evolution-source-registry[3741]: obtained OAuth 2.0 access token
21:15:55  evolution-source-registry[3741]: GET https://api.fastmail.com/.well-known/jmap
21:15:55  evolution-calendar-factory[3749]: obtained OAuth 2.0 access token
21:15:57  evolution-source-registry[3741]: POST https://.../jmap/api/
```

Same second, no prompt, no stall, account live. The only variable changed was
*when the process connected to the keyring relative to the replacement*.

## Who owns this

Three distinct defects sit on top of each other. Any one of them being fixed
would make the symptom much less severe.

1. **libsecret** — the GLib warning is explicit and self-diagnosing:
   `GTask secret_service_real_prompt_async ... finalized without ever returning`.
   A prompt whose peer disappears mid-flight must complete with an error, not
   be finalized silently. As it stands the caller's async op never completes,
   which is what turns a recoverable disconnect into a hang.

2. **gnome-keyring** — `--replace` tears down a daemon that has prompts in
   flight and clients connected, with no mechanism for those clients to
   rebind. A daemon replacement that expects clients to survive it needs
   either a well-known-name handoff clients can follow, or a signal that tells
   them to re-resolve.

**Not** gnome-session, and **not** evolution-data-server. An earlier revision
of this document blamed the former; the replacement was the test VM's own
autostart. EDS is simply a client that connected first.

The blast radius is not Evolution-specific: any client that resolves
`org.freedesktop.secrets` before a replacement is affected for its whole
lifetime.

## Is this worth filing at all?

Honestly assessed: the libsecret half is. `GTask ... finalized without ever
returning` is GLib telling us about a defect in so many words, it is
reproducible, and it converts a recoverable disconnect into a hang. The
gnome-keyring half is weaker — "don't use `--replace` while clients are
connected" is a defensible answer, though the option's documentation says
nothing about it.

Before filing, check whether the login keyring can simply be given an empty
password on the affected machine, which removes the need for any replacement
and is the correct fix for this VM.

## What clients can do meanwhile

Not much, and nothing good. `SecretService` is a process-wide singleton in
libsecret, so a client cannot cheaply drop and re-resolve it. Watching
`NameOwnerChanged` for `org.freedesktop.secrets` and re-resolving is possible
but duplicates what libsecret should be doing. Setting a shorter D-Bus timeout
converts a 25-second hang into a fast failure without fixing the underlying
inability to reach the keyring.

## Reproduction

1. Start a session and let a secret-service client connect (any OAuth 2.0
   account in Evolution will do).
2. Run `gnome-keyring-daemon --replace --unlock --components=secrets,pkcs11`,
   feeding the keyring password on stdin.
3. Drive the client to read a secret. Observe in `journalctl --user -b`: a
   lookup that fails naming a `:N.M` unique name, or stalls for 25 seconds,
   plus libsecret's `GTask ... finalized without ever returning` in the
   client's own log.
4. Restart the client and observe the same lookup succeed immediately.

## Version information

Ubuntu 24.04.4 LTS.

| Package | Version |
|---|---|
| gnome-keyring | 46.1-2build1 |
| libpam-gnome-keyring | 46.1-2build1 |
| libsecret-1-0 | 0.21.4-1build3 |
| gnome-session-bin | 46.0-1ubuntu4 |
| gnome-session-common | 46.0-1ubuntu4 |
| libglib2.0-0t64 | 2.80.0-6ubuntu3.8 |
| evolution-data-server | 3.52.3-0ubuntu1.2 |
| evolution | 3.52.3-0ubuntu1.1 |

The JMAP backends whose log lines appear above are a third-party plugin
(evolution-jmap). They are incidental: they are simply what was configured as
an OAuth 2.0 account on this machine, and they are the source of the
`fetching OAuth 2.0 access token via EDS` / `obtained OAuth 2.0 access token`
lines. The failure is entirely inside the EDS credential path and the secret
service beneath it; any OAuth 2.0 account would show it.
