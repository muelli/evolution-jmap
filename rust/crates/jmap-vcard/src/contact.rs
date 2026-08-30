// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact [`ContactCard`] ↔ vCard 3.0.
//!
//! The mapped set is deliberately the one the address book backend needs to
//! be useful — UID, FN, N, NICKNAME, EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE,
//! NOTE, BDAY, URL, CALURI, FBURL, PHOTO, CATEGORIES and the `X-` lines EDS
//! keeps instant-messaging handles, spouse, manager, assistant, blog URL,
//! and video URL on —
//! and no more. Everything else on a card (preferred languages, the crypto keys
//! it lists, what the contact is spoken to as, …) is *dropped*, which is only
//! safe because saving goes back to the server as a PatchObject naming the
//! mapped properties: a property we never mapped is a property we never
//! overwrite.
//!
//! `ORG` is the one property whose *value* is a list rather than a field: RFC
//! 2426 §3.5.5 states the organisation's name and then the units within it,
//! in the order RFC 9553 §2.2.3's `units` gives them, so an entry crosses as
//! one line with as many components as it has units. What does not cross is
//! the entry's `sortAs` and `contexts`, which `ORG` has no component and no
//! parameter for — hence the `X_JMAP_KEY` this side already writes on an
//! `EMAIL`, and a save that patches `organizations/<key>/name` in place.
//!
//! `titles` is the one property of which only *some* entries cross. RFC 9553
//! §2.2.4 keeps the job title and the role played in one map, told apart by
//! `kind`, and allows vendor kinds besides those two; vCard 3.0 has exactly
//! `TITLE` and `ROLE`. An entry of any other kind is therefore dropped rather
//! than written on a line that would misstate it — and, as with every other
//! dropped thing here, the save path must then leave it alone.
//!
//! `addresses` is lossy the same way one level down, and is the one property
//! that crosses on two lines. RFC 2426 §3.2.1's `ADR` has seven fields; RFC
//! 9553 §2.5.1 builds an address out of named components, sixteen kinds of
//! them. Seven of those kinds have a field of their own
//! (`ADDRESS_COMPONENTS`) and one more, the house `number`, shares the
//! street's (`JOINED_COMPONENTS`); the rest — `floor`, `room`, `landmark` —
//! have nowhere to go, and are left off the line rather than written into a
//! field that would say something else about them. Beside the `ADR`
//! goes RFC 2426 §3.2.2's `LABEL`, the address written out as it should be
//! printed, which is RFC 9553's `full` and what EDS keeps in its three
//! synthetic address-label fields. An address may have either line, or both:
//! `full` stands on its own for an address "even if the individual address
//! components are not known", and an address stated only in components vCard
//! has no field for has neither line and is invisible — so `addresses` too is
//! a map of which the vCard states only some entries.
//!
//! `notes` is the plainest of them and lossy only around the value: RFC 2426
//! §3.6.2's `NOTE` is free text, so an entry's own text crosses whole, while
//! RFC 9553 §2.8.3's `created` and `author` — when the note was written and
//! by whom — have no component and no parameter to sit in, and so ride along
//! in the entry's `extra` for the save to patch around. An entry saying
//! nothing at all gets no line, which is the same invisibility again.
//!
//! `anniversaries` is lossy in its *value*, which is new: RFC 9553 §2.8.1
//! dates a memorable event either as a `PartialDate`, which may state as
//! little as a year, or as a `Timestamp`, which states a point in time. A
//! vCard date line states one calendar day and nothing else, so a date that
//! names no single day gets no line — not because the line has nowhere to put
//! it, but because EDS reads anything short of a whole date as *no* date and
//! would show the user 1000-01-01. A whole date before the year 1000 gets no
//! line for the neighbouring reason: EDS reads it correctly and writes it back
//! clamped, so the line would come home naming a different millennium — see
//! `Day::survives_the_field_it_lands_in`. A point in time crosses as the
//! day it falls on, leaving the hour behind for the save to patch around
//! ([`states_a_point_in_time`]). Of the three kinds, `birth` goes on RFC 2426
//! §3.1.5's `BDAY` and `wedding` on the line EDS reads `E_CONTACT_ANNIVERSARY`
//! off; `death` has no line at all, so `anniversaries` too is a map of which
//! the vCard states only some entries.
//!
//! `nicknames` is the one property whose vCard *cardinality* is the decision.
//! RFC 2426 §3.1.3 states the nicknames as one comma-separated list on a
//! single `NICKNAME` line, which would leave RFC 9553 §2.2.2's keyed entries
//! with nowhere to carry a key each — so an entry gets a line of its own
//! instead, as `NOTE` and `TITLE` already do. That is also what EDS does with
//! the value: measured against libebook-contacts 3.52, it hands the whole
//! value back as one string rather than splitting it on commas, and escapes a
//! comma the user typed, so a list on one line would reach the contact editor
//! as a single nickname with commas in it. Unlike a date line, the `NICKNAME`
//! is rewritten in place with its parameters intact, so the `X_JMAP_KEY`
//! survives the trip through Evolution and the save needs no rekeying. An
//! entry that names nothing gets no line, which is the same invisibility again.
//!
//! `links` is `titles`' shape with a stricter filter: RFC 9553 §2.6.3 keys the
//! resources a card points at and tells them apart by `kind`, of which it
//! defines exactly one — `contact`, a URI for writing to the person, which RFC
//! 9555 §2.6.3 states on vCard 4.0's `CONTACT-URI`. vCard 3.0 has only RFC
//! 2426 §3.6.8's `URL`, the plain website, so *only* an entry naming no kind
//! at all crosses; `contact` and any vendor kind get no line rather than one
//! that would tell the user this is the contact's home page. What also does not
//! cross is the entry's `mediaType`, `contexts`, `pref` and `label`, for which
//! a bare URI has no parameter, so they ride in its `extra` as a nickname's do.
//! The `X_JMAP_KEY` survives here too: measured against libebook-contacts
//! 3.52, `E_CONTACT_HOMEPAGE_URL` is the first `URL` line, a set rewrites that
//! line's value and leaves its parameters alone, and any further `URL` line
//! passes through untouched.
//!
//! `calendars` is the first property whose `kind` picks the *line* rather than
//! deciding whether there is one. RFC 9553 §2.4.1 keys the calendaring
//! resources of the contact and tells them apart by a `kind` it makes
//! mandatory: a `calendar` of theirs, or the `freeBusy` data drawn from one.
//! RFC 9555 §2.13.2 and §2.13.3 state those on `CALURI` and `FBURL`, which are
//! vCard 4.0 properties (RFC 6350 §6.9.3 and §6.9.1) — and they are written on
//! the 3.0 card anyway, because EDS keeps `E_CONTACT_CALENDAR_URI` and
//! `E_CONTACT_FREEBUSY_URL` on exactly those lines whatever the version says,
//! measured against libebook-contacts 3.52. So the two kinds cross and an entry
//! naming any other — or, malformed, naming none — gets no line at all, since
//! nothing then says which of the two fields it belongs in and both are in
//! front of the user under a heading of their own. What does not cross is the
//! entry's `mediaType`, `contexts`, `pref` and `label`, for which neither line
//! has a parameter, so they ride in its `extra` as a link's do. The
//! `X_JMAP_KEY` survives here too, for the reason it survives on a `URL`: a
//! set rewrites the first line of that name in place and leaves its parameters
//! alone, and any further line of the same name passes through untouched.
//!
//! `contexts` is the one member that crosses on four different properties, and
//! the one narrowed by what EDS does with the parameter rather than by what
//! vCard can spell. RFC 9553 §1.5.1 lets an entry name every context it belongs
//! to; an `ADR` and a `TEL` state exactly one, because EDS picks the field a
//! line lands in by matching `TYPE`, and a line wearing both matches two — one
//! address filling both the Home and the Work block of Evolution's contact
//! editor, with one line behind them, so retyping either rewrites the other.
//! See `context_slot` for the measurement and [`states_context`] for what the
//! save then does about the context left off. An `EMAIL` is not narrowed: EDS
//! files it by position rather than by `TYPE`. A phone's `features` are
//! narrowed the same way and for the same reason — `TYPE=WORK,VOICE,FAX` fills
//! the Business Phone and the Business Fax field alike, and without a context
//! `TYPE=VOICE,FAX` fills neither, leaving the number in no field at all. See
//! `feature_slot` and [`states_phone_feature`].
//!
//! `onlineServices` is the one property vCard 3.0 has no line for at all. RFC
//! 9553 §2.3.2 names the contact as one service or protocol knows them; RFC
//! 4770's `IMPP` is vCard 4.0, which is not the format
//! `e_contact_new_from_vcard()` is handed, so the line is the `X-` one EDS
//! itself keeps a handle on — `ONLINE_SERVICES` — and the mapping states only
//! the ten services libebook-contacts 3.52 gives contact-editor slots to. Which
//! makes the property lossy in three separate places: a service EDS has no field
//! for has no line, an entry stating a `uri` and no `user` has one only where
//! `SERVICE_SCHEMES` says the URI holds the handle and nothing else (see
//! `drawn_service`), and neither has a handle the line would come back from
//! EDS having renamed.
//!
//! Its `TYPE` is also the one parameter here that is *not* the JSContact member
//! it looks like. It is the slot EDS files the handle in — `HOME` or `WORK`,
//! measured — and a line carrying none reaches no field the user can see, so
//! every line must carry one whether the entry states a context or not. It is
//! therefore written from the entry's `contexts` where they say something and
//! never read back off the line — which is why [`OnlineService`] models no
//! `contexts` of its own, and lets them ride in its `extra` as a nickname's do.
//!
//! `keywords` is the first mapped property that is a *set* rather than a keyed
//! map, and the first to cross on one line holding all of it. RFC 9553 §2.8.2
//! files a card under bare-string tags; RFC 2426 §3.7.1's `CATEGORIES` lists
//! them, comma-separated, and EDS reads that line as `E_CONTACT_CATEGORY_LIST` —
//! Evolution's Categories field. There is nothing inside an entry to preserve
//! and no key to patch by, so unlike every property above it the set goes back
//! **replaced whole**, and a tag the line cannot carry is therefore a tag the
//! next save would *delete* rather than merely one the user cannot see — unless
//! the save puts it back, which is what it does. What those tags are is
//! [`states_keyword`]. One line rather than one per tag — the opposite of the
//! `NICKNAME` decision — because a second `CATEGORIES` line reaches no field the
//! user can see, measured against libebook-contacts 3.52; the reader takes them
//! all in anyway, so tags on a line EDS ignores are not lost by a save.
//!
//! `media` is the one mapped property whose value is not text. RFC 9553 §2.6.4 keys the media a card
//! carries and tells a photo, a sound and a logo apart by a `kind` it makes
//! mandatory; RFC 2426 §3.1.4's `PHOTO` is the picture *of the contact*, which
//! is what Evolution shows beside the name, so only a photo crosses and the
//! other kinds get no line — `titles`' filter again. A picture the card
//! *carries* arrives as a `data:` URI (RFC 2397) and crosses as the bytes
//! themselves under `ENCODING=b`, which is the only form EDS reads a media type
//! off: measured against libebook-contacts 3.52, `TYPE=JPEG` becomes
//! `image/JPEG` and `TYPE=image/jpeg` becomes `image/image/jpeg`, so the
//! parameter states the subtype alone (`image_subtype`). A picture the card
//! only *points at* crosses as a `VALUE=uri` reference — the shape EDS's own
//! writer emits, and one it reads as no picture at all when that parameter is
//! missing, also measured. What else gets no line is a `data:` URI spelling its
//! bytes as percent-encoded octets rather than base64, since `ENCODING=b` is
//! the only encoding the line carries (`photo`).
//!
//! A `PHOTO` line is read back into a `media` entry the same way (`read_photo`),
//! so the picture the *user* chooses in Evolution reaches the server. Only the
//! two forms above are read, because they are the two EDS's own writer emits;
//! what a line the reader has to be careful about is spelled out there. The
//! sounds and logos a card carries have no vCard 3.0 property at all and so come
//! back from nothing — the save patches around them, as it does around every
//! other entry the emitter left off.
//!
//! What the save cannot lean on is the `X_JMAP_KEY`: unlike a `NICKNAME`'s,
//! the key on a `PHOTO` line does not survive an edit. EDS rebuilds the line out
//! of the photo it holds and writes none of the parameters back, exactly as it
//! does for a date line (measured against libebook-contacts 3.52) — so the entry
//! a chosen picture belongs to is found by pairing rather than by key, which is
//! `jmap-book-sync`'s `diff_media`.
//!
//! `relatedTo` is the one property whose *key* is what crosses. RFC 9553 §2.1.8
//! keys the entities a card relates to by the related Card's `uid` and says how
//! each relates in a set of types; vCard 3.0 has no `RELATED` — RFC 6350 §6.6.6
//! is 4.0 — and of the twenty types, `spouse` is the one Evolution has a field
//! for, on the line EDS keeps `E_CONTACT_SPOUSE` on
//! (`X_EVOLUTION_SPOUSE`). The value on that line is the person's *name*, and
//! the only place a name can be is the key: RFC 9555 §2.9.5 is what says a key
//! may hold free text rather than an identifier, since that is what a vCard
//! `RELATED;VALUE=text` becomes. So an entry keyed by a URI has no line — a URN
//! shown under the heading "Spouse" would be written back as the person's name by
//! the next save — and neither has one keyed by a name EDS would respell, which
//! for this property is not merely a rename but a *different entry*. See
//! [`states_spouse`].
//!
//! Which also makes it the one property that carries no `X_JMAP_KEY`: there is
//! nothing for the parameter to say that the value does not. The reader takes the
//! key off the line's own text, so nothing is invented and no key has to survive
//! Evolution — but a marriage the user retypes arrives under a key the server
//! never had, which is the save's problem rather than this layer's.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_UNPADDED};
use calcard::common::IanaString;
use calcard::vcard::{
    VCardEntry, VCardParameter, VCardParameterName, VCardParameterValue, VCardProperty, VCardValue,
    VCardValueType,
};
use calcard::{Entry, Parser};
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization,
    Relation, Title,
};
use serde_json::{Map, Value, json};

use crate::error::VCardError;

/// Carries the JSContact `uid` when the vCard `UID` is taken by the JMAP id.
const X_JMAP_UID: &str = "X-JMAP-UID";
/// Carries the JSContact map key of an `emails`/`phones` entry.
const X_JMAP_KEY: &str = "X-JMAP-KEY";

/// JSContact name component kinds in reading order, paired with their
/// position in the vCard `N` value (`family;given;additional;prefix;suffix`).
const NAME_COMPONENTS: [(&str, usize); 5] = [
    ("title", 3),
    ("given", 1),
    ("given2", 2),
    ("surname", 0),
    ("credential", 4),
];

/// JSContact `contexts` keys and their vCard `TYPE` spelling.
///
/// A line states at most *one* of them — see [`context_slot`] — because EDS
/// files a line by its `TYPE` into whichever per-context field matches, and
/// matches more than one when the line carries more than one.
const CONTEXTS: [(&str, &str); 2] = [("work", "WORK"), ("private", "HOME")];

/// JSContact phone `features` and their vCard `TYPE` spelling, **most
/// specific first**.
///
/// A line states at most *one* of them, for the reason [`CONTEXTS`] gives, and
/// the order here is which one — see [`feature_slot`].
const PHONE_FEATURES: [(&str, &str); 5] = [
    ("mobile", "CELL"),
    ("pager", "PAGER"),
    ("fax", "FAX"),
    ("voice", "VOICE"),
    ("video", "VIDEO"),
];

/// The vCard `TYPE` parameter names recognized for each JSContact phone feature on inbound parsing.
///
/// In addition to standard vCard 3.0 type names (e.g. `CELL`), recognizes common synonyms
/// emitted by real-world vCard generators (e.g. `MOBILE`).
const PHONE_FEATURE_TYPES: [(&str, &[&str]); 5] = [
    ("mobile", &["CELL", "MOBILE"]),
    ("pager", &["PAGER"]),
    ("fax", &["FAX"]),
    ("voice", &["VOICE"]),
    ("video", &["VIDEO"]),
];

