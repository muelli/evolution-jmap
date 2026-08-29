<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT comment for GNOME/evolution#411 — "Schedule sending an email to a later date and time"

**Status: draft, not filed.** Review before posting. This is a *comment on the
existing issue*, deliberately not a new report — #411 is open and already
notes that JMAP could schedule server-side, so a duplicate would only add
noise. What follows is the API-level detail the issue does not yet have.

---

Some protocol-level detail that may be useful for scoping this, from writing a
JMAP backend for Evolution (`evolution-jmap`).

**Two of Evolution's account types can already schedule server-side.** For
these, Evolution would not need to stay running, hold the message in a local
Outbox, or run a timer:

- **JMAP** (RFC 8621 §7): `EmailSubmission` carries `sendAt`, and
  `undoStatus` lets a still-pending send be cancelled. It is conditional —
  the server advertises `maxDelayedSend` (seconds) in its
  `urn:ietf:params:jmap:submission` capability, and 0 means "not supported" —
  and it is backed by the SMTP FUTURERELEASE extension (RFC 4865).
- **EWS**: Exchange supports deferred delivery by setting the
  `PR_DEFERRED_SEND_TIME` / `PR_DEFERRED_DELIVERY_TIME` MAPI properties as
  extended properties on the send request. (`evolution-ews` does not do this
  today — `ews_send_to_sync()` sends immediately.)

**The blocker looks like the Camel API rather than any one backend.**
`CamelTransportClass::send_to_sync` is:

```c
gboolean (*send_to_sync) (CamelTransport *transport,
                          CamelMimeMessage *message,
                          CamelAddress *from,
                          CamelAddress *recipients,
                          gboolean *out_sent_message_saved,
                          GCancellable *cancellable,
                          GError **error);
```

There is nowhere to put "send at T". So even where the server would happily
take the instruction, no provider can pass it on, and every backend is forced
into the client-side-Outbox approach regardless of what its protocol offers.

**A possible shape**, if this is ever picked up: carry the requested send time
on the `CamelMimeMessage` itself (an X-header consumed and stripped by the
transport, or a Camel-level property), so the vfunc signature need not change
and providers that cannot schedule can ignore it and let Evolution fall back to
the existing Outbox behaviour. Providers that can would then delegate, and
`undoStatus`-style cancellation could hang off the same path.

Happy to be told this is the wrong layer — mostly flagging that the capability
exists in at least two protocols and is currently unreachable from Evolution.
