<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT comment for GNOME/evolution#411, "Schedule sending an email to a later date and time"

**Status: draft, not filed.** Review before posting. This is a *comment on the
existing issue*, deliberately not a new report: #411 is open and already notes
that JMAP could schedule server-side, so a duplicate would only add noise.

**Revision note (2026-08-29):** an earlier draft proposed carrying the send
time as an X-header on the `CamelMimeMessage`. That was a workaround chosen to
avoid an ABI break, and it is the wrong thing to propose. It smuggles control
data through the payload, and it delivers scheduling while still leaving the
user unable to see or cancel what is pending. Rewritten around the shape the
problem actually has.

---

Some protocol-level detail that may be useful for scoping this, from writing a
JMAP backend for Evolution (`evolution-jmap`). Short version: two of
Evolution's account types can already schedule sends server-side, and the thing
standing in the way looks less like a missing feature than like an API shape.

## Two backends could delegate this to the server today

For these, Evolution would not need to stay running, hold the message in a
local Outbox, or run a timer:

- **JMAP** (RFC 8621 §7): `EmailSubmission` carries `sendAt`, and `undoStatus`
  lets a still-pending send be cancelled. It is conditional. The server
  advertises `maxDelayedSend` in seconds in its
  `urn:ietf:params:jmap:submission` capability, where 0 means unsupported, and
  it is backed by the SMTP FUTURERELEASE extension (RFC 4865).
- **EWS**: Exchange supports deferred delivery by setting the
  `PR_DEFERRED_SEND_TIME` / `PR_DEFERRED_DELIVERY_TIME` MAPI properties as
  extended properties on the send request. `evolution-ews` does not do this
  today; `ews_send_to_sync()` sends immediately.

## The obstacle is that Camel models sending as a call, not as an object

```c
gboolean (*send_to_sync) (CamelTransport *transport,
                          CamelMimeMessage *message,
                          CamelAddress *from,
                          CamelAddress *recipients,
                          gboolean *out_sent_message_saved,
                          GCancellable *cancellable,
                          GError **error);
```

This encodes the assumption that sending is instantaneous and final: you call
it, you get `TRUE`, it is over. That stops being true the moment a message can
rest in a server-side queue. Then sending has a lifecycle (pending, then sent
or failed or cancelled), and the queue is owned by something other than
Evolution.

So the shape that seems right is that **submitting returns a submission
object** rather than a boolean, and a transport gains operations over it:
enumerate pending submissions, query one's state, cancel one. Alongside it
there would be a capability, "I can defer up to N seconds", with 0 meaning
never, so the composer can enable *Send later* per account and explain why it
is unavailable rather than offering it all-or-nothing.

Three things suggest this is the right shape rather than merely a larger one.

**The protocols already are that object.** JMAP's `EmailSubmission` has an id,
a `sendAt`, an `undoStatus`, and is queryable. It is precisely this, and the
current API flattens it into a boolean. Exchange's deferred message sits in the
server Outbox and can be deleted. FUTURERELEASE yields a queue entry. Camel is
the only layer in the stack pretending the queue is not there.

**The existing signature is already straining.** The
`gboolean *out_sent_message_saved` out-parameter exists because "did the server
keep its own copy?" could not be expressed by the return value. That is the
same pressure, namely that a send has richer outcomes than success or failure,
being answered one out-parameter at a time.

**It would subsume undo-send.** The "send through the Outbox after N minutes"
mechanism (#122, and #1461 about which folder it uses) exists *because* there
is no cancellable submission. With one, undo-send is `sendAt = now + 30s` plus
a cancel, and unlike the present version it survives closing the laptop,
because the server holds the message rather than Evolution. One change answers
three open requests.

There may also be a structural reason there is no natural home for this today:
a Camel store and a Camel transport are two services with no pointer between
them, which is Camel's shape rather than JMAP's. A submission is exactly the
missing link, since it refers to a message and to an identity.

## Cost, stated plainly

This is an ABI break on `CamelTransportClass` plus real new API, and the mailer
would have to stop treating a send as synchronous-and-final. That is presumably
why it has not happened. If the break is unacceptable, the same end state is
reachable by adding a parallel `submit_sync()` that returns a submission while
`send_to_sync()` stays for immediate sends, with providers implementing either
or both.

I am an outsider proposing changes to your ABI, so please read this as "here is
what the protocols underneath can do and where they currently hit a wall"
rather than as a design demand; you will have context I do not. If there is
interest in the Camel-side shape, I am happy to do the JMAP-side work against
whatever it ends up being.