/// JSContact address component kinds, paired with their position in the
/// vCard `ADR` value (RFC 2426 §3.2.1: post office box, extended address,
/// street, locality, region, postal code, country), listed in that order —
/// which is the order a reader gives the components it finds.
const ADDRESS_COMPONENTS: [(&str, usize); 7] = [
    ("postOfficeBox", 0),
    ("apartment", 1),
    ("name", 2),
    ("locality", 3),
    ("region", 4),
    ("postcode", 5),
    ("country", 6),
];

/// JSContact address component kinds that share a vCard `ADR` field with
/// another kind instead of having one of their own, paired with the kind
/// whose field they join.
///
/// RFC 2426 §3.2.1 gives the street address one field, while RFC 9553 §2.5.1
/// lets a card name the street and the house number separately. Leaving the
/// number off the line would take the house out of the address the user
/// reads, so it goes on the street field beside the street name, in the order
/// the card lists its components — which is the only thing that says whether
/// the number is read before the street name (`1 Main Street`) or after it
/// (`Hauptstraße 1`).
const JOINED_COMPONENTS: [(&str, &str); 1] = [("number", "name")];

/// JSContact title `kind` values and the vCard property stating each.
const TITLE_KINDS: [(&str, &str); 2] = [("title", "TITLE"), ("role", "ROLE")];

/// RFC 2426 §3.7.1's `CATEGORIES` — the tags EDS reads as
/// `E_CONTACT_CATEGORY_LIST`, which is Evolution's Categories field.
const CATEGORIES: &str = "CATEGORIES";

/// The one RFC 9553 §2.6.4 media kind a vCard 3.0 `PHOTO` line states: the
/// picture of the contact, which is what Evolution shows.
const PHOTO_KIND: &str = "photo";

/// RFC 2397's URI scheme, which is how a card states a picture it carries
/// rather than one it points at.
const DATA_SCHEME: &str = "data:";

/// What RFC 2397 §3 puts before the comma when the data is base64 rather than
/// percent-encoded octets.
const BASE64_MARKER: &str = ";base64";

/// The media type prefix a `PHOTO` line's `TYPE` states the remainder of; see
/// [`image_subtype`].
const IMAGE_PREFIX: &str = "image/";

/// What EDS puts in a `PHOTO`'s `TYPE` for a picture it holds no media type
/// for: measured against libebook-contacts 3.52, setting a photo whose
/// `mime_type` is NULL writes `TYPE="X-EVOLUTION-UNKNOWN"`. It names no image
/// format, so [`read_photo`] reads it as no media type rather than as
/// `image/X-EVOLUTION-UNKNOWN`, which would tell the server the bytes are a
/// format that does not exist.
const UNKNOWN_TYPE: &str = "X-EVOLUTION-UNKNOWN";

/// The `VALUE` a `PHOTO` line carrying a reference states, rather than the
/// bytes themselves.
const URI_VALUE: &str = "uri";

/// The online services EDS keeps a contact's handles for, paired with the
/// vCard property each is stated on, and spelled as RFC 9553 §2.3.2 invites —
/// the way the service itself does.
///
/// These are exactly the services libebook-contacts 3.52 gives per-slot fields
/// to (`E_CONTACT_IM_JABBER_HOME_1` and its fifty-nine siblings), which is what
/// makes a handle on one of them a handle Evolution's contact editor can show
/// and change. Two of EDS's own instant-messaging fields are deliberately
/// missing: `X-TWITTER`, which it knows only as a multi-valued field with no
/// slots to put a handle in, and `X-SIP`, which is not a service name but a
/// protocol EDS keeps in a field of a different shape.
const ONLINE_SERVICES: [(&str, &str); 10] = [
    ("AIM", "X-AIM"),
    ("Gadu-Gadu", "X-GADUGADU"),
    ("Google Talk", "X-GOOGLE-TALK"),
    ("GroupWise", "X-GROUPWISE"),
    ("ICQ", "X-ICQ"),
    ("Jabber", "X-JABBER"),
    ("MSN", "X-MSN"),
    ("Matrix", "X-MATRIX"),
    ("Skype", "X-SKYPE"),
    ("Yahoo", "X-YAHOO"),
];

/// The URI schemes each service states its handles under, for the services whose
/// scheme names the handle *literally* — `xmpp:vera@jabber.example` is the JID
/// `vera@jabber.example` with a prefix and nothing else.
///
/// This is what lets an entry stating only a `uri` (RFC 9553 §2.3.2 asks for
/// either that or a `user`) reach the free-text field EDS keeps handles in:
/// without it the handle would have to be guessed out of the URI, and the guess
/// would be written back on the next save.
///
/// - `aim` and `aol` for AIM (`aim:<screenname>` or `aol:<screenname>`).
/// - `gg`, `gadugadu`, and `gadu` for Gadu-Gadu (RFC 7595 template `gg:<userid>`,
///   whose path is the numerical user identifier / UIN).
/// - `xmpp` (RFC 5122 §2.1) and `jabber` for Jabber / XMPP JIDs.
/// - `xmpp` and `gtalk` for Google Talk JIDs.
/// - `groupwise` and `novell` for GroupWise.
/// - `icq` for ICQ (`icq:<uin>`).
/// - `msn` and `msnim` for MSN Messenger (`msn:<user>` or `msnim:<user>`).
/// - `matrix` for Matrix bare handle URIs (`matrix:<handle>`).
/// - `skype` is the scheme Skype's own links use, where the bare form
///   `skype:<name>` is the Skype Name and the rest is a query telling the client
///   what to do with it — which [`plain_handle`] refuses.
/// - `yahoo` and `ymsgr` for Yahoo Messenger (`yahoo:<user>` or `ymsgr:<user>`).
///
/// Action/query URIs (such as `aim:goim?screenname=...`, `msnim:chat?contact=...`,
/// `ymsgr:sendim?...`, `icq:message?uin=...`, `matrix:u/vera:...`) are refused by
/// [`plain_handle`] because they carry query parameters or path structures that
/// do not represent bare handles.
///
/// Getting a scheme *wrong* is bounded the same way: a URI whose scheme does not
/// match is not drawn, which is the behaviour of every service missing here.
const SERVICE_SCHEMES: [(&str, &str); 18] = [
    ("AIM", "aim"),
    ("AIM", "aol"),
    ("Gadu-Gadu", "gg"),
    ("Gadu-Gadu", "gadugadu"),
    ("Gadu-Gadu", "gadu"),
    ("Google Talk", "xmpp"),
    ("Google Talk", "gtalk"),
    ("GroupWise", "groupwise"),
    ("GroupWise", "novell"),
    ("ICQ", "icq"),
    ("Jabber", "xmpp"),
    ("Jabber", "jabber"),
    ("MSN", "msn"),
    ("MSN", "msnim"),
    ("Matrix", "matrix"),
    ("Skype", "skype"),
    ("Yahoo", "yahoo"),
    ("Yahoo", "ymsgr"),
];

/// The slot EDS files a handle in when nothing says otherwise, and the only one
/// it writes a handle of its own accord into (measured against
/// libebook-contacts 3.52).
const DEFAULT_SLOT: &str = "HOME";

/// The line EDS 3.52 keeps `E_CONTACT_ANNIVERSARY` on — the field Evolution's
/// contact editor labels "Anniversary".
///
/// vCard 3.0 has no standard property for a wedding day: RFC 6474's `ANNIVERSARY`
/// is vCard 4.0. In EDS 3.52, `e_contact_new_from_vcard()` reads
/// `X-EVOLUTION-ANNIVERSARY` (ignoring `ANNIVERSARY`), while in EDS 3.60+ it
/// reads standard `ANNIVERSARY`. We emit both lines so that EDS 3.52 reads its
/// vendor line and EDS 3.60+ reads standard ANNIVERSARY, and each preserves
/// the other as an unrecognised extension without requiring build-time version detection.
const X_EVOLUTION_ANNIVERSARY: &str = "X-EVOLUTION-ANNIVERSARY";

/// JSContact anniversary `kind` values and the vCard property stating each.
///
/// RFC 9553 §2.8.1's third kind, `death`, is missing on purpose: no vCard 3.0
/// property and no EDS field states it, and putting the date on a `BDAY`
/// would tell the user it is a birthday.
const ANNIVERSARY_KINDS: [(&str, &str); 2] =
    [("birth", "BDAY"), ("wedding", X_EVOLUTION_ANNIVERSARY)];

/// The line EDS keeps `E_CONTACT_SPOUSE` on — the field Evolution's contact
/// editor labels "Spouse".
///
/// vCard 3.0 has no property for a relation at all: RFC 6350 §6.6.6's `RELATED`
/// is vCard 4.0, which `e_contact_new_from_vcard()` is not given. So this is not
/// a shortcut past a standard line, it is the only line there is — and it is the
/// one EDS reads the field off and writes it back onto, measured against
/// libebook-contacts 3.52.
const X_EVOLUTION_SPOUSE: &str = "X-EVOLUTION-SPOUSE";

/// The line EDS keeps `E_CONTACT_MANAGER` on — the field Evolution's contact
/// editor labels "Manager".
const X_EVOLUTION_MANAGER: &str = "X-EVOLUTION-MANAGER";

/// The line EDS keeps `E_CONTACT_ASSISTANT` on — the field Evolution's contact
/// editor labels "Assistant".
const X_EVOLUTION_ASSISTANT: &str = "X-EVOLUTION-ASSISTANT";

/// RFC 9553 §2.1.8 relation types this mapping states on EDS relation lines.
const SPOUSE_RELATION: &str = "spouse";
const MANAGER_RELATION: &str = "manager";
const ASSISTANT_RELATION: &str = "assistant";

/// The line EDS keeps `E_CONTACT_BLOG_URL` on.
const X_EVOLUTION_BLOG_URL: &str = "X-EVOLUTION-BLOG-URL";

/// The line EDS keeps `E_CONTACT_VIDEO_URL` on.
const X_EVOLUTION_VIDEO_URL: &str = "X-EVOLUTION-VIDEO-URL";

/// JSContact calendar `kind` values and the vCard property stating each.
///
/// RFC 9555 §2.13.2 and §2.13.3 pair them: `calendar` with RFC 6350 §6.9.3's
/// `CALURI` and `freeBusy` with §6.9.1's `FBURL`. Both are vCard 4.0
/// properties, and both are nevertheless written on the 3.0 card EDS is handed
/// — because EDS reads them off one and writes them back onto one, measured
/// against libebook-contacts 3.52, which is what decides the question here.
///
/// EDS's third calendaring field, `ICSCALENDAR`, is missing on purpose: RFC
/// 9553 §2.4.1 has two kinds, so nothing on a card says an entry belongs there
/// rather than on the `CALURI` beside it.
const CALENDAR_KINDS: [(&str, &str); 2] = [("calendar", "CALURI"), ("freeBusy", "FBURL")];

/// RFC 9553 §2.2.4's default `kind` for a title that names none.
const DEFAULT_TITLE_KIND: &str = "title";

/// The kind of a JSContact title, with the default filled in.
///
/// The save path has to agree with this side about what an unsaid kind
/// means, or it will patch a `kind` onto every card that left it out.
pub fn title_kind(kind: Option<&str>) -> &str {
    kind.unwrap_or(DEFAULT_TITLE_KIND)
}

/// Whether the vCard mapping covers a JSContact title of this `kind`.
fn maps_title_kind(kind: Option<&str>) -> bool {
    TITLE_KINDS
        .iter()
        .any(|(mapped, _)| *mapped == title_kind(kind))
}

/// Whether the `N` value written for a name states this component.
///
/// Anything that saves a card back to the server has to know exactly which
/// JSContact fields a vCard can carry, or it will overwrite the ones it
/// silently dropped on the way in. The predicates below are that knowledge,
/// kept next to the tables they answer for.
///
/// Two things keep a component off the line, and the question is the same for
/// both: `N` has no field for its kind, or it says nothing to put in one.
/// `name_fields` leaves an empty component out exactly as it leaves an empty
/// entry off a card, so a save reading absence as a removal must ask about the
/// value too — the same "was this stated" [`states_context`] asks of a `TYPE`.
pub fn states_name_component(component: &NameComponent) -> bool {
    !component.value.is_empty() && name_field(&component.kind).is_some()
}

/// Whether a JSContact [`Name`] states any mapped property on a vCard (full name,
/// structured N components, or file-as string).
///
/// A name that states none of these has no vCard lines and is invisible to the
/// save path.
pub fn states_name(name: &Name) -> bool {
    name.full
        .as_deref()
        .filter(|full| !full.is_empty())
        .is_some()
        || derive_full(name).is_some()
        || name
            .components
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(states_name_component)
        || states_file_as(Some(name))
}

/// Whether a name states an Evolution file-as string.
///
/// Evolution's `E_CONTACT_FILE_AS` is written as `X-EVOLUTION-FILE-AS` on
/// vCard 3.0 lines and stored in `Name.extra["fileAs"]`.
pub fn states_file_as(name: Option<&Name>) -> bool {
    let Some(name) = name else {
        return false;
    };
    name.extra
        .get("fileAs")
        .or_else(|| name.extra.get("file_as"))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// The position in the vCard `N` value a JSContact name component kind is
/// written into, or `None` for a kind the value has no field for.
fn name_field(kind: &str) -> Option<usize> {
    NAME_COMPONENTS
        .iter()
        .find(|(mapped, _)| *mapped == kind)
        .map(|(_, index)| *index)
}

/// Whether the vCard mapping covers a JSContact `contexts` key.
pub fn maps_context(key: &str) -> bool {
    CONTEXTS.iter().any(|(mapped, _)| *mapped == key)
}

/// The one context `TYPE` a line carries, or `None` for an entry whose
/// contexts vCard 3.0 can spell none of.
///
/// **One and only one**, and that is the whole point of the function. EDS
/// picks the field a line lands in by matching the field's `TYPE` set against
/// the line's, so a single `ADR;TYPE=WORK,HOME` satisfies both
/// `E_CONTACT_ADDRESS_WORK` and `E_CONTACT_ADDRESS_HOME`, and one address shows
/// up in two of the blocks Evolution's contact editor lets the user edit
/// separately. There is only one line behind them: retyping the work address
/// rewrites it, and the home address the user never touched becomes the new
/// work one. Measured against libebook-contacts 3.52 — `eds-sys`'
/// `a_line_wearing_both_context_types_fills_two_slots_that_overwrite_each_other`
/// drives exactly that sequence — and the same holds for `TEL`, where
/// `E_CONTACT_PHONE_BUSINESS` wants `WORK`+`VOICE` and `E_CONTACT_PHONE_HOME`
/// wants `HOME`+`VOICE`, so a line carrying all three satisfies both.
///
/// So an entry at work *and* privately lands in [`DEFAULT_SLOT`], where the
/// user looks first — the choice [`service_slot`] already makes for a handle,
/// for the same reason. An entry with no context vCard can spell carries no
/// `TYPE` at all, which is a slot of its own: EDS reads such a line as
/// `E_CONTACT_ADDRESS_OTHER`.
///
/// A phone's `features` are narrowed the same way and for the same reason, by
/// [`feature_slot`]. What is *not* narrowed is an `EMAIL`, which EDS files by
/// position (`E_CONTACT_EMAIL_1` to `_4`) and not by `TYPE` at all.
///
/// The context left off the line is the save's problem, and
/// [`states_context`] is the answer.
fn context_slot(contexts: Option<&Value>) -> Option<&'static str> {
    let stated = type_names(&CONTEXTS, contexts);
    match stated.len() {
        0 => None,
        1 => Some(stated[0]),
        _ => Some(DEFAULT_SLOT),
    }
}

/// Whether the line written for an entry with these `contexts` states this
/// context.
///
/// The question the save path asks before reading a context's absence from an
/// edited line as the user having removed it. `context_slot` leaves one of two
/// contexts off the line, and a context the user was never shown is not one
/// they can have cleared — the same treatment [`maps_context`] already gives a
/// context vCard 3.0 has no `TYPE` for at all.
pub fn states_context(contexts: Option<&Value>, key: &str) -> bool {
    let Some(slot) = context_slot(contexts) else {
        return false;
    };
    CONTEXTS
        .iter()
        .any(|(mapped, name)| *mapped == key && *name == slot)
}

/// Whether the vCard mapping covers a JSContact phone `features` key.
pub fn maps_phone_feature(key: &str) -> bool {
    PHONE_FEATURES.iter().any(|(mapped, _)| *mapped == key)
}

/// The one feature `TYPE` a `TEL` carries, or `None` for a phone whose
/// features vCard 3.0 can spell none of.
///
/// [`context_slot`]'s problem one axis over: EDS picks the phone field by
/// matching `TYPE` there too, and a number that says it is both a voice line
/// and a fax says it to two fields the user edits separately —
/// `E_CONTACT_PHONE_BUSINESS` wants `WORK`+`VOICE`, `E_CONTACT_PHONE_BUSINESS_FAX`
/// wants `WORK`+`FAX`, and one `TEL;TYPE=WORK,VOICE,FAX` satisfies both, so
/// retyping the office number silently moves the office fax with it. Without a
/// context it is worse than duplication: the two unqualified fields
/// `E_CONTACT_PHONE_OTHER` (`VOICE`) and `E_CONTACT_PHONE_OTHER_FAX` (`FAX`)
/// are exclusive, and a bare `TEL;TYPE=VOICE,FAX` lands in *neither* — the
/// number is in no field of the contact editor at all. Measured against
/// libebook-contacts 3.52 by `eds-sys`'
/// `a_line_wearing_several_feature_types_reaches_two_fields_or_none` and
/// `editing_one_of_the_two_fields_a_multi_feature_line_fills_rewrites_the_other`.
///
/// **Which** feature survives is [`PHONE_FEATURES`]' order, and most of that
/// order is EDS's own, read off the pairs where EDS resolves the collision
/// itself: `VOICE,CELL` and `FAX,CELL` reach the mobile field alone, and
/// `VOICE,PAGER` and `FAX,PAGER` the pager alone, so a feature naming a device
/// outranks the two unqualified ones. `VIDEO` is last because this EDS knows
/// no such `TYPE`: alone it reaches nothing, so it can never be the slot while
/// another feature is available to be stated. `voice` sits just above it as
/// the unmarked default — a `TEL;TYPE=WORK` with no feature at all already
/// fills the *voice* field, so `voice` is the feature still said when it is
/// left off, and `fax` the one that would be lost.
///
/// Two orderings EDS does not decide and this mapping does: `mobile` over
/// `pager` (`CELL,PAGER` fills both fields, so there is nothing to read off),
/// and `fax` over `voice` (`VOICE,FAX` fills neither). Neither pick loses
/// anything from the *card* — the feature left off the line is kept by
/// [`states_phone_feature`] — only which of the contact editor's phone fields
/// the number appears in.
fn feature_slot(features: Option<&Value>) -> Option<&'static str> {
    type_names(&PHONE_FEATURES, features).into_iter().next()
}

/// Whether the line written for a phone with these `features` states this
/// feature.
///
/// [`states_context`] for features: a feature `feature_slot` left off the line
/// is not one the user can have cleared by not typing it back.
pub fn states_phone_feature(features: Option<&Value>, key: &str) -> bool {
    let Some(slot) = feature_slot(features) else {
        return false;
    };
    PHONE_FEATURES
        .iter()
        .any(|(mapped, name)| *mapped == key && *name == slot)
}

/// Whether the `ADR` line written for an address states this component —
/// [`states_name_component`]'s question one property over, and answered the
/// same way: the seven fields must have room for its kind, and it must say
/// something to put there. See `address_fields`, which skips both.
pub fn states_address_component(component: &AddressComponent) -> bool {
    !component.value.is_empty() && address_field(&component.kind).is_some()
}

/// The `ADR` field a component of this kind is stated in, whether it has one
/// to itself or shares it with another kind.
fn address_field(kind: &str) -> Option<usize> {
    if let Some((_, index)) = ADDRESS_COMPONENTS
        .iter()
        .find(|(mapped, _)| *mapped == kind)
    {
        return Some(*index);
    }
    let (_, onto) = JOINED_COMPONENTS
        .iter()
        .find(|(mapped, _)| *mapped == kind)?;
    ADDRESS_COMPONENTS
        .iter()
        .find(|(mapped, _)| mapped == onto)
        .map(|(_, index)| *index)
}

/// Whether an address reaches the user at all — whether it has anything an
/// `ADR` or a `LABEL` line can state.
///
/// This is the emitter's own decision, asked of it by name, so that the save
/// path cannot drift from what [`card_to_vcard`] actually wrote. Every keyed
/// map the mapping carries has one of these, because every one of them has
/// entries a vCard leaves out; a save that decided for itself which those
/// were would eventually decide differently, and delete an entry the user
/// never saw.
pub fn states_address(address: &Address) -> bool {
    address_fields(address).is_some() || address_label(address).is_some()
}

/// The text a `LABEL` line states for an address, or `None` for one written
/// out as nothing — which says no more than an `EMAIL:` with no address does,
/// and gets no line either.
pub fn address_label(address: &Address) -> Option<&str> {
    address.full.as_deref().filter(|full| !full.is_empty())
}

/// Whether a note reaches the user at all — whether it says anything a
/// `NOTE` line could state.
pub fn states_note(note: &Note) -> bool {
    !note.note.is_empty()
}

/// Whether a nickname reaches the user at all — whether it names anything a
/// `NICKNAME` line could state.
pub fn states_nickname(nickname: &Nickname) -> bool {
    !nickname.name.is_empty()
}

/// Whether a link reaches the user at all: it must point somewhere *and* be
/// of a kind vCard 3.0 or EDS has a property for.
///
/// As with a title, the kind alone is not the question — a plain link with no
/// URI has no line either, and calling it visible would let a save
/// delete it.
pub fn states_link(link: &Link) -> bool {
    !link.uri.is_empty()
        && !edged_with_whitespace(&link.uri)
        && !link.uri.contains('\r')
        && maps_link_kind(link.kind.as_deref())
}

/// Whether the vCard mapping covers a JSContact link of this `kind`.
///
/// - `None`: plain website, mapped to RFC 2426 §3.6.8 `URL` (Evolution Homepage).
/// - `Some("blog")`: blog URL, mapped to `X-EVOLUTION-BLOG-URL` (Evolution Blog URL).
/// - `Some("video")`: video stream URL, mapped to `X-EVOLUTION-VIDEO-URL` (Evolution Video URL).
///
/// RFC 9553 §2.6.3 defines `contact` (a URI for writing to the person), which RFC 9555
/// §2.6.3 states on vCard 4.0's `CONTACT-URI`. Other kinds outside this set get no line
/// and remain safe on the server.
fn maps_link_kind(kind: Option<&str>) -> bool {
    matches!(kind, None | Some("blog") | Some("video"))
}

/// Whether a calendar reaches the user at all: it must point somewhere *and*
/// be of a kind one of the two lines states.
///
/// As with a link, the kind alone is not the question — a calendar with no URI
/// has no line either, and calling it visible would let a save delete it.
pub fn states_calendar(calendar: &Calendar) -> bool {
    !calendar.uri.is_empty() && calendar_property(calendar.kind.as_deref()).is_some()
}

/// The vCard property a calendar of this `kind` is stated on.
///
/// RFC 9553 §2.4.1 makes the kind mandatory and gives it no default, so an
/// entry naming none says nothing about which of the two lines its URI belongs
/// on — and the two are different fields in front of the user. A kind outside
/// the table, vendor kinds included, is the same case: the mapping states the
/// kinds it knows and leaves the rest to the server.
fn calendar_property(kind: Option<&str>) -> Option<&'static str> {
    let kind = kind?;
    CALENDAR_KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, property)| *property)
}

/// The calendar `kind` a line of this name states, the inverse of
/// [`calendar_property`].
fn calendar_kind(property: &str) -> Option<&'static str> {
    CALENDAR_KINDS
        .iter()
        .find(|(_, name)| *name == property)
        .map(|(kind, _)| *kind)
}

/// Whether a relation states the given relation type.
fn has_relation_type(relation: &Relation, expected: &str) -> bool {
    relation
        .relation
        .as_ref()
        .and_then(|types| types.get(expected))
        == Some(&Value::Bool(true))
}

/// Whether a related entity reaches the user as a spouse, `key` being the
/// entity's name.
pub fn states_spouse(key: &str, relation: &Relation) -> bool {
    has_relation_type(relation, SPOUSE_RELATION) && names_a_person(key)
}

/// Whether a related entity reaches the user as a manager, `key` being the
/// entity's name.
pub fn states_manager(key: &str, relation: &Relation) -> bool {
    has_relation_type(relation, MANAGER_RELATION) && names_a_person(key)
}

/// Whether a related entity reaches the user as an assistant, `key` being the
/// entity's name.
pub fn states_assistant(key: &str, relation: &Relation) -> bool {
    has_relation_type(relation, ASSISTANT_RELATION) && names_a_person(key)
}

/// Whether the marriage is all a relation says, so that withdrawing it leaves
/// nothing of the entry to keep.
///
/// The question a save has to answer about the name the user just replaced. The
/// line stated one thing about that entity — that it is a spouse — so that is the
/// only thing the save may withdraw: an entry the server also calls `kin` stays,
/// with the marriage struck off, and an entry that said nothing else goes
/// altogether rather than lingering as a relation of no stated type, which is
/// not what emptying the field said. A member this version has never heard of
/// counts as something else said, for the reason every property here patches
/// rather than replaces; the `@type` tag does not, because naming the object's
/// type says nothing about the relation.
pub fn states_nothing_but_the_marriage(relation: &Relation) -> bool {
    relation.extra.keys().all(|member| member == "@type")
        && relation
            .relation
            .iter()
            .flatten()
            .all(|(kind, _)| kind == SPOUSE_RELATION)
}

/// Whether a `relatedTo` key is a name a line can show the user and a save can
/// read back out unchanged.
fn names_a_person(key: &str) -> bool {
    !key.is_empty() && !names_a_uri(key) && !edged_with_whitespace(key) && !key.contains('\r')
}

/// Whether a value is a URI rather than free text, by RFC 3986 §3.1's grammar
/// for the scheme: an ASCII letter, then letters, digits, `+`, `-` and `.`, then
/// the colon.
///
/// Checking the grammar rather than looking for a colon is what keeps a name
/// from being mistaken for an identifier: `Jean Paul: the second` holds a colon
/// and no scheme.
fn names_a_uri(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Whether a media entry reaches the user at all: it must be a photo, and the
/// bytes or the URI it names must be something a `PHOTO` line can state.
///
/// `photo` is where each of those is spelled out — the single point this and
/// [`card_to_vcard`] agree through, so an entry cannot be called visible here
/// and then left off the vCard.
pub fn states_media(media: &Media) -> bool {
    photo(media).is_some()
}

/// What a `PHOTO` line states for one media entry, or `None` for an entry that
/// gets no line.
///
/// An entry has no line when:
/// - it is not a photo. RFC 9553 §2.6.4 keeps all three kinds of media in one
///   map, and RFC 2426 §3.1.4's `PHOTO` is the picture *of the contact*; a
///   `logo`, a `sound` or a vendor kind on that line would show the user the
///   wrong image. An entry naming no kind at all is malformed — §2.6.4 makes
///   `kind` mandatory — and is treated the same way rather than guessed at.
/// - it points nowhere, which says no more than an `EMAIL:` with no address.
/// - it carries its bytes as a `data:` URI (RFC 2397) that does not spell them
///   as base64, or spells them as base64 this side cannot decode. `ENCODING=b`
///   is the only encoding an EDS-bound vCard 3.0 `PHOTO` carries, and handing
///   EDS a value it would decode into a broken image is worse than showing the
///   user no picture: the save cannot repair what it never sends.
fn photo(media: &Media) -> Option<Photo<'_>> {
    if media.kind.as_deref() != Some(PHOTO_KIND) || media.uri.is_empty() {
        return None;
    }
    let Some(rest) = strip_prefix_ci(&media.uri, DATA_SCHEME) else {
        return Some(Photo::Uri(&media.uri));
    };
    let (metadata, payload) = rest.split_once(',')?;
    let stated = strip_suffix_ci(metadata, BASE64_MARKER)?;
    Some(Photo::Inline {
        subtype: image_subtype(media.media_type.as_deref().unwrap_or(stated)),
        base64: BASE64.encode(decoded(payload)?),
    })
}

/// Whether two media entries state the same `PHOTO` line — the same picture,
/// however each of them spells it.
///
/// What the save compares, for the reason [`online_service_handle`] is what it
/// compares for a handle: the line is what the user saw, and the entry read back
/// off it is not the entry that produced it. A `data:` URI may leave its base64
/// padding off (RFC 4648 §3.2) where the line carries the canonical spelling,
/// and a media type the URI stated arrives as the entry's own `mediaType`, so
/// comparing the members would call an untouched picture an edit and rewrite it
/// on every save. The subtypes are compared case-insensitively, as RFC 2045 §5.1
/// defines them.
pub fn same_photo(one: &Media, other: &Media) -> bool {
    match (photo(one), photo(other)) {
        (None, None) => true,
        (Some(Photo::Uri(one)), Some(Photo::Uri(other))) => one == other,
        (
            Some(Photo::Inline {
                subtype: one,
                base64: ours,
            }),
            Some(Photo::Inline {
                subtype: other,
                base64: theirs,
            }),
        ) => {
            ours == theirs
                && one
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(other.unwrap_or_default())
        }
        _ => false,
    }
}

/// What a `PHOTO` line states: the bytes, or where to fetch them.
enum Photo<'a> {
    /// The picture itself, base64 as `ENCODING=b` wants it, and the subtype
    /// [`image_subtype`] read off its media type.
    Inline {
        subtype: Option<&'a str>,
        base64: String,
    },
    /// A URI to fetch the picture from, for a card that only points at one.
    Uri(&'a str),
}

/// The subtype of an `image/*` media type, which is all a `PHOTO` line's `TYPE`
/// may state, or `None` for a media type the parameter cannot say.
///
/// EDS builds the photo's mime type by putting `image/` in front of the `TYPE`:
/// measured against libebook-contacts 3.52, `TYPE=JPEG` arrives as `image/JPEG`
/// and `TYPE=image/jpeg` as `image/image/jpeg`, which names no format at all.
/// So the parameter states the subtype alone — and for bytes whose media type is
/// not an image, nothing: EDS accepts a line with no `TYPE` and reports no mime
/// type for it, which is honest, where `TYPE=pdf` would tell the address book the
/// bytes are an image format that does not exist.
fn image_subtype(media_type: &str) -> Option<&str> {
    // The media type's own parameters (`;charset=…`) are no part of its name.
    let name = media_type.split(';').next().unwrap_or_default().trim();
    strip_prefix_ci(name, IMAGE_PREFIX).filter(|subtype| !subtype.is_empty())
}

/// A base64 payload as its bytes: the standard alphabet, padded or not.
///
/// The bytes are decoded here and re-encoded by [`photo`] rather than copied
/// across from the URI, so that the line carries the canonical spelling of what
/// the URI meant. A `data:` URI is written by hand as often as by a library, and
/// it is glib's base64 reader that decodes the line at the other end, not the one
/// that wrote the URI.
fn decoded(payload: &str) -> Option<Vec<u8>> {
    BASE64
        .decode(payload)
        .or_else(|_| BASE64_UNPADDED.decode(payload))
        .ok()
}

/// `value` without `prefix`, comparing it case-insensitively — for a URI scheme
/// and a media type, both of which RFC 3986 §3.1 and RFC 2045 §5.1 define as
/// case-insensitive.
fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let start = value.get(..prefix.len())?;
    start
        .eq_ignore_ascii_case(prefix)
        .then(|| &value[prefix.len()..])
}

/// `value` without `suffix`, compared as [`strip_prefix_ci`] compares.
fn strip_suffix_ci<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let split = value.len().checked_sub(suffix.len())?;
    let end = value.get(split..)?;
    end.eq_ignore_ascii_case(suffix).then(|| &value[..split])
}

/// Whether a handle at an online service reaches the user at all.
///
/// Three things have to hold at once, and `drawn_service` is where each is
/// spelled out: the service must be one EDS has a field for, the entry must
/// state a handle — as a `user`, or as a `uri` this side can read one out of —
/// and that handle must survive the trip through EDS unrenamed.
pub fn states_online_service(service: &OnlineService) -> bool {
    drawn_service(service).is_some()
}

/// The vCard property a handle is stated on and the handle itself, or `None` for
/// an entry with no line — the single point [`states_online_service`] and
/// [`card_to_vcard`] agree through, so an entry cannot be called visible and
/// then left out.
///
/// An entry has no line when:
///
/// - **its service is not one EDS has a field for.** The line *is* the service:
///   there is no property that states an arbitrary one, and an invented
///   `X-SIGNAL` would reach nothing the user can see while making the save
///   believe the entry had been shown. See [`ONLINE_SERVICES`].
/// - **it states no handle.** RFC 9553 §2.3.2 asks for a `user` or a `uri`, and
///   only the first is a handle, which is what the EDS field holds. An entry
///   stating only a URI is drawn when [`SERVICE_SCHEMES`] says what its scheme
///   means and the URI states the handle and nothing besides — see
///   [`online_service_handle`] — and left invisible otherwise, because the
///   alternative is guessing at a handle and then writing the guess back.
/// - **its handle would come back from EDS spelled differently.** The empty
///   handle says nothing; a carriage return is dropped by [`card_to_vcard`]
///   as a security property; and ends made of ASCII whitespace are trimmed by
///   EDS — measured against libebook-contacts 3.52, where `X-JABBER: vera@a `
///   reaches the user as `vera@a`. Each would have the next save rename the
///   handle on the server, which costs more than not showing it. See
///   [`edged_with_whitespace`].
fn drawn_service(service: &OnlineService) -> Option<(&'static str, &str)> {
    let property = service_property(service.service.as_deref()?)?;
    let handle = online_service_handle(service)?;
    let drawable = !handle.is_empty() && !handle.contains('\r') && !edged_with_whitespace(handle);
    drawable.then_some((property, handle))
}

/// The handle a line states for an entry: its `user`, or — for an entry that
/// names none — the one its `uri` spells out.
///
/// The `user` wins where both are there: it is what the service calls the
/// contact, while the URI is a second way of saying the same thing, and the
/// field the line reaches holds the first.
///
/// The save path compares *this* rather than the `user` on either side, because
/// the vCard states only the handle: an entry that arrived as a URI comes back
/// as a `user` saying the same thing, and calling that an edit would rewrite the
/// entry every time the contact is touched.
pub fn online_service_handle(service: &OnlineService) -> Option<&str> {
    match service.user.as_deref() {
        Some(user) => Some(user),
        None => handle_in_uri(service.service.as_deref()?, service.uri.as_deref()?),
    }
}

/// The handle inside a service's URI, for a URI that states one and nothing
/// else.
fn handle_in_uri<'a>(service: &str, uri: &'a str) -> Option<&'a str> {
    let wanted = normalised_service(service);
    let (stated, handle) = uri.split_once(':')?;
    // Case-insensitively, as RFC 3986 §3.1 requires of a scheme.
    let matches = SERVICE_SCHEMES.iter().any(|(name, scheme)| {
        normalised_service(name) == wanted && stated.eq_ignore_ascii_case(scheme)
    });
    (matches && plain_handle(handle)).then_some(handle)
}

/// The URI a service would state a handle under, or `None` when there is no
/// scheme for the service or no URI that would say just this handle.
///
/// The save path's other half: the entry the server stated as a URI is patched
/// as one, rather than answered with a `user` it never had. `None` is not a
/// failure — it means the rename has to change the entry's shape, which is
/// always allowed and never wrong, only less faithful.
pub fn online_service_uri(service: &str, handle: &str) -> Option<String> {
    let scheme = service_scheme(service)?;
    plain_handle(handle).then(|| format!("{scheme}:{handle}"))
}

/// Whether a URI's scheme-specific part is a bare handle.
///
/// A path, a query, a fragment or a percent-encoding means the URI says more
/// than who the contact is — `skype:echo123?call` names an action, and
/// `xmpp:vera%40jabber.example` states a handle this side would have to decode
/// and the next save would have to re-encode. Whitespace is out for the reason
/// it is out of a `user`: a URI cannot hold it, so a handle carrying one has no
/// URI to go back into.
///
/// Used in both directions, so that a handle read out of a URI is exactly a
/// handle that can be written back into one.
fn plain_handle(handle: &str) -> bool {
    !handle.is_empty()
        && !handle.contains(['/', '?', '#', '%'])
        && !handle
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

/// The URI scheme handles at this service are stated under.
fn service_scheme(service: &str) -> Option<&'static str> {
    let wanted = normalised_service(service);
    SERVICE_SCHEMES
        .iter()
        .find(|(name, _)| normalised_service(name) == wanted)
        .map(|(_, scheme)| *scheme)
}

/// The vCard property handles at this service are stated on.
fn service_property(service: &str) -> Option<&'static str> {
    let wanted = normalised_service(service);
    ONLINE_SERVICES
        .iter()
        .find(|(name, _)| normalised_service(name) == wanted)
        .map(|(_, property)| *property)
}

/// The service a vCard property states handles for, spelled as
/// [`ONLINE_SERVICES`] spells it — which is what a handle the user has just
/// typed is filed under, since the line EDS wrote names no service but itself.
fn service_of(property: &str) -> Option<&'static str> {
    ONLINE_SERVICES
        .iter()
        .find(|(_, mapped)| *mapped == property)
        .map(|(name, _)| *name)
}

/// Whether two service names name the same service.
///
/// RFC 9553 §2.3.2 requires case-insensitive equality; this is wider, ignoring
/// the punctuation and spacing inside the name as well, because `Gadu-Gadu`,
/// `GaduGadu` and `gadu gadu` are one service under three spellings. Being wide
/// here is the safe direction: the mapping uses this to decide *not* to write,
/// so a match the RFC does not demand leaves the server's own spelling alone,
/// while a miss would rename it.
pub fn same_service(one: Option<&str>, other: Option<&str>) -> bool {
    match (one, other) {
        (Some(one), Some(other)) => normalised_service(one) == normalised_service(other),
        (None, None) => true,
        _ => false,
    }
}

/// A service name with everything that is not a letter or a digit removed, and
/// the rest lower-cased.
fn normalised_service(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The slot a handle goes in: the `TYPE` value EDS reads to decide which of
/// Evolution's per-context fields shows it.
///
/// Chosen from the entry's `contexts`, which is as far as that member crosses:
/// nothing is read back off the parameter, because every line has to carry one
/// to be visible at all and reading it would put a context on every entry that
/// stated none. Exactly one slot per line — a handle wearing both `TYPE`s shows
/// up in two fields the user can edit independently, and nothing would say which
/// edit wins — so a service used at work *and* privately, or in a context vCard
/// 3.0 cannot spell, lands in [`DEFAULT_SLOT`], where the user looks first.
fn service_slot(service: &OnlineService) -> &'static str {
    let context = |name: &str| {
        service
            .contexts
            .as_ref()
            .or_else(|| service.extra.get("contexts"))
            .and_then(|contexts| contexts.get(name))
            == Some(&Value::Bool(true))
    };
    match context("work") && !context("private") {
        true => "WORK",
        false => DEFAULT_SLOT,
    }
}

/// Whether one tag of `keywords` goes on the `CATEGORIES` line — the
/// `states_*` predicate of the one mapped property that is a *set* rather than
/// a keyed map of objects.
///
/// Being a set changes what the save does with the answer, not what the
/// question means. A `CATEGORIES` line holds the whole set and a JSContact
/// keyword is a bare string, so there is nothing inside an entry to preserve
/// and no key to patch by: the line states what was shown, and the save writes
/// back a whole new set. A tag this predicate refuses was therefore not merely
/// unseen but *absent from what the user edited*, and the save has to put it
/// back by hand rather than read its absence as a deletion. What is refused:
///
/// - **A value that is not `true`.** RFC 9553 §1.4.3 has every value of a Set be
///   `true`; drawing anything else would say the tag is set where the server
///   said it is not.
/// - **An empty tag.** An empty item between two commas reads back as no tag at
///   all, so the tag would vanish between the drawing and the save.
/// - **A tag holding a carriage return.** [`crate::syntax::write`] drops it —
///   that is a security property, not tidiness — so the tag would come back
///   spelled differently and a save would rename it. A line feed is not this
///   case: it has an escape and survives both writers.
/// - **A tag whose ends are whitespace.** EDS trims them: measured against
///   libebook-contacts 3.52, `CATEGORIES: quiet` reaches the user as `quiet`,
///   and the next save would rename the tag on the server. See
///   `edged_with_whitespace`.
///
/// The single point the save and `drawn_tags` agree through, so a tag cannot
/// be called shown and then left off the line.
pub fn states_keyword(tag: &str, set: &Value) -> bool {
    set == &Value::Bool(true)
        && !tag.is_empty()
        && !tag.contains('\r')
        && !edged_with_whitespace(tag)
}

/// Whether a tag begins or ends with a character EDS would trim off it.
///
/// The set is ASCII whitespace, which is one character wider than what EDS was
/// measured to strip — it keeps a vertical tab — because the two errors are not
/// the same size. Refusing to draw a tag costs the sight of it; drawing one that
/// comes back trimmed costs the tag, on the server, without anybody having
/// asked.
fn edged_with_whitespace(tag: &str) -> bool {
    let whitespace = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];
    tag.starts_with(whitespace) || tag.ends_with(whitespace)
}

/// The tags to write on the `CATEGORIES` line, in the order the set holds them —
/// which is sorted, so the vCard is stable across renderings; a reordering would
/// otherwise look to the save like an edit.
fn drawn_tags(card: &ContactCard) -> Vec<&str> {
    card.keywords
        .iter()
        .flatten()
        .filter(|(tag, set)| states_keyword(tag, set))
        .map(|(tag, _)| tag.as_str())
        .collect()
}

/// The tags the card is filed under, as a JSContact `keywords` Set.
///
/// Every `CATEGORIES` line is read, not just the first, and that is not
/// pedantry about RFC 2426 §3.7.1 admitting the property more than once: EDS
/// shows the user the first line only and, when the Categories field is edited,
/// rewrites that one and leaves the second exactly as it was — measured against
/// libebook-contacts 3.52. Reading both is what keeps the tags on the second
/// from being deleted by the next save, having never been shown.
///
/// A tag named twice — across the lines or within one — is one member, because a
/// set is what both sides mean. An empty item is dropped rather than carried as
/// the tag whose name is nothing: `CATEGORIES:` and `CATEGORIES:a,,b` state
/// nothing between their separators.
///
/// `None` rather than an empty map for a card with no tags, for the reason the
/// keyed maps have one: the save reads an edit off a difference from what was
/// shown, and an empty set would claim the contact is untagged where the vCard
/// made no claim at all.
fn read_keywords(entries: &[VCardEntry]) -> Option<BTreeMap<String, Value>> {
    let tags: BTreeMap<String, Value> = entries
        .iter()
        .filter(|entry| entry.name.as_str().eq_ignore_ascii_case(CATEGORIES))
        .flat_map(entry_items)
        .filter(|tag| !tag.is_empty())
        .map(|tag| (tag, Value::Bool(true)))
        .collect();
    (!tags.is_empty()).then_some(tags)
}

/// Whether an email address reaches the user at all. An entry with no
/// address states nothing, so it gets no `EMAIL` line.
pub fn states_email(email: &ContactEmail) -> bool {
    !email.address.is_empty()
}

/// Whether a phone number reaches the user at all.
pub fn states_phone(phone: &ContactPhone) -> bool {
    !phone.number.is_empty()
}

/// Whether a title reaches the user at all: the mapping must have a property
/// for its `kind` *and* the entry must name something.
///
/// The kind alone is not the question. A title of kind `title` that names
/// nothing has no `TITLE` line either, and asking only `maps_title_kind`
/// would call it visible and let a save delete it.
pub fn states_title(title: &Title) -> bool {
    !title.name.is_empty() && maps_title_kind(title.kind.as_deref())
}

/// Whether an organisation reaches the user at all — whether the `ORG` line
/// has a name or a unit to state. An entry holding only a `sortAs` has
/// neither.
pub fn states_organization(organization: &Organization) -> bool {
    organization_components(organization).is_some()
}

/// Whether the `ORG` line written for an organisation states this unit.
///
/// [`states_name_component`]' question one property over, and the reason a
/// save has to ask it is the same: `organization_components` leaves out a
/// unit that names nothing, so reading its absence from the edited line as a
/// removal deletes a unit the user was never shown. Unlike a component there
/// is no second half to the question — every unit that *has* a name is
/// written, however many of them there are, because `ORG` takes as many
/// components as the entry has units.
pub fn states_org_unit(unit: &OrgUnit) -> bool {
    !unit.name.is_empty()
}

/// Whether an anniversary reaches the user at all: the mapping must have a
/// property for its `kind` *and* its date must name one calendar day.
pub fn states_anniversary(anniversary: &Anniversary) -> bool {
    anniversary_property(&anniversary.kind).is_some() && anniversary_date(anniversary).is_some()
}

/// The date a vCard line states for an anniversary, or `None` for a date no
/// single day can be read out of.
///
/// This is what the save compares by, rather than the JSON: the two shapes
/// RFC 9553 §2.8.1 allows can name the same day, so a card whose birthday is
/// a `Timestamp` must not look edited merely because it came back as the day
/// the user was shown.
pub fn anniversary_date(anniversary: &Anniversary) -> Option<String> {
    let date = anniversary.date.as_ref()?;
    // A `Timestamp` states a point in time. The day it falls on is read in
    // UTC, which is the only zone the card names.
    let day = if let Some(utc) = date.get("utc").and_then(Value::as_str) {
        read_day(utc)?
    } else {
        let day = Day {
            year: member(date, "year")?,
            month: member(date, "month")?,
            day: member(date, "day")?,
        };
        day.is_a_date().then_some(day)?
    };
    day.survives_the_field_it_lands_in().then(|| day.text())
}

/// Whether an anniversary is dated by a point in time (RFC 9553 §2.8.1's
/// `Timestamp`) rather than by a calendar day (its `PartialDate`).
///
/// The save asks because the two are patched differently: a day's members can
/// be reached into one at a time, leaving whatever else the object carries in
/// place, while a point in time the user has retyped as a day is a different
/// kind of object and has to be written whole.
pub fn states_a_point_in_time(anniversary: &Anniversary) -> bool {
    anniversary
        .date
        .as_ref()
        .is_some_and(|date| date.get("utc").is_some())
}

/// The vCard property an anniversary of this `kind` is stated on.
fn anniversary_property(kind: &str) -> Option<&'static str> {
    ANNIVERSARY_KINDS
        .iter()
        .find(|(mapped, _)| *mapped == kind)
        .map(|(_, name)| *name)
}

/// The earliest year `e_contact_date_to_string()` will write, measured against
/// libebook-contacts 3.52. See [`Day::survives_the_field_it_lands_in`].
const EARLIEST_YEAR: u32 = 1000;

/// One calendar day: the whole of what a vCard 3.0 date line can state.
struct Day {
    year: u32,
    month: u32,
    day: u32,
}

impl Day {
    /// The day as RFC 2426 §3.1.5 asks for it — ISO 8601's extended form,
    /// which is also the one `e_contact_date_to_string()` writes back.
    fn text(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Whether the numbers name a day of a kind the calendar has. Which
    /// months are 30 days long is left to the server that stated the date;
    /// what is refused here is a date no month could have.
    fn is_a_date(&self) -> bool {
        (1..=9999).contains(&self.year)
            && (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
    }

    /// Whether EDS will hand this day back the day it is, so that a line
    /// stating it can be believed when it comes home.
    ///
    /// `e_contact_date_to_string()` CLAMPs each part into the range it can
    /// print — the year to [`EARLIEST_YEAR`]`..=9999`, the month to `1..=12`,
    /// the day to `1..=31`. Reading is not clamped, so a line whose year is
    /// under a thousand parses back correctly and is *written* back as the
    /// year 1000: `eds-sys/tests/contacts.rs` measures both halves, up to the
    /// `BDAY:0800-06-21` that becomes `BDAY:1000-06-21` the moment the field
    /// is set.
    ///
    /// The month and the day need no check of their own — [`Self::is_a_date`]
    /// already refuses everything outside the ranges the clamp keeps — so this
    /// is the year alone. A day it refuses is stated on no line, which leaves
    /// it invisible to the user but leaves the server's date alone: an
    /// anniversary no line states is one `diff_entries` will not patch.
    fn survives_the_field_it_lands_in(&self) -> bool {
        self.year >= EARLIEST_YEAR
    }

    /// The day as the `PartialDate` a save writes when the user retyped one.
    fn json(&self) -> Value {
        json!({
            "@type": "PartialDate",
            "year": self.year,
            "month": self.month,
            "day": self.day,
        })
    }
}

/// The day a date line states, or `None` for text that names none.
///
/// Both ISO 8601 forms are read — `1964-03-27` and `19640327` — because
/// `e_contact_date_from_string()` reads both, and so a vCard that has been
/// through another client may carry either. A time after the date is dropped
/// rather than refused, for the same reason.
fn read_day(text: &str) -> Option<Day> {
    let digits: String = text
        .split(['T', 't'])
        .next()?
        .chars()
        .filter(|character| *character != '-')
        .collect();
    if digits.len() != 8 || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let day = Day {
        year: digits[0..4].parse().ok()?,
        month: digits[4..6].parse().ok()?,
        day: digits[6..8].parse().ok()?,
    };
    day.is_a_date().then_some(day)
}

/// One numeric member of a JSContact date object.
fn member(date: &Value, name: &str) -> Option<u32> {
    date.get(name)?.as_u64()?.try_into().ok()
}

/// Unwraps Apple-style `_$!<LabelName>!$_` markers into clean label names, or
/// returns the trimmed input text if not wrapped in markers.
fn clean_apple_label(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed
        .strip_prefix("_$!<")
        .and_then(|s| s.strip_suffix(">!$_"))
    {
        inner.trim()
    } else {
        trimmed
    }
}

/// Render a contact card as a vCard 3.0 string, ready for
/// `e_contact_new_from_vcard()`.
pub fn card_to_vcard(card: &ContactCard) -> String {
    let mut entries = Vec::new();

    // EDS keys its cache on the vCard UID and passes it back to
    // load_contact_sync()/remove_contact_sync(), so it has to be the
    // identifier the JMAP methods take — the server-assigned id. The
    // JSContact uid, which is a different namespace, rides alongside.
    if let Some(uid) = card
        .id
        .as_ref()
        .map(|id| id.as_str())
        .or(card.uid.as_deref())
    {
        entries.push(VCardEntry::new(VCardProperty::Uid).with_value(uid.to_owned()));
    }
    if let Some(uid) = &card.uid {
        entries.push(
            VCardEntry::new(VCardProperty::Other(X_JMAP_UID.to_owned())).with_value(uid.clone()),
        );
    }

    if let Some(name) = &card.name {
        // An empty stated `full` reads back as absent (`read_name` below filters
        // an empty FN to `None`, the same way it treats no FN at all), so it must
        // be treated as absent here too — otherwise the first round trip keeps a
        // literal empty FN while every later one derives and keeps a non-empty
        // one, and the two never reach the same fixed point.
        let full = name
            .full
            .as_deref()
            .filter(|full| !full.is_empty())
            .map(str::to_owned)
            .or_else(|| derive_full(name));
        if let Some(full) = full {
            entries.push(VCardEntry::new(VCardProperty::Fn).with_value(full));
        }
        if let Some(fields) = name_fields(name) {
            entries.push(
                VCardEntry::new(VCardProperty::N)
                    .with_values(fields.into_iter().map(VCardValue::Text).collect()),
            );
        }
    }

    if let Some(file_as) = card
        .name
        .as_ref()
        .and_then(|n| n.extra.get("fileAs").or_else(|| n.extra.get("file_as")))
        .or_else(|| card.extra.get("fileAs"))
        .or_else(|| card.extra.get("file_as"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        entries.push(
            VCardEntry::new(VCardProperty::Other("X-EVOLUTION-FILE-AS".to_owned()))
                .with_value(file_as.to_owned()),
        );
    }

    // One line per entry rather than RFC 2426 §3.1.3's comma-separated list,
    // so that each keeps its JSContact key — and because EDS reads the value
    // as one string either way.
    for (key, nickname) in card.nicknames.iter().flatten() {
        if !states_nickname(nickname) {
            continue;
        }
        entries.push(
            VCardEntry::new(VCardProperty::Nickname)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_value(nickname.name.clone()),
        );
    }

    let mut emails: Vec<_> = card.emails.iter().flatten().collect();
    emails.sort_by_key(|(key, email)| (email.pref.unwrap_or(u32::MAX), *key));
    for (key, email) in emails {
        if !states_email(email) {
            continue;
        }
        let mut params = vec![VCardParameter::new(
            VCardParameterName::Other(X_JMAP_KEY.to_owned()),
            VCardParameterValue::Text(key.clone()),
        )];
        for type_name in type_names(&CONTEXTS, email.contexts.as_ref()) {
            params.push(VCardParameter::typ(VCardParameterValue::Text(
                type_name.to_owned(),
            )));
        }
        if email.pref.is_some() {
            // vCard 3.0 has no ranking, only a preferred flag.
            params.push(VCardParameter::typ(VCardParameterValue::Text(
                "PREF".to_owned(),
            )));
        }
        entries.push(
            VCardEntry::new(VCardProperty::Email)
                .with_params(params)
                .with_value(email.address.clone()),
        );
    }

    let mut phones: Vec<_> = card.phones.iter().flatten().collect();
    phones.sort_by_key(|(key, phone)| (phone.pref.unwrap_or(u32::MAX), *key));
    for (key, phone) in phones {
        if !states_phone(phone) {
            continue;
        }
        let mut params = vec![VCardParameter::new(
            VCardParameterName::Other(X_JMAP_KEY.to_owned()),
            VCardParameterValue::Text(key.clone()),
        )];
        if let Some(type_name) = context_slot(phone.contexts.as_ref()) {
            params.push(VCardParameter::typ(VCardParameterValue::Text(
                type_name.to_owned(),
            )));
        }
        if let Some(type_name) = feature_slot(phone.features.as_ref()) {
            params.push(VCardParameter::typ(VCardParameterValue::Text(
                type_name.to_owned(),
            )));
        }
        if phone.pref.is_some() {
            params.push(VCardParameter::typ(VCardParameterValue::Text(
                "PREF".to_owned(),
            )));
        }
        entries.push(
            VCardEntry::new(VCardProperty::Tel)
                .with_params(params)
                .with_value(phone.number.clone()),
        );
    }

    let mut addresses: Vec<_> = card.addresses.iter().flatten().collect();
    addresses.sort_by_key(|(key, address)| (address_pref(address).unwrap_or(u32::MAX), *key));
    for (key, address) in addresses {
        let mut type_params = Vec::new();
        if let Some(slot) = context_slot(address.contexts.as_ref()) {
            type_params.push(VCardParameter::typ(VCardParameterValue::Text(
                slot.to_owned(),
            )));
        }
        if address_pref(address).is_some() {
            type_params.push(VCardParameter::typ(VCardParameterValue::Text(
                "PREF".to_owned(),
            )));
        }
        if let Some(fields) = address_fields(address) {
            let mut params = vec![VCardParameter::new(
                VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                VCardParameterValue::Text(key.clone()),
            )];
            params.extend(type_params.clone());
            entries.push(
                VCardEntry::new(VCardProperty::Adr)
                    .with_params(params)
                    .with_values(fields.into_iter().map(VCardValue::Text).collect()),
            );
        }
        // The same address written out for an envelope, on the line RFC 2426
        // §3.2.2 gives it — directly after its own `ADR`, and on its own when
        // the components are not known and there is no `ADR` to follow.
        if let Some(full) = address_label(address) {
            let mut params = vec![VCardParameter::new(
                VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                VCardParameterValue::Text(key.clone()),
            )];
            params.extend(type_params);
            entries.push(
                VCardEntry::new(VCardProperty::Other("LABEL".to_owned()))
                    .with_params(params)
                    .with_value(full.to_owned()),
            );
        }
    }

    for (key, organization) in card.organizations.iter().flatten() {
        let Some(components) = organization_components(organization) else {
            continue;
        };
        entries.push(
            VCardEntry::new(VCardProperty::Org)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_values(components.into_iter().map(VCardValue::Text).collect()),
        );
    }

    for (key, title) in card.titles.iter().flatten() {
        if !states_title(title) {
            continue;
        }
        let Some((_, name)) = TITLE_KINDS
            .iter()
            .find(|(kind, _)| *kind == title_kind(title.kind.as_deref()))
        else {
            continue;
        };
        let prop = if *name == "TITLE" {
            VCardProperty::Title
        } else {
            VCardProperty::Role
        };
        entries.push(
            VCardEntry::new(prop)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_value(title.name.clone()),
        );
    }

    for (key, note) in card.notes.iter().flatten() {
        if !states_note(note) {
            continue;
        }
        entries.push(
            VCardEntry::new(VCardProperty::Note)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_value(note.note.clone()),
        );
    }

    for (key, link) in card.links.iter().flatten() {
        if !states_link(link) {
            continue;
        }
        let prop = match link.kind.as_deref() {
            None => VCardProperty::Url,
            Some("blog") => VCardProperty::Other(X_EVOLUTION_BLOG_URL.to_owned()),
            Some("video") => VCardProperty::Other(X_EVOLUTION_VIDEO_URL.to_owned()),
            _ => continue,
        };
        entries.push(
            VCardEntry::new(prop)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_value(link.uri.clone()),
        );
    }

    // The contact's own calendar and the free/busy data drawn from it, each on
    // the line EDS keeps that one on. An entry naming neither kind gets no
    // line: see [`calendar_property`].
    for (key, calendar) in card.calendars.iter().flatten() {
        let Some(name) = calendar_property(calendar.kind.as_deref()) else {
            continue;
        };
        if calendar.uri.is_empty() {
            continue;
        }
        let prop = if name == "CALURI" {
            VCardProperty::Caluri
        } else {
            VCardProperty::Fburl
        };
        entries.push(
            VCardEntry::new(prop)
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_value(calendar.uri.clone()),
        );
    }

    // The picture the card carries, on the line Evolution shows as the
    // contact's photo — inline where the card holds the bytes, since that is
    // the only form EDS reads a media type off, and a `VALUE=uri` reference
    // where it merely points at them. Both forms and the parameters each takes
    // are measured against libebook-contacts 3.52; see [`photo`] for the
    // entries that get no line at all.
    for (key, media) in card.media.iter().flatten() {
        match photo(media) {
            None => continue,
            Some(Photo::Inline { subtype, base64 }) => {
                let mut params = vec![VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                )];
                if let Some(subtype) = subtype {
                    params.push(VCardParameter::typ(VCardParameterValue::Text(
                        subtype.to_owned(),
                    )));
                }
                params.push(VCardParameter::new(
                    VCardParameterName::Other("ENCODING".to_owned()),
                    VCardParameterValue::Text("b".to_owned()),
                ));
                entries.push(
                    VCardEntry::new(VCardProperty::Photo)
                        .with_params(params)
                        .with_value(base64),
                );
            }
            // Without `VALUE=uri` EDS reaches no field at all, so the user is
            // shown no picture rather than one fetched from the URI.
            Some(Photo::Uri(uri)) => {
                entries.push(
                    VCardEntry::new(VCardProperty::Photo)
                        .with_param(VCardParameter::new(
                            VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                            VCardParameterValue::Text(key.clone()),
                        ))
                        .with_param(VCardParameter::new(
                            VCardParameterName::Value,
                            VCardParameterValue::Text("uri".to_owned()),
                        ))
                        .with_value(uri.to_owned()),
                );
            }
        }
    }

    // One line per service, on the property EDS keeps that service's handles
    // on, and always with a slot: a line carrying no `TYPE` reaches none of the
    // fields Evolution shows.
    for (key, service) in card.online_services.iter().flatten() {
        let Some((property, handle)) = drawn_service(service) else {
            continue;
        };
        entries.push(
            VCardEntry::new(VCardProperty::Other(property.to_owned()))
                .with_param(VCardParameter::new(
                    VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                    VCardParameterValue::Text(key.clone()),
                ))
                .with_param(VCardParameter::typ(VCardParameterValue::Text(
                    service_slot(service).to_owned(),
                )))
                .with_value(handle.to_owned()),
        );
    }

    for (key, anniversary) in card.anniversaries.iter().flatten() {
        let Some(date) = anniversary_date(anniversary) else {
            continue;
        };
        if anniversary.kind == "birth" {
            entries.push(
                VCardEntry::new(VCardProperty::Bday)
                    .with_param(VCardParameter::new(
                        VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                        VCardParameterValue::Text(key.clone()),
                    ))
                    .with_value(date),
            );
        } else if anniversary.kind == "wedding" {
            // Emit both X-EVOLUTION-ANNIVERSARY (for EDS 3.52 compatibility)
            // and standard ANNIVERSARY (for EDS 3.60+ compatibility).
            entries.push(
                VCardEntry::new(VCardProperty::Other(X_EVOLUTION_ANNIVERSARY.to_owned()))
                    .with_param(VCardParameter::new(
                        VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                        VCardParameterValue::Text(key.clone()),
                    ))
                    .with_value(date.clone()),
            );
            entries.push(
                VCardEntry::new(VCardProperty::Other("ANNIVERSARY".to_owned()))
                    .with_param(VCardParameter::new(
                        VCardParameterName::Other(X_JMAP_KEY.to_owned()),
                        VCardParameterValue::Text(key.clone()),
                    ))
                    .with_value(date),
            );
        }
    }

    // The relations EDS keeps fields on: spouse (E_CONTACT_SPOUSE),
    // manager (E_CONTACT_MANAGER), and assistant (E_CONTACT_ASSISTANT).
    // And with no key on them, which no other keyed map here can do:
    // RFC 9553 §2.1.8 keys `relatedTo` by the related entity, so the name
    // on the line *is* the entry's key and there is nothing left for an
    // X-JMAP-KEY to say. Every other relation, and every entity named by a
    // UID rather than by name, gets no line: see [`states_spouse`],
    // [`states_manager`], and [`states_assistant`].
    for (key, relation) in card.related_to.iter().flatten() {
        if states_spouse(key, relation) {
            entries.push(
                VCardEntry::new(VCardProperty::Other(X_EVOLUTION_SPOUSE.to_owned()))
                    .with_value(key.clone()),
            );
        }
        if states_manager(key, relation) {
            entries.push(
                VCardEntry::new(VCardProperty::Other(X_EVOLUTION_MANAGER.to_owned()))
                    .with_value(key.clone()),
            );
        }
        if states_assistant(key, relation) {
            entries.push(
                VCardEntry::new(VCardProperty::Other(X_EVOLUTION_ASSISTANT.to_owned()))
                    .with_value(key.clone()),
            );
        }
    }

    // The whole set on one line, which is all EDS reads, and with no key on it:
    // a tag is its own identity. Empty when every tag the card holds is one the
    // line cannot carry — see [`states_keyword`] — and then there is no line,
    // exactly as for a card with no tags.
    let tags = drawn_tags(card);
    if !tags.is_empty() {
        entries.push(
            VCardEntry::new(VCardProperty::Categories).with_values(
                tags.into_iter()
                    .map(|tag| VCardValue::Text(tag.to_owned()))
                    .collect(),
            ),
        );
    }

    let mut out = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
    for entry in &entries {
        entry
            .write_to(&mut out, true)
            .expect("writing to String never fails");
    }
    out.push_str("END:VCARD\r\n");
    fold_overlong_lines(out)
}

/// The RFC 2426 §2.6 folding width `card_to_vcard`'s output holds to, in
/// octets, excluding the line break.
const MAX_LINE_OCTETS: usize = 75;

/// Folds any physical line longer than [`MAX_LINE_OCTETS`], cutting at UTF-8
/// character boundaries.
///
/// Folding is calcard's job, and its writer does it. Before calcard 0.3.13,
/// a structured value's `;` separators were written *after* its fold check,
/// and that check was skipped when the component coming next was empty
/// text, so a value whose text folded to exactly 75 octets kept every empty
/// trailing slot on the same physical line: 81 octets for an `ADR`, one per
/// empty slot (found by a fuzzer seed; the regression test in
/// `tests/proptest_fuzz.rs` still carries the shrunken card; was
/// <https://github.com/stalwartlabs/calcard/issues/25>, fixed upstream in
/// 0.3.13). This pass is now insurance rather than an active workaround —
/// kept because it is a single scan and it is the guarantee the tightened
/// `prop_emitted_vcard_lines_target_75_octets_and_are_valid_utf8` property
/// rests on. Folding is defined on the octet layer — unfolding restores the
/// same stream wherever a cut lands — so any cut is *correct*; cutting at
/// character boundaries, and never between a `\` and the octet it escapes,
/// additionally keeps every physical line valid UTF-8 and every escape pair
/// whole for line-oriented readers.
fn fold_overlong_lines(vcard: String) -> String {
    if vcard
        .split("\r\n")
        .all(|line| line.len() <= MAX_LINE_OCTETS)
    {
        return vcard;
    }
    let mut out = String::with_capacity(vcard.len() + 16);
    for (index, line) in vcard.split("\r\n").enumerate() {
        if index > 0 {
            out.push_str("\r\n");
        }
        let mut rest = line;
        // A continuation line's leading space spends one of its 75 octets.
        let mut budget = MAX_LINE_OCTETS;
        while rest.len() > budget {
            let mut cut = budget;
            while cut > 0 && !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            // An odd run of backslashes ending at the cut means the last one
            // escapes the octet on the far side — step inside the run so the
            // pair stays on one line.
            if rest[..cut]
                .bytes()
                .rev()
                .take_while(|&byte| byte == b'\\')
                .count()
                % 2
                == 1
            {
                cut -= 1;
            }
            if cut == 0 {
                // Unreachable for anything calcard emits, but never loop: take
                // the first boundary — unfolding restores the octets either way.
                cut = (1..=rest.len())
                    .find(|&i| rest.is_char_boundary(i))
                    .unwrap_or(rest.len());
            }
            out.push_str(&rest[..cut]);
            out.push_str("\r\n ");
            rest = &rest[cut..];
            budget = MAX_LINE_OCTETS - 1;
        }
        out.push_str(rest);
    }
    out
}

/// Pre-normalizes extended ISO 8601 hyphenated dates (`YYYY-MM-DD` and timestamps)
/// on date property lines (`BDAY`, `ANNIVERSARY`, `X-EVOLUTION-ANNIVERSARY`,
/// `X-ABDATE`, `X-AB-DATE`) into basic format (`YYYYMMDD` / `YYYYMMDDTHHMMSS...`)
/// before passing the vCard stream to `calcard`.
///
/// Context & upstream rationale:
/// Upstream `calcard` (0.3.11) exhibits two date parsing limitations:
/// 1. `parse_vcard_date` (used when `VALUE=date` is stated) reads extended format
///    `YYYY-MM-DD` only to the year and month, stopping at the second hyphen and
///    leaving `day` as `None` (losing the day).
/// 2. `parse_vcard_date_and_or_time` (used for standard date-and-or-time properties)
///    misinterprets the second hyphen as a timezone offset transition, storing the
///    day into `tz_hour` when no timezone offset is present, or overwriting it with
///    the actual timezone offset (e.g. `+02:00` corrupting day 12 into day 02), or
///    leaving `day` as `None` on UTC timestamps (`Z`).
///
/// In contrast, `calcard`'s basic-format date parser (`YYYYMMDD` / `YYYYMMDDTHHMMSS`)
/// parses all date components (year, month, day, time, timezone) completely and
/// losslessly across all versions (vCard 2.1, 3.0, 4.0). Pre-normalizing hyphenated
/// date lines to basic format on import guarantees 100% fidelity without data loss.
fn normalize_vcard_dates(vcard: &str) -> String {
    let mut out = String::with_capacity(vcard.len());
    let mut remaining = vcard;
    while !remaining.is_empty() {
        let (line_with_ending, rest) = match remaining.find('\n') {
            Some(pos) => (&remaining[..=pos], &remaining[pos + 1..]),
            None => (remaining, ""),
        };
        remaining = rest;

        let line_trimmed_ending = line_with_ending.trim_end_matches(['\r', '\n']);
        let ending = &line_with_ending[line_trimmed_ending.len()..];

        if let Some((header, value)) = split_vcard_property_line(line_trimmed_ending) {
            let prop_name = header
                .split(';')
                .next()
                .unwrap_or(header)
                .rsplit('.')
                .next()
                .unwrap_or(header)
                .trim();
            if is_date_property_name(prop_name) {
                let normalized_val = normalize_date_value(prop_name, value);
                out.push_str(header);
                out.push(':');
                out.push_str(&normalized_val);
                out.push_str(ending);
                continue;
            }
        }
        out.push_str(line_with_ending);
    }
    out
}

fn is_date_property_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("BDAY")
        || name.eq_ignore_ascii_case("ANNIVERSARY")
        || name.eq_ignore_ascii_case(X_EVOLUTION_ANNIVERSARY)
        || name.eq_ignore_ascii_case("X-ABDATE")
        || name.eq_ignore_ascii_case("X-AB-DATE")
}

fn split_vcard_property_line(line: &str) -> Option<(&str, &str)> {
    let mut in_quotes = false;
    for (idx, byte) in line.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            b':' if !in_quotes => {
                return Some((&line[..idx], &line[idx + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn normalize_date_value(prop_name: &str, val: &str) -> String {
    let bytes = val.as_bytes();
    if bytes.len() == 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        // Extended date YYYY-MM-DD -> basic YYYYMMDD
        let year = &val[0..4];
        let month = &val[5..7];
        let day = &val[8..10];
        format!("{year}{month}{day}")
    } else if prop_name.eq_ignore_ascii_case("ANNIVERSARY")
        && bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        // vCard 4.0 ANNIVERSARY with timestamp: convert to basic format
        let year = &val[0..4];
        let month = &val[5..7];
        let day = &val[8..10];
        let mut out = format!("{year}{month}{day}");
        let rest = &val[10..];
        if let Some(time_part) = rest.strip_prefix(['T', 't']) {
            out.push('T');
            for ch in time_part.chars() {
                if ch != ':' {
                    out.push(ch);
                }
            }
        } else {
            out.push_str(rest);
        }
        out
    } else {
        val.to_owned()
    }
}

/// Read a vCard 3.0 string into a contact card.
///
/// The `id` is whatever the vCard's `UID` says, which for a contact
/// Evolution has just created is a locally invented string rather than a
/// JMAP id — the caller knows which case it is in and must drop it before
/// sending a create.
pub fn vcard_to_card(vcard: &str) -> Result<ContactCard, VCardError> {
    let normalized = normalize_vcard_dates(vcard);
    let card = match Parser::new(&normalized).strict().entry() {
        Entry::VCard(card) => card,
        Entry::UnterminatedComponent(_) => return Err(VCardError::Unterminated),
        Entry::InvalidLine(line) => return Err(VCardError::Malformed(line)),
        _ => return Err(VCardError::NotAVCard),
    };
    let text = |name: &str| {
        card.entries
            .iter()
            .find(|entry| entry.name.as_str().eq_ignore_ascii_case(name))
            .map(entry_text)
            .filter(|value| !value.is_empty())
    };

    let name = read_name(&card.entries);
    let mut nicknames = BTreeMap::new();
    let mut emails = BTreeMap::new();
    let mut phones = BTreeMap::new();
    let mut addresses = BTreeMap::new();
    let mut organizations = BTreeMap::new();
    let mut titles = BTreeMap::new();
    let mut notes = BTreeMap::new();
    let mut anniversaries: BTreeMap<String, Anniversary> = BTreeMap::new();
    let mut links = BTreeMap::new();
    let mut calendars = BTreeMap::new();
    let mut media = BTreeMap::new();
    let mut online_services = BTreeMap::new();
    let mut related_to = BTreeMap::new();

    let mut group_labels = BTreeMap::new();
    for entry in &card.entries {
        if let Some(group) = entry.group.as_deref()
            && entry.name.as_str().eq_ignore_ascii_case("X-ABLabel")
        {
            let text = entry_text(entry);
            if !text.is_empty() {
                group_labels.insert(group, text);
            }
        }
    }

    for entry in &card.entries {
        let name_upper = entry.name.as_str().to_ascii_uppercase();
        match name_upper.as_str() {
            "NICKNAME" => {
                // Read as a `text-list` value, because that is what RFC 2426
                // §3.1.3 makes it and what calcard parses it as: a comma the
                // card left unescaped is part of the nickname here, exactly as
                // it is to EDS, rather than a separator that would file the
                // rest of the line as a second nickname.
                let nickname = Nickname {
                    name: entry_text_list(entry),
                    extra: BTreeMap::new(),
                    ..Nickname::default()
                };
                if !states_nickname(&nickname) {
                    continue;
                }
                nicknames.insert(entry_key(entry, "k", &nicknames), nickname);
            }
            "EMAIL" => {
                let address = entry_text(entry);
                if address.is_empty() {
                    continue;
                }
                let mut contexts = read_flags(&CONTEXTS, entry);
                let mut extra = BTreeMap::new();
                if let Some(group) = entry.group.as_deref()
                    && let Some(raw_label) = group_labels.get(group)
                {
                    let clean = clean_apple_label(raw_label);
                    match clean.to_ascii_lowercase().as_str() {
                        "work" | "school" => {
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"work": true}));
                            }
                        }
                        "home" => {
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"private": true}));
                            }
                        }
                        "other" => {}
                        _ => {
                            extra.insert("label".to_owned(), Value::String(clean.to_owned()));
                        }
                    }
                }
                let email = ContactEmail {
                    address,
                    contexts,
                    pref: entry_has_type(entry, "PREF").then_some(1),
                    extra,
                };
                emails.insert(entry_key(entry, "e", &emails), email);
            }
            "TEL" => {
                let number = entry_text(entry);
                if number.is_empty() {
                    continue;
                }
                let mut contexts = read_flags(&CONTEXTS, entry);
                let mut features = read_phone_flags(entry);
                let mut extra = BTreeMap::new();
                if let Some(group) = entry.group.as_deref()
                    && let Some(raw_label) = group_labels.get(group)
                {
                    let clean = clean_apple_label(raw_label);
                    let clean_lower = clean.to_ascii_lowercase();
                    match clean_lower.as_str() {
                        "mobile" | "cell" | "iphone" => {
                            let mut map = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = map.as_object_mut() {
                                obj.insert("mobile".to_owned(), Value::Bool(true));
                            }
                            features = Some(map);
                        }
                        "pager" => {
                            let mut map = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = map.as_object_mut() {
                                obj.insert("pager".to_owned(), Value::Bool(true));
                            }
                            features = Some(map);
                        }
                        "workfax" | "work fax" => {
                            let mut fmap = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = fmap.as_object_mut() {
                                obj.insert("fax".to_owned(), Value::Bool(true));
                            }
                            features = Some(fmap);
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"work": true}));
                            }
                        }
                        "homefax" | "home fax" => {
                            let mut fmap = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = fmap.as_object_mut() {
                                obj.insert("fax".to_owned(), Value::Bool(true));
                            }
                            features = Some(fmap);
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"private": true}));
                            }
                        }
                        "fax" => {
                            let mut fmap = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = fmap.as_object_mut() {
                                obj.insert("fax".to_owned(), Value::Bool(true));
                            }
                            features = Some(fmap);
                        }
                        "work" | "school" => {
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"work": true}));
                            }
                        }
                        "home" => {
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"private": true}));
                            }
                        }
                        "main" => {
                            let mut fmap = features.unwrap_or_else(|| serde_json::json!({}));
                            if let Some(obj) = fmap.as_object_mut() {
                                obj.insert("voice".to_owned(), Value::Bool(true));
                            }
                            features = Some(fmap);
                            if contexts.is_none() {
                                contexts = Some(serde_json::json!({"work": true}));
                            }
                        }
                        "other" => {}
                        _ => {
                            extra.insert("label".to_owned(), Value::String(clean.to_owned()));
                        }
                    }
                }
                let phone = ContactPhone {
                    number,
                    contexts,
                    features,
                    pref: entry_has_type(entry, "PREF").then_some(1),
                    extra,
                };
                phones.insert(entry_key(entry, "p", &phones), phone);
            }
            "ADR" => {
                let group_label = entry
                    .group
                    .as_deref()
                    .and_then(|g| group_labels.get(g))
                    .map(String::as_str);
                let Some(address) = read_address(entry, group_label) else {
                    continue;
                };
                addresses.insert(entry_key(entry, "a", &addresses), address);
            }
            "ORG" => {
                let Some(organization) = read_organization(entry) else {
                    continue;
                };
                organizations.insert(entry_key(entry, "o", &organizations), organization);
            }
            "TITLE" | "ROLE" => {
                let Some(title) = read_title(entry) else {
                    continue;
                };
                titles.insert(entry_key(entry, "t", &titles), title);
            }
            "NOTE" => {
                let note = Note {
                    note: entry_text(entry),
                    extra: BTreeMap::new(),
                    ..Note::default()
                };
                if !states_note(&note) {
                    continue;
                }
                notes.insert(entry_key(entry, "n", &notes), note);
            }
            "URL" | X_EVOLUTION_BLOG_URL | X_EVOLUTION_VIDEO_URL => {
                // Read as one value, which is what calcard makes of a URI:
                // neither the comma nor the semicolon inside it separates
                // anything, so a query string listing tags arrives as the URI
                // the line stated rather than as a fragment of it.
                let mut kind = if name_upper == "URL" {
                    None
                } else if name_upper == X_EVOLUTION_BLOG_URL {
                    Some("blog".to_owned())
                } else {
                    Some("video".to_owned())
                };
                let mut extra = BTreeMap::new();
                if let Some(group) = entry.group.as_deref()
                    && let Some(raw_label) = group_labels.get(group)
                {
                    let clean = clean_apple_label(raw_label);
                    let clean_lower = clean.to_ascii_lowercase();
                    match clean_lower.as_str() {
                        "homepage" | "home page" => {
                            kind = None;
                        }
                        "blog" => {
                            kind = Some("blog".to_owned());
                        }
                        "work" | "school" => {
                            extra.insert("contexts".to_owned(), serde_json::json!({"work": true}));
                        }
                        "home" => {
                            extra.insert(
                                "contexts".to_owned(),
                                serde_json::json!({"private": true}),
                            );
                        }
                        "other" => {}
                        _ => {
                            extra.insert("label".to_owned(), Value::String(clean.to_owned()));
                        }
                    }
                }
                let link = Link {
                    uri: entry_text(entry),
                    kind,
                    extra,
                    ..Link::default()
                };
                if !states_link(&link) {
                    continue;
                }
                links.insert(entry_key(entry, "l", &links), link);
            }
            // Both calendaring lines feed one keyed map, so the line's own name
            // is the only thing that says what kind the entry is — and the keys
            // the reader invents for the two have to be free of each other's.
            "CALURI" | "FBURL" => {
                let uri = entry_text(entry);
                if uri.is_empty() {
                    continue;
                }
                let calendar = Calendar {
                    kind: calendar_kind(&name_upper).map(str::to_owned),
                    uri,
                    extra: BTreeMap::new(),
                    ..Calendar::default()
                };
                calendars.insert(entry_key(entry, "c", &calendars), calendar);
            }
            "PHOTO" => {
                let Some(photo) = read_photo(entry) else {
                    continue;
                };
                media.insert(entry_key(entry, "m", &media), photo);
            }
            // Every line of the name, not only the first EDS shows the user: a
            // relation nobody can edit is still one a save must not delete. The
            // key is the line's own text, so nothing is invented and an
            // X-JMAP-KEY, if some other client wrote one, is not read.
            X_EVOLUTION_SPOUSE
            | X_EVOLUTION_MANAGER
            | X_EVOLUTION_ASSISTANT
            | "X-ABRELATEDNAMES"
            | "X-AB-RELATED-NAMES"
            | "RELATED" => {
                let name = entry_text(entry);
                if !names_a_person(&name) {
                    continue;
                }
                let relation_type = if name_upper == X_EVOLUTION_SPOUSE {
                    SPOUSE_RELATION.to_owned()
                } else if name_upper == X_EVOLUTION_MANAGER {
                    MANAGER_RELATION.to_owned()
                } else if name_upper == X_EVOLUTION_ASSISTANT {
                    ASSISTANT_RELATION.to_owned()
                } else if name_upper == "RELATED" {
                    if entry_has_type(entry, "spouse") || entry_has_type(entry, "partner") {
                        SPOUSE_RELATION.to_owned()
                    } else if entry_has_type(entry, "manager") {
                        MANAGER_RELATION.to_owned()
                    } else if entry_has_type(entry, "assistant") {
                        ASSISTANT_RELATION.to_owned()
                    } else if let Some(type_param) = entry_param(entry, "TYPE") {
                        let clean = clean_apple_label(&type_param);
                        if clean.is_empty() {
                            "contact".to_owned()
                        } else {
                            clean.to_ascii_lowercase()
                        }
                    } else if let Some(group) = entry.group.as_deref()
                        && let Some(raw_label) = group_labels.get(group)
                    {
                        let clean = clean_apple_label(raw_label);
                        match clean.to_ascii_lowercase().as_str() {
                            "spouse" | "partner" => SPOUSE_RELATION.to_owned(),
                            "manager" => MANAGER_RELATION.to_owned(),
                            "assistant" => ASSISTANT_RELATION.to_owned(),
                            other if !other.is_empty() => other.to_owned(),
                            _ => "contact".to_owned(),
                        }
                    } else {
                        "contact".to_owned()
                    }
                } else if let Some(group) = entry.group.as_deref()
                    && let Some(raw_label) = group_labels.get(group)
                {
                    let clean = clean_apple_label(raw_label);
                    match clean.to_ascii_lowercase().as_str() {
                        "spouse" | "partner" => SPOUSE_RELATION.to_owned(),
                        "manager" => MANAGER_RELATION.to_owned(),
                        "assistant" => ASSISTANT_RELATION.to_owned(),
                        other if !other.is_empty() => other.to_owned(),
                        _ => "contact".to_owned(),
                    }
                } else {
                    continue;
                };
                let entry_rel = related_to.entry(name).or_insert_with(|| Relation {
                    relation: Some(BTreeMap::new()),
                    extra: BTreeMap::new(),
                });
                if let Some(types) = &mut entry_rel.relation {
                    types.insert(relation_type, Value::Bool(true));
                } else {
                    entry_rel.relation = Some([(relation_type, Value::Bool(true))].into());
                }
            }
            "BDAY" | "ANNIVERSARY" | X_EVOLUTION_ANNIVERSARY | "X-ABDATE" | "X-AB-DATE" => {
                if name_upper == "X-ABDATE" || name_upper == "X-AB-DATE" {
                    let date_text = entry_text(entry);
                    if let Some(day) = read_day(&date_text)
                        && let Some(group) = entry.group.as_deref()
                        && let Some(raw_label) = group_labels.get(group)
                    {
                        let clean = clean_apple_label(raw_label);
                        let kind = match clean.to_ascii_lowercase().as_str() {
                            "anniversary" | "wedding" => "wedding".to_owned(),
                            "birthday" | "birth" => "birth".to_owned(),
                            other if !other.is_empty() => other.to_owned(),
                            _ => "wedding".to_owned(),
                        };
                        let anniversary = Anniversary {
                            kind,
                            date: Some(day.json()),
                            extra: BTreeMap::new(),
                        };
                        let key = entry_param(entry, X_JMAP_KEY).filter(|k| !k.is_empty());
                        if let Some(k) = key.as_deref()
                            && let Some(existing) = anniversaries.get(k)
                            && existing.kind == anniversary.kind
                            && existing.date == anniversary.date
                        {
                            continue;
                        }
                        if key.is_none()
                            && anniversaries.values().any(|existing| {
                                existing.kind == anniversary.kind
                                    && existing.date == anniversary.date
                            })
                        {
                            continue;
                        }
                        anniversaries.insert(entry_key(entry, "y", &anniversaries), anniversary);
                    }
                } else if let Some(anniversary) = read_anniversary(entry) {
                    let key = entry_param(entry, X_JMAP_KEY).filter(|k| !k.is_empty());
                    if let Some(k) = key.as_deref()
                        && let Some(existing) = anniversaries.get(k)
                        && existing.kind == anniversary.kind
                        && existing.date == anniversary.date
                    {
                        continue;
                    }
                    if key.is_none()
                        && anniversaries.values().any(|existing| {
                            existing.kind == anniversary.kind && existing.date == anniversary.date
                        })
                    {
                        continue;
                    }
                    anniversaries.insert(entry_key(entry, "y", &anniversaries), anniversary);
                }
            }
            "IMPP" => {
                let uri = entry_text(entry);
                if let Some((scheme, handle)) = uri.split_once(':') {
                    let wanted_scheme = scheme.to_ascii_lowercase();
                    let matched_service = if wanted_scheme == "xmpp" {
                        Some("Jabber")
                    } else {
                        SERVICE_SCHEMES
                            .iter()
                            .find(|(_, s)| s.eq_ignore_ascii_case(&wanted_scheme))
                            .map(|(service, _)| *service)
                    };
                    if let Some(service) = matched_service
                        && plain_handle(handle)
                    {
                        let entry_obj = OnlineService {
                            service: Some(service.to_owned()),
                            user: Some(handle.to_owned()),
                            uri: None,
                            extra: BTreeMap::new(),
                            ..OnlineService::default()
                        };
                        online_services.insert(entry_key(entry, "s", &online_services), entry_obj);
                    }
                }
            }
            // One of the `X-` lines EDS keeps instant-messaging handles on, and
            // nothing else: a line for a service this mapping does not state is
            // left where it is rather than read as an entry the server never
            // had. The `TYPE` is not read — it is the slot, not the contexts.
            name => {
                let Some(service) = service_of(name) else {
                    continue;
                };
                let handle = entry_text(entry);
                if handle.is_empty() {
                    continue;
                }
                let entry_obj = OnlineService {
                    service: Some(service.to_owned()),
                    user: Some(handle),
                    uri: None,
                    extra: BTreeMap::new(),
                    ..OnlineService::default()
                };
                online_services.insert(entry_key(entry, "s", &online_services), entry_obj);
            }
        }
    }

    // The `LABEL` lines after the `ADR` ones, because a label states an
    // address the card may already have named and has to find it first.
    for entry in card
        .entries
        .iter()
        .filter(|entry| entry.name.as_str().eq_ignore_ascii_case("LABEL"))
    {
        let full = entry_text(entry);
        if full.is_empty() {
            continue;
        }
        let contexts = read_flags(&CONTEXTS, entry);
        let key = label_entry(entry, contexts.as_ref(), &addresses, &full);
        let address = addresses.entry(key).or_insert_with(|| Address {
            contexts,
            ..Address::default()
        });
        address.full = Some(full);
        if entry_has_type(entry, "PREF") && !address.extra.contains_key("pref") {
            address
                .extra
                .insert("pref".to_owned(), serde_json::Value::from(1));
        }
    }

    Ok(ContactCard {
        id: text("UID").map(Into::into),
        // Membership follows from which EDS source is being served, not from
        // the contact, so the backend fills it in on create.
        address_book_ids: None,
        card_type: Some("Card".to_owned()),
        version: Some("1.0".to_owned()),
        uid: text(X_JMAP_UID),
        name,
        nicknames: (!nicknames.is_empty()).then_some(nicknames),
        emails: (!emails.is_empty()).then_some(emails),
        phones: (!phones.is_empty()).then_some(phones),
        addresses: (!addresses.is_empty()).then_some(addresses),
        organizations: (!organizations.is_empty()).then_some(organizations),
        titles: (!titles.is_empty()).then_some(titles),
        notes: (!notes.is_empty()).then_some(notes),
        anniversaries: (!anniversaries.is_empty()).then_some(anniversaries),
        links: (!links.is_empty()).then_some(links),
        calendars: (!calendars.is_empty()).then_some(calendars),
        // Only the pictures — a `PHOTO` line is the one media kind vCard 3.0
        // states, so the sounds and logos a card carries are read back by
        // nothing and left to the save to patch around.
        media: (!media.is_empty()).then_some(media),
        online_services: (!online_services.is_empty()).then_some(online_services),
        keywords: read_keywords(&card.entries),
        // Only the marriages: `spouse` is the one relation type with a line, so
        // every other entity the card relates to is read back by nothing and
        // left to the save to patch around.
        related_to: (!related_to.is_empty()).then_some(related_to),
        crypto_keys: None,
        directories: None,
        personal_info: None,
        speak_to_as: None,
        preferred_languages: None,
        localizations: None,
        kind: None,
        extra: BTreeMap::new(),
        ..ContactCard::default()
    })
}

/// The media entry a `PHOTO` line states, or `None` for a line naming no
/// picture at all.
///
/// The inverse of the emitter's two forms, and only of those two: `VALUE=uri` is
/// a picture the card points at, and anything else is a picture it carries,
/// whose bytes cross as a `data:` URI (RFC 2397) because that is how RFC 9553
/// §2.6.4 states one. Both are what EDS's own writer emits, measured against
/// libebook-contacts 3.52 — with the `X-JMAP-KEY` gone from a line the user
/// edited, which is the save's problem rather than this one's.
///
/// The bytes come from [`entry_binary`] where the line carried bytes and
/// from the value's own where it carried text, since calcard decodes the base64
/// either way and surfaces bytes only when the result is not a string: an SVG is
/// a picture whose bytes *are* text. The cost of taking both paths is that a
/// `PHOTO` line stating neither `VALUE=uri` nor a transfer encoding — a bare
/// `PHOTO:<uri>`, which EDS reads as no picture at all (measured) — is
/// indistinguishable here from a picture whose bytes are text, and is read as
/// the latter. Neither EDS's writer nor this emitter produces such a line.
///
/// The media type is built from the `TYPE` parameter rather than taken from
/// calcard, so that the one rule EDS applies is applied once and in one place:
/// the parameter states the subtype and the type is `image/` in front of it (see
/// [`image_subtype`]). [`UNKNOWN_TYPE`] is what EDS writes when it has none, and
/// names no format, so it reads back as none.
fn read_photo(entry: &VCardEntry) -> Option<Media> {
    let raw_text = entry_text(entry);
    let states_a_reference = (entry_param(entry, "VALUE")
        .is_some_and(|value| value.eq_ignore_ascii_case(URI_VALUE))
        && !raw_text.starts_with("data:"))
        || raw_text.starts_with("http://")
        || raw_text.starts_with("https://")
        || raw_text.starts_with("ftp://")
        || raw_text.starts_with("file://");

    let media_type_param = entry_param(entry, "MEDIATYPE")
        .or_else(|| {
            entry_param(entry, "TYPE")
                .filter(|subtype| {
                    !subtype.eq_ignore_ascii_case(UNKNOWN_TYPE)
                        && !matches!(
                            subtype.to_ascii_uppercase().as_str(),
                            "WORK" | "HOME" | "PREF" | "OTHER"
                        )
                })
                .map(|subtype| {
                    if subtype.contains('/') {
                        subtype
                    } else {
                        format!("{IMAGE_PREFIX}{subtype}")
                    }
                })
        })
        .or_else(|| {
            entry.params.iter().find_map(|param| {
                let name = param.name.as_str();
                is_known_image_subtype(name).then(|| format!("{IMAGE_PREFIX}{name}"))
            })
        })
        .or_else(|| entry_binary_content_type(entry).map(str::to_owned));

    // A PHOTO property can only represent an image format. Non-image types (such as
    // audio/ogg or application/pdf) cannot be stated on vCard 3.0 PHOTO lines, and
    // keeping them on parse would cause roundtrip asymmetry against card_to_vcard.
    let media_type = media_type_param.filter(|mt| image_subtype(mt).is_some());

    if states_a_reference {
        return (!raw_text.is_empty()).then(|| photo_entry(raw_text, media_type));
    }

    let (bytes, stated_media_type) = if let Some(rest) = strip_prefix_ci(&raw_text, DATA_SCHEME) {
        if let Some((metadata, payload)) = rest.split_once(',') {
            let stated = strip_suffix_ci(metadata, BASE64_MARKER)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let bytes = decoded(payload).or_else(|| entry_binary(entry).map(<[u8]>::to_vec))?;
            (bytes, stated)
        } else {
            let bytes = entry_binary(entry)?.to_vec();
            (bytes, None)
        }
    } else if let Some(bytes) = entry_binary(entry) {
        (bytes.to_vec(), None)
    } else if let Some(bytes) = decoded(&raw_text) {
        (bytes, None)
    } else {
        let bytes = raw_text.into_bytes();
        (bytes, None)
    };

    if bytes.is_empty() {
        return None;
    }

    let media_type =
        media_type.or_else(|| stated_media_type.filter(|mt| image_subtype(mt).is_some()));
    let uri = format!(
        "{DATA_SCHEME}{}{BASE64_MARKER},{}",
        media_type.as_deref().unwrap_or_default(),
        BASE64.encode(&bytes)
    );
    Some(photo_entry(uri, media_type))
}

fn is_known_image_subtype(subtype: &str) -> bool {
    matches!(
        subtype.to_ascii_uppercase().as_str(),
        "JPEG"
            | "JPG"
            | "PNG"
            | "GIF"
            | "TIFF"
            | "TIF"
            | "BMP"
            | "WEBP"
            | "SVG"
            | "SVG+XML"
            | "HEIC"
            | "AVIF"
            | "ICO"
    )
}

/// A media entry for a picture of the contact — the one kind a `PHOTO` line
/// states, so the only kind this side reads back.
fn photo_entry(uri: String, media_type: Option<String>) -> Media {
    Media {
        kind: Some(PHOTO_KIND.to_owned()),
        uri,
        media_type,
        extra: BTreeMap::new(),
        ..Media::default()
    }
}

/// The title a `TITLE` or `ROLE` line states, or `None` for a line with no
/// text on it.
///
/// The kind is left unsaid when it is the default, so that reading back a
/// card that never named one produces the card that was there — a save then
/// has nothing to patch.
fn read_title(entry: &VCardEntry) -> Option<Title> {
    let name = entry_text(entry);
    if name.is_empty() {
        return None;
    }
    let kind = TITLE_KINDS
        .iter()
        .find(|(_, mapped)| mapped.eq_ignore_ascii_case(entry.name.as_str()))
        .map(|(kind, _)| *kind)
        .filter(|kind| *kind != DEFAULT_TITLE_KIND);
    Some(Title {
        name,
        kind: kind.map(str::to_owned),
        extra: BTreeMap::new(),
        ..Title::default()
    })
}

/// The anniversary a date line states, or `None` for a line no calendar day
/// can be read out of.
///
/// The kind is the line's own: a `BDAY` states a birthday and nothing else,
/// so unlike a title's it is never guessed at and never left unsaid.
fn read_anniversary(entry: &VCardEntry) -> Option<Anniversary> {
    let kind = if entry.name.as_str().eq_ignore_ascii_case("BDAY") {
        "birth"
    } else if entry.name.as_str().eq_ignore_ascii_case("ANNIVERSARY")
        || entry
            .name
            .as_str()
            .eq_ignore_ascii_case(X_EVOLUTION_ANNIVERSARY)
    {
        "wedding"
    } else {
        return None;
    };
    Some(Anniversary {
        kind: kind.to_owned(),
        date: Some(read_day(&entry_text(entry))?.json()),
        extra: BTreeMap::new(),
    })
}

/// The seven `ADR` fields for an address, or `None` for one with nothing to
/// put in any of them — an address stated only in components vCard has no
/// field for, which is then invisible to the user and to the save.
///
/// Empty fields are kept: a field's position is what says which part of the
/// address it is.
fn address_fields(address: &Address) -> Option<Vec<String>> {
    let components = address.components.as_ref()?;
    let mut fields = vec![String::new(); ADDRESS_COMPONENTS.len()];
    let mut any = false;
    for component in components {
        let Some(index) = address_field(&component.kind) else {
            continue;
        };
        if component.value.is_empty() {
            continue;
        }
        // Components that share a field — a street named on two lines, or a
        // street name and the house number standing on it — are written into
        // it one after another, in the order the card lists them.
        if !fields[index].is_empty() {
            fields[index].push(' ');
        }
        fields[index].push_str(&component.value);
        any = true;
    }
    any.then_some(fields)
}

/// The components an edited `ADR` line states, with every field that still
/// says exactly what the server built it from given those parts back.
///
/// A field built from several components is read back as one component of the
/// field's own kind, because nothing in `Hauptstraße 1` says where the street
/// name ends and the house number begins, and a guess would be wrong in half
/// the world's addresses. See `restore_shared_fields` for what is then done
/// about it.
pub fn restore_address_components(
    current: &[AddressComponent],
    edited: &[AddressComponent],
) -> Vec<AddressComponent> {
    restore_shared_fields(
        current,
        edited,
        ADDRESS_COMPONENTS.map(|(_, index)| index),
        |component| address_field(&component.kind),
        |component| component.value.as_str(),
    )
}

/// The components an edited `N` value states, with every field that still says
/// exactly what the server built it from given those parts back.
///
/// The `N` value has one field per component kind, so what shares a field here
/// is two components of the *same* kind: RFC 9553 §2.2.1 states a
/// double-barrelled given name as two `given` components, and `N`'s second
/// field holds them both. The treatment is `restore_shared_fields`, the same
/// one an address's street gets.
pub fn restore_name_components(
    current: &[NameComponent],
    edited: &[NameComponent],
) -> Vec<NameComponent> {
    restore_shared_fields(
        current,
        edited,
        NAME_COMPONENTS.map(|(_, index)| index),
        |component| name_field(&component.kind),
        |component| component.value.as_str(),
    )
}

/// The parts a vCard field was built from, told apart again where the field
/// still reads as those parts joined.
///
/// Several components can share one field — a street name and the house number
/// standing on it, both halves of a double-barrelled given name — and come back
/// from the vCard as the single component that field's text was written into.
/// Left at that, opening a contact and closing it again would flatten the parts
/// the server had stated separately, so a save asks this first: if the field
/// still reads as the parts joined, it *is* those parts, unedited, and they are
/// put back in the order and shape they went out in. If it does not, the user
/// retyped the field, and it stays the one component they typed — the parts it
/// was built from cannot be recovered, and keeping the old ones would leave a
/// house number standing on a street that is no longer there, or half an old
/// first name beside the new one.
///
/// `fields` names the positions to look at, `field` says which one a component
/// is written into, and `value` reads the text a component contributes.
fn restore_shared_fields<T: Clone>(
    current: &[T],
    edited: &[T],
    fields: impl IntoIterator<Item = usize>,
    field: impl Fn(&T) -> Option<usize>,
    value: impl Fn(&T) -> &str,
) -> Vec<T> {
    let mut restored = edited.to_vec();
    for index in fields {
        let parts: Vec<&T> = current
            .iter()
            .filter(|part| field(part) == Some(index) && !value(part).is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }
        let joined = parts
            .iter()
            .map(|part| value(part))
            .collect::<Vec<_>>()
            .join(" ");
        let mut stated = restored
            .iter()
            .enumerate()
            .filter(|(_, part)| field(part) == Some(index));
        let at = match (stated.next(), stated.next()) {
            (Some((at, part)), None) if value(part) == joined => at,
            _ => continue,
        };
        restored.splice(at..=at, parts.into_iter().cloned());
    }
    restored
}

/// The preference rank of an address, stored in its `pref` property (as `Address`
/// in JSContact RFC 9553 §2.5.1 has a `pref` property), or `None` if unranked.
fn address_pref(address: &Address) -> Option<u32> {
    address.pref.or_else(|| {
        address
            .extra
            .get("pref")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    })
}

/// The address an `ADR` line states, or `None` when every field of it is
/// empty — the same "nothing was said" an `EMAIL:` with no address is.
fn read_address(entry: &VCardEntry, group_label: Option<&str>) -> Option<Address> {
    let fields = entry_components(entry);
    let mut components = Vec::new();
    for (kind, index) in ADDRESS_COMPONENTS {
        let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
            continue;
        };
        components.push(AddressComponent::new(kind, value));
    }
    let full = entry_param(entry, "LABEL").filter(|label| !label.is_empty());
    if components.is_empty() && full.is_none() {
        return None;
    }
    let mut extra = BTreeMap::new();
    if entry_has_type(entry, "PREF") {
        extra.insert("pref".to_owned(), serde_json::Value::from(1));
    }
    let mut contexts = read_flags(&CONTEXTS, entry);
    if let Some(raw_label) = group_label {
        let clean = clean_apple_label(raw_label);
        match clean.to_ascii_lowercase().as_str() {
            "work" | "school" => {
                if contexts.is_none() {
                    contexts = Some(serde_json::json!({"work": true}));
                }
            }
            "home" => {
                if contexts.is_none() {
                    contexts = Some(serde_json::json!({"private": true}));
                }
            }
            "other" => {}
            _ => {
                extra.insert("label".to_owned(), Value::String(clean.to_owned()));
            }
        }
    }
    Some(Address {
        components: (!components.is_empty()).then_some(components),
        contexts,
        full,
        extra,
        ..Address::default()
    })
}

/// The `ORG` components for an organisation — its name, then its units — or
/// `None` for an entry that names neither and so has no line to be written on.
///
/// An organisation with units and no name keeps the empty first component:
/// the name's meaning is its position, so letting a unit slide into it would
/// say the department is the employer.
fn organization_components(organization: &Organization) -> Option<Vec<String>> {
    let name = organization.name.clone().unwrap_or_default();
    let units: Vec<String> = organization
        .units
        .iter()
        .flatten()
        .filter(|unit| !unit.name.is_empty())
        .map(|unit| unit.name.clone())
        .collect();
    if name.is_empty() && units.is_empty() {
        return None;
    }
    let mut components = vec![name];
    components.extend(units);
    Some(components)
}

/// The organisation an `ORG` line states, or `None` when every component of
/// it is empty — the same "nothing was said" an `EMAIL:` with no address is.
fn read_organization(entry: &VCardEntry) -> Option<Organization> {
    let components = entry_components(entry);
    let name = components.first().filter(|name| !name.is_empty()).cloned();
    let units: Vec<OrgUnit> = components
        .iter()
        .skip(1)
        .filter(|unit| !unit.is_empty())
        .map(|unit| OrgUnit::new(unit))
        .collect();
    if name.is_none() && units.is_empty() {
        return None;
    }
    Some(Organization {
        name,
        units: (!units.is_empty()).then_some(units),
        extra: BTreeMap::new(),
        ..Organization::default()
    })
}

/// The `addresses` entry a `LABEL` line states: the one it names, the one it
/// matches, or a new one of its own.
///
/// An address stated only in `full` has no `ADR` line, so its key crosses on
/// the `LABEL` and nowhere else — which is why a key naming no address yet is
/// taken at its word rather than being replaced by an invented one.
///
/// Failing a key there is the `TYPE`, which is how RFC 2426 §3.2.2 has a
/// `LABEL` say which `ADR` it is the written-out form of. That fallback is
/// not decoration: `E_CONTACT_ADDRESS_LABEL_HOME` is one of EDS's synthetic
/// fields, so EDS rebuilds the line from the text alone and the `X-JMAP-KEY`
/// this side wrote does not survive the trip through Evolution. Without the
/// fallback every save would then file the label as a second address.
fn label_entry(
    entry: &VCardEntry,
    contexts: Option<&Value>,
    addresses: &BTreeMap<String, Address>,
    full: &str,
) -> String {
    let unlabelled_or_matching =
        |address: &Address| address.full.is_none() || address.full.as_deref() == Some(full);
    if let Some(key) = entry_param(entry, X_JMAP_KEY).filter(|key| !key.is_empty())
        && addresses.get(&key).is_none_or(unlabelled_or_matching)
    {
        return key;
    }
    if let Some((key, _)) = addresses.iter().find(|(_, address)| {
        unlabelled_or_matching(address) && address.contexts.as_ref() == contexts
    }) {
        return key.clone();
    }
    entry_key(entry, "a", addresses)
}

/// The JSContact map key for an entry: the one we round-tripped, or the
/// first free `e1`, `e2`, … for a vCard that never had one.
fn entry_key<T>(entry: &VCardEntry, prefix: &str, taken: &BTreeMap<String, T>) -> String {
    if let Some(key) = entry_param(entry, X_JMAP_KEY).filter(|key| !key.is_empty())
        && !taken.contains_key(&key)
    {
        return key;
    }
    (1..)
        .map(|index| format!("{prefix}{index}"))
        .find(|candidate| !taken.contains_key(candidate))
        .expect("an unbounded sequence has a free element")
}

/// The five vCard `N` fields, or `None` if the card names no components.
fn name_fields(name: &Name) -> Option<Vec<String>> {
    let components = name.components.as_ref()?;
    let mut fields = vec![String::new(); 5];
    let mut any = false;
    for component in components {
        let Some(index) = name_field(&component.kind) else {
            continue;
        };
        if component.value.is_empty() {
            continue;
        }
        // Two components of the same kind (a double-barrelled given name)
        // share one vCard field, and are told apart again on the way back by
        // `restore_name_components`.
        if !fields[index].is_empty() {
            fields[index].push(' ');
        }
        fields[index].push_str(&component.value);
        any = true;
    }
    any.then_some(fields)
}

/// A display name assembled from the components, for a card that has none.
fn derive_full(name: &Name) -> Option<String> {
    let components = name.components.as_ref()?;
    let mut parts = Vec::new();
    for (kind, _) in NAME_COMPONENTS {
        for component in components {
            if component.kind == kind && !component.value.is_empty() {
                parts.push(component.value.as_str());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn read_name(entries: &[VCardEntry]) -> Option<Name> {
    let find = |name: &str| {
        entries
            .iter()
            .find(|entry| entry.name.as_str().eq_ignore_ascii_case(name))
    };

    let full = find("FN").map(entry_text).filter(|f| !f.is_empty());
    let fields = find("N").map(entry_components).unwrap_or_default();

    // Each component is read as the bare kind and value the `N` field states.
    // What a name component carries besides that — RFC 9553 §2.2.1's `phonetic`
    // spelling — has no field and no parameter here, so it is left to the save
    // to put back on the component it belongs to.
    let mut components = Vec::new();
    for (kind, index) in NAME_COMPONENTS {
        let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
            continue;
        };
        components.push(NameComponent::new(kind, value));
    }

    let mut extra = BTreeMap::new();
    if let Some(file_as) = find("X-EVOLUTION-FILE-AS")
        .or_else(|| find("FILE-AS"))
        .or_else(|| find("X-FILE-AS"))
        .map(entry_text)
        .filter(|f| !f.is_empty())
    {
        extra.insert("fileAs".to_string(), Value::String(file_as));
    }

    // No FN, no usable N, and no fileAs: the vCard simply does not name anybody. Note
    // that a missing N is never guessed at by splitting FN — a wrong guess
    // would be written back to the server on the next save.
    if full.is_none() && components.is_empty() && extra.is_empty() {
        return None;
    }
    Some(Name {
        components: (!components.is_empty()).then_some(components),
        full,
        extra,
        ..Name::default()
    })
}

/// vCard `TYPE` values for the JSContact boolean map `value`.
fn type_names(table: &[(&str, &'static str)], value: Option<&Value>) -> Vec<&'static str> {
    let Some(Value::Object(flags)) = value else {
        return Vec::new();
    };
    table
        .iter()
        .filter(|(key, _)| flags.get(*key) == Some(&Value::Bool(true)))
        .map(|(_, type_name)| *type_name)
        .collect()
}

/// The JSContact boolean map for the `TYPE` values present on `entry`.
fn read_flags(table: &[(&str, &str)], entry: &VCardEntry) -> Option<Value> {
    let flags: Map<String, Value> = table
        .iter()
        .filter(|(_, type_name)| entry_has_type(entry, type_name))
        .map(|(key, _)| ((*key).to_owned(), Value::Bool(true)))
        .collect();
    (!flags.is_empty()).then_some(Value::Object(flags))
}

/// The JSContact phone `features` boolean map for the `TYPE` values present on a `TEL` entry.
fn read_phone_flags(entry: &VCardEntry) -> Option<Value> {
    let flags: Map<String, Value> = PHONE_FEATURE_TYPES
        .iter()
        .filter(|(_, types)| {
            types
                .iter()
                .any(|type_name| entry_has_type(entry, type_name))
        })
        .map(|(key, _)| ((*key).to_owned(), Value::Bool(true)))
        .collect();
    (!flags.is_empty()).then_some(Value::Object(flags))
}

fn param_text(value: &VCardParameterValue) -> String {
    match value {
        VCardParameterValue::Text(text) => text.clone(),
        VCardParameterValue::Integer(number) => number.to_string(),
        VCardParameterValue::Timestamp(stamp) => stamp.to_string(),
        VCardParameterValue::Bool(true) => "TRUE".to_owned(),
        VCardParameterValue::Bool(false) => "FALSE".to_owned(),
        VCardParameterValue::ValueType(kind) => kind.as_str().to_owned(),
        VCardParameterValue::Type(kind) => kind.as_str().to_owned(),
        VCardParameterValue::Calscale(scale) => scale.as_str().to_owned(),
        VCardParameterValue::Level(level) => level.as_str().to_owned(),
        VCardParameterValue::Phonetic(system) => system.as_str().to_owned(),
        _ => String::new(),
    }
}

fn value_text(value: &VCardValue) -> Option<String> {
    match value {
        VCardValue::Text(text) => Some(text.clone()),
        VCardValue::Component(items) => Some(items.join(",")),
        VCardValue::PartialDateTime(date) => {
            let day = date.day.or_else(|| {
                // Workaround for calcard parse_vcard_date_and_or_time bug where the
                // second hyphen causes idx to jump to tz_hour.
                date.tz_hour.filter(|d| (1..=31).contains(d))
            });
            if let (Some(y), Some(m), Some(d)) = (date.year, date.month, day) {
                Some(format!("{y:04}-{m:02}-{d:02}"))
            } else {
                let mut text = String::new();
                date.format_as_vcard(&mut text, &VCardValueType::DateAndOrTime)
                    .ok()?;
                Some(text)
            }
        }
        VCardValue::Integer(number) => Some(number.to_string()),
        VCardValue::Float(number) => Some(number.to_string()),
        VCardValue::Boolean(true) => Some("TRUE".to_owned()),
        VCardValue::Boolean(false) => Some("FALSE".to_owned()),
        VCardValue::Kind(kind) => Some(kind.as_str().to_owned()),
        VCardValue::Sex(sex) => Some(sex.as_str().to_owned()),
        _ => None,
    }
}

fn entry_text(entry: &VCardEntry) -> String {
    entry
        .values
        .iter()
        .filter_map(value_text)
        .collect::<Vec<_>>()
        .join(";")
}

fn entry_text_list(entry: &VCardEntry) -> String {
    entry
        .values
        .iter()
        .filter_map(value_text)
        .collect::<Vec<_>>()
        .join(",")
}

fn entry_components(entry: &VCardEntry) -> Vec<String> {
    entry.values.iter().filter_map(value_text).collect()
}

fn entry_items(entry: &VCardEntry) -> Vec<String> {
    entry.values.iter().filter_map(value_text).collect()
}

fn entry_binary(entry: &VCardEntry) -> Option<&[u8]> {
    entry.values.iter().find_map(|value| match value {
        VCardValue::Binary(data) => Some(data.data.as_slice()),
        _ => None,
    })
}

fn entry_binary_content_type(entry: &VCardEntry) -> Option<&str> {
    entry.values.iter().find_map(|value| match value {
        VCardValue::Binary(data) => data.content_type.as_deref(),
        _ => None,
    })
}

fn entry_param(entry: &VCardEntry, name: &str) -> Option<String> {
    entry
        .params
        .iter()
        .find(|param| param.name.as_str().eq_ignore_ascii_case(name))
        .map(|param| param_text(&param.value))
}

fn entry_has_type(entry: &VCardEntry, value: &str) -> bool {
    entry.params.iter().any(|param| {
        if param.name.as_str().eq_ignore_ascii_case("TYPE") {
            let text = param_text(&param.value);
            text.trim_matches('"')
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case(value))
        } else {
            param.name.as_str().eq_ignore_ascii_case(value)
        }
    })
}
