// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! JSContact [`ContactCard`] ↔ vCard 3.0.
//!
//! The mapped set is deliberately the one the address book backend needs to
//! be useful — UID, FN, N, NICKNAME, EMAIL, TEL, ADR, LABEL, ORG, TITLE, ROLE,
//! NOTE, BDAY, URL, CALURI, FBURL, PHOTO, CATEGORIES and the instant-messaging
//! `X-` lines —
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
//! parameter for — hence the [`X_JMAP_KEY`] this side already writes on an
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
//! ([`ADDRESS_COMPONENTS`]) and one more, the house `number`, shares the
//! street's ([`JOINED_COMPONENTS`]); the rest — `floor`, `room`, `landmark` —
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
//! would show the user 1000-01-01. A point in time crosses as the day it
//! falls on, leaving the hour behind for the save to patch around
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
//! is rewritten in place with its parameters intact, so the [`X_JMAP_KEY`]
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
//! The [`X_JMAP_KEY`] survives here too: measured against libebook-contacts
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
//! [`X_JMAP_KEY`] survives here too, for the reason it survives on a `URL`: a
//! set rewrites the first line of that name in place and leaves its parameters
//! alone, and any further line of the same name passes through untouched.
//!
//! `onlineServices` is the one property vCard 3.0 has no line for at all. RFC
//! 9553 §2.3.2 names the contact as one service or protocol knows them; RFC
//! 4770's `IMPP` is vCard 4.0, which is not the format
//! `e_contact_new_from_vcard()` is handed, so the line is the `X-` one EDS
//! itself keeps a handle on — [`ONLINE_SERVICES`] — and the mapping states only
//! the ten services libebook-contacts 3.52 gives contact-editor slots to. Which
//! makes the property lossy in three separate places: a service EDS has no field
//! for has no line, an entry stating a `uri` and no `user` has one only where
//! [`SERVICE_SCHEMES`] says the URI holds the handle and nothing else (see
//! [`drawn_service`]), and neither has a handle the line would come back from
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
//! parameter states the subtype alone ([`image_subtype`]). A picture the card
//! only *points at* crosses as a `VALUE=uri` reference — the shape EDS's own
//! writer emits, and one it reads as no picture at all when that parameter is
//! missing, also measured. What else gets no line is a `data:` URI spelling its
//! bytes as percent-encoded octets rather than base64, since `ENCODING=b` is
//! the only encoding the line carries ([`photo`]).
//!
//! A `PHOTO` line is read back into a `media` entry the same way ([`read_photo`]),
//! so the picture the *user* chooses in Evolution reaches the server. Only the
//! two forms above are read, because they are the two EDS's own writer emits;
//! what a line the reader has to be careful about is spelled out there. The
//! sounds and logos a card carries have no vCard 3.0 property at all and so come
//! back from nothing — the save patches around them, as it does around every
//! other entry the emitter left off.
//!
//! What the save cannot lean on is the [`X_JMAP_KEY`]: unlike a `NICKNAME`'s,
//! the key on a `PHOTO` line does not survive an edit. EDS rebuilds the line out
//! of the photo it holds and writes none of the parameters back, exactly as it
//! does for a date line (measured against libebook-contacts 3.52) — so the entry
//! a chosen picture belongs to is found by pairing rather than by key, which is
//! `jmap-book-sync`'s `diff_media`.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_UNPADDED};
use jmap_proto::contacts::{
    Address, AddressComponent, Anniversary, Calendar, ContactCard, ContactEmail, ContactPhone,
    Link, Media, Name, NameComponent, Nickname, Note, OnlineService, OrgUnit, Organization, Title,
};
use serde_json::{Map, Value, json};

use crate::error::VCardError;
use crate::syntax::{self, Property};

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
const CONTEXTS: [(&str, &str); 2] = [("work", "WORK"), ("private", "HOME")];

/// JSContact phone `features` and their vCard `TYPE` spelling.
const PHONE_FEATURES: [(&str, &str); 5] = [
    ("voice", "VOICE"),
    ("fax", "FAX"),
    ("mobile", "CELL"),
    ("pager", "PAGER"),
    ("video", "VIDEO"),
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

/// The URI scheme each service states its handles under, for the services whose
/// scheme names the handle *literally* — `xmpp:vera@jabber.example` is the JID
/// `vera@jabber.example` with a prefix and nothing else.
///
/// This is what lets an entry stating only a `uri` (RFC 9553 §2.3.2 asks for
/// either that or a `user`) reach the free-text field EDS keeps handles in:
/// without it the handle would have to be guessed out of the URI, and the guess
/// would be written back on the next save.
///
/// Deliberately shorter than [`ONLINE_SERVICES`], because a scheme is only
/// listed here where its scheme-specific part *is* the handle:
///
/// - `xmpp` is RFC 5122 §2.1's, whose path is a JID. Google Talk ran on XMPP,
///   so its handles are JIDs too, and one scheme serves both.
/// - `skype` is the scheme Skype's own links use, where the bare form
///   `skype:<name>` is the Skype Name and the rest is a query telling the client
///   what to do with it — which [`plain_handle`] refuses.
/// - `matrix` is left out: RFC-registered, but it states an identifier as
///   `u/vera:matrix.example` rather than as the `@vera:matrix.example` the field
///   holds, so reading one means rewriting it and writing one means the reverse.
/// - AIM, Gadu-Gadu, ICQ, MSN and Yahoo each had a conventional scheme that this
///   table does not yet name, because none was verified here against the IANA
///   registry. That omission costs exactly what it cost before this table
///   existed — a `uri`-only entry at one of them stays invisible — and adding
///   one is a line of table plus a test.
///
/// Getting a scheme *wrong* is bounded the same way: a URI whose scheme does not
/// match is not drawn, which is the behaviour of every service missing here.
const SERVICE_SCHEMES: [(&str, &str); 3] = [
    ("Google Talk", "xmpp"),
    ("Jabber", "xmpp"),
    ("Skype", "skype"),
];

/// The slot EDS files a handle in when nothing says otherwise, and the only one
/// it writes a handle of its own accord into (measured against
/// libebook-contacts 3.52).
const DEFAULT_SLOT: &str = "HOME";

/// The line EDS keeps `E_CONTACT_ANNIVERSARY` on — the field Evolution's
/// contact editor labels "Anniversary".
///
/// vCard 3.0 has no property for a wedding day: RFC 6474's `ANNIVERSARY` is
/// vCard 4.0, which `e_contact_new_from_vcard()` is not given. Writing the
/// date on any other line would keep it out of the only field that shows it.
const X_EVOLUTION_ANNIVERSARY: &str = "X-EVOLUTION-ANNIVERSARY";

/// JSContact anniversary `kind` values and the vCard property stating each.
///
/// RFC 9553 §2.8.1's third kind, `death`, is missing on purpose: no vCard 3.0
/// property and no EDS field states it, and putting the date on a `BDAY`
/// would tell the user it is a birthday.
const ANNIVERSARY_KINDS: [(&str, &str); 2] =
    [("birth", "BDAY"), ("wedding", X_EVOLUTION_ANNIVERSARY)];

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

/// Whether the vCard mapping covers a JSContact `name.components` kind.
///
/// Anything that saves a card back to the server has to know exactly which
/// JSContact fields a vCard can carry, or it will overwrite the ones it
/// silently dropped on the way in. The predicates below are that knowledge,
/// kept next to the tables they answer for.
pub fn maps_name_component(kind: &str) -> bool {
    name_field(kind).is_some()
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

/// Whether the vCard mapping covers a JSContact phone `features` key.
pub fn maps_phone_feature(key: &str) -> bool {
    PHONE_FEATURES.iter().any(|(mapped, _)| *mapped == key)
}

/// Whether the vCard mapping covers a JSContact address component kind.
pub fn maps_address_component(kind: &str) -> bool {
    address_field(kind).is_some()
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
/// of a kind vCard 3.0 has a property for.
///
/// As with a title, the kind alone is not the question — a plain link with no
/// URI has no `URL` line either, and calling it visible would let a save
/// delete it.
pub fn states_link(link: &Link) -> bool {
    !link.uri.is_empty() && maps_link_kind(link.kind.as_deref())
}

/// Whether the vCard mapping covers a JSContact link of this `kind`.
///
/// Only a link that names no kind at all, which is the plain website RFC 2426
/// §3.6.8's `URL` means. RFC 9553 §2.6.3 defines one kind, `contact` — a URI
/// for writing to the person — and allows vendor kinds besides; RFC 9555
/// §2.6.3 states `contact` on vCard 4.0's `CONTACT-URI`, which the 3.0 reader
/// EDS gives us does not know. Writing either on a `URL` would tell the user
/// it is the contact's home page.
fn maps_link_kind(kind: Option<&str>) -> bool {
    kind.is_none()
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

/// Whether a media entry reaches the user at all: it must be a photo, and the
/// bytes or the URI it names must be something a `PHOTO` line can state.
///
/// [`photo`] is where each of those is spelled out — the single point this and
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
/// Three things have to hold at once, and [`drawn_service`] is where each is
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
///   handle says nothing; a carriage return is dropped by [`crate::syntax::write`]
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
    let scheme = service_scheme(service)?;
    let (stated, handle) = uri.split_once(':')?;
    // Case-insensitively, as RFC 3986 §3.1 requires of a scheme.
    (stated.eq_ignore_ascii_case(scheme) && plain_handle(handle)).then_some(handle)
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
            .extra
            .get("contexts")
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
///   [`edged_with_whitespace`].
///
/// The single point the save and [`drawn_tags`] agree through, so a tag cannot
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
fn read_keywords(properties: &[Property]) -> Option<BTreeMap<String, Value>> {
    let tags: BTreeMap<String, Value> = properties
        .iter()
        .filter(|property| property.name == CATEGORIES)
        .flat_map(Property::items)
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
/// nothing has no `TITLE` line either, and asking only [`maps_title_kind`]
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
    if let Some(utc) = date.get("utc").and_then(Value::as_str) {
        return read_day(utc).map(|day| day.text());
    }
    let day = Day {
        year: member(date, "year")?,
        month: member(date, "month")?,
        day: member(date, "day")?,
    };
    day.is_a_date().then(|| day.text())
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

/// Render a contact card as a vCard 3.0 string, ready for
/// `e_contact_new_from_vcard()`.
pub fn card_to_vcard(card: &ContactCard) -> String {
    let mut properties = vec![Property::new("VERSION", "3.0")];

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
        properties.push(Property::new("UID", uid));
    }
    if let Some(uid) = &card.uid {
        properties.push(Property::new(X_JMAP_UID, uid));
    }

    if let Some(name) = &card.name {
        if let Some(full) = name.full.clone().or_else(|| derive_full(name)) {
            properties.push(Property::new("FN", &full));
        }
        if let Some(fields) = name_fields(name) {
            properties.push(Property::structured("N", fields));
        }
    }

    // One line per entry rather than RFC 2426 §3.1.3's comma-separated list,
    // so that each keeps its JSContact key — and because EDS reads the value
    // as one string either way.
    for (key, nickname) in card.nicknames.iter().flatten() {
        if !states_nickname(nickname) {
            continue;
        }
        properties.push(Property::new("NICKNAME", &nickname.name).with_param(X_JMAP_KEY, key));
    }

    for (key, email) in card.emails.iter().flatten() {
        if !states_email(email) {
            continue;
        }
        let mut types = type_names(&CONTEXTS, email.contexts.as_ref());
        if email.pref.is_some() {
            // vCard 3.0 has no ranking, only a preferred flag.
            types.push("PREF");
        }
        properties.push(
            Property::new("EMAIL", &email.address)
                .with_param(X_JMAP_KEY, key)
                .with_params("TYPE", types),
        );
    }

    for (key, phone) in card.phones.iter().flatten() {
        if !states_phone(phone) {
            continue;
        }
        let mut types = type_names(&CONTEXTS, phone.contexts.as_ref());
        types.extend(type_names(&PHONE_FEATURES, phone.features.as_ref()));
        properties.push(
            Property::new("TEL", &phone.number)
                .with_param(X_JMAP_KEY, key)
                .with_params("TYPE", types),
        );
    }

    for (key, address) in card.addresses.iter().flatten() {
        let types = type_names(&CONTEXTS, address.contexts.as_ref());
        if let Some(fields) = address_fields(address) {
            properties.push(
                Property::structured("ADR", fields)
                    .with_param(X_JMAP_KEY, key)
                    .with_params("TYPE", types.clone()),
            );
        }
        // The same address written out for an envelope, on the line RFC 2426
        // §3.2.2 gives it — directly after its own `ADR`, and on its own when
        // the components are not known and there is no `ADR` to follow.
        if let Some(full) = address_label(address) {
            properties.push(
                Property::new("LABEL", full)
                    .with_param(X_JMAP_KEY, key)
                    .with_params("TYPE", types),
            );
        }
    }

    for (key, organization) in card.organizations.iter().flatten() {
        let Some(components) = organization_components(organization) else {
            continue;
        };
        properties.push(Property::structured("ORG", components).with_param(X_JMAP_KEY, key));
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
        properties.push(Property::new(name, &title.name).with_param(X_JMAP_KEY, key));
    }

    for (key, note) in card.notes.iter().flatten() {
        if !states_note(note) {
            continue;
        }
        properties.push(Property::new("NOTE", &note.note).with_param(X_JMAP_KEY, key));
    }

    for (key, link) in card.links.iter().flatten() {
        if !states_link(link) {
            continue;
        }
        properties.push(Property::new("URL", &link.uri).with_param(X_JMAP_KEY, key));
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
        properties.push(Property::new(name, &calendar.uri).with_param(X_JMAP_KEY, key));
    }

    // The picture the card carries, on the line Evolution shows as the
    // contact's photo — inline where the card holds the bytes, since that is
    // the only form EDS reads a media type off, and a `VALUE=uri` reference
    // where it merely points at them. Both forms and the parameters each takes
    // are measured against libebook-contacts 3.52; see [`photo`] for the
    // entries that get no line at all.
    for (key, media) in card.media.iter().flatten() {
        let property = match photo(media) {
            None => continue,
            Some(Photo::Inline { subtype, base64 }) => Property::new("PHOTO", &base64)
                .with_param(X_JMAP_KEY, key)
                .with_params("TYPE", subtype)
                .with_param("ENCODING", "b"),
            // Without `VALUE=uri` EDS reaches no field at all, so the user is
            // shown no picture rather than one fetched from the URI.
            Some(Photo::Uri(uri)) => Property::new("PHOTO", uri)
                .with_param(X_JMAP_KEY, key)
                .with_param("VALUE", "uri"),
        };
        properties.push(property);
    }

    // One line per service, on the property EDS keeps that service's handles
    // on, and always with a slot: a line carrying no `TYPE` reaches none of the
    // fields Evolution shows.
    for (key, service) in card.online_services.iter().flatten() {
        let Some((property, handle)) = drawn_service(service) else {
            continue;
        };
        properties.push(
            Property::new(property, handle)
                .with_param(X_JMAP_KEY, key)
                .with_param("TYPE", service_slot(service)),
        );
    }

    for (key, anniversary) in card.anniversaries.iter().flatten() {
        let (Some(name), Some(date)) = (
            anniversary_property(&anniversary.kind),
            anniversary_date(anniversary),
        ) else {
            continue;
        };
        properties.push(Property::new(name, &date).with_param(X_JMAP_KEY, key));
    }

    // The whole set on one line, which is all EDS reads, and with no key on it:
    // a tag is its own identity. Empty when every tag the card holds is one the
    // line cannot carry — see [`states_keyword`] — and then there is no line,
    // exactly as for a card with no tags.
    let tags = drawn_tags(card);
    if !tags.is_empty() {
        properties.push(Property::list(CATEGORIES, tags));
    }

    syntax::write(&properties)
}

/// Read a vCard 3.0 string into a contact card.
///
/// The `id` is whatever the vCard's `UID` says, which for a contact
/// Evolution has just created is a locally invented string rather than a
/// JMAP id — the caller knows which case it is in and must drop it before
/// sending a create.
pub fn vcard_to_card(vcard: &str) -> Result<ContactCard, VCardError> {
    let properties = syntax::parse(vcard)?;
    let text = |name: &str| {
        properties
            .iter()
            .find(|property| property.name == name)
            .map(Property::text)
            .filter(|value| !value.is_empty())
    };

    let name = read_name(&properties);
    let mut nicknames = BTreeMap::new();
    let mut emails = BTreeMap::new();
    let mut phones = BTreeMap::new();
    let mut addresses = BTreeMap::new();
    let mut organizations = BTreeMap::new();
    let mut titles = BTreeMap::new();
    let mut notes = BTreeMap::new();
    let mut anniversaries = BTreeMap::new();
    let mut links = BTreeMap::new();
    let mut calendars = BTreeMap::new();
    let mut media = BTreeMap::new();
    let mut online_services = BTreeMap::new();

    for property in &properties {
        match property.name.as_str() {
            "NICKNAME" => {
                // Read as a `text-list` value, because that is what RFC 2426
                // §3.1.3 makes it and what calcard parses it as: a comma the
                // card left unescaped is part of the nickname here, exactly as
                // it is to EDS, rather than a separator that would file the
                // rest of the line as a second nickname.
                let nickname = Nickname {
                    name: property.text_list(),
                    extra: BTreeMap::new(),
                };
                if !states_nickname(&nickname) {
                    continue;
                }
                nicknames.insert(entry_key(property, "k", &nicknames), nickname);
            }
            "EMAIL" => {
                let address = property.text();
                if address.is_empty() {
                    continue;
                }
                let email = ContactEmail {
                    address,
                    contexts: read_flags(&CONTEXTS, property),
                    pref: property.has_type("PREF").then_some(1),
                    ..ContactEmail::default()
                };
                emails.insert(entry_key(property, "e", &emails), email);
            }
            "TEL" => {
                let number = property.text();
                if number.is_empty() {
                    continue;
                }
                let phone = ContactPhone {
                    number,
                    contexts: read_flags(&CONTEXTS, property),
                    features: read_flags(&PHONE_FEATURES, property),
                    ..ContactPhone::default()
                };
                phones.insert(entry_key(property, "p", &phones), phone);
            }
            "ADR" => {
                let Some(address) = read_address(property) else {
                    continue;
                };
                addresses.insert(entry_key(property, "a", &addresses), address);
            }
            "ORG" => {
                let Some(organization) = read_organization(property) else {
                    continue;
                };
                organizations.insert(entry_key(property, "o", &organizations), organization);
            }
            "TITLE" | "ROLE" => {
                let Some(title) = read_title(property) else {
                    continue;
                };
                titles.insert(entry_key(property, "t", &titles), title);
            }
            "NOTE" => {
                let note = Note {
                    note: property.text(),
                    extra: BTreeMap::new(),
                };
                if !states_note(&note) {
                    continue;
                }
                notes.insert(entry_key(property, "n", &notes), note);
            }
            "URL" => {
                // Read as one value, which is what calcard makes of a URI:
                // neither the comma nor the semicolon inside it separates
                // anything, so a query string listing tags arrives as the URI
                // the line stated rather than as a fragment of it.
                let link = Link {
                    uri: property.text(),
                    kind: None,
                    extra: BTreeMap::new(),
                };
                if !states_link(&link) {
                    continue;
                }
                links.insert(entry_key(property, "l", &links), link);
            }
            // Both calendaring lines feed one keyed map, so the line's own name
            // is the only thing that says what kind the entry is — and the keys
            // the reader invents for the two have to be free of each other's.
            "CALURI" | "FBURL" => {
                let uri = property.text();
                if uri.is_empty() {
                    continue;
                }
                let calendar = Calendar {
                    kind: calendar_kind(&property.name).map(str::to_owned),
                    uri,
                    extra: BTreeMap::new(),
                };
                calendars.insert(entry_key(property, "c", &calendars), calendar);
            }
            "PHOTO" => {
                let Some(photo) = read_photo(property) else {
                    continue;
                };
                media.insert(entry_key(property, "m", &media), photo);
            }
            "BDAY" | X_EVOLUTION_ANNIVERSARY => {
                let Some(anniversary) = read_anniversary(property) else {
                    continue;
                };
                anniversaries.insert(entry_key(property, "y", &anniversaries), anniversary);
            }
            // One of the `X-` lines EDS keeps instant-messaging handles on, and
            // nothing else: a line for a service this mapping does not state is
            // left where it is rather than read as an entry the server never
            // had. The `TYPE` is not read — it is the slot, not the contexts.
            name => {
                let Some(service) = service_of(name) else {
                    continue;
                };
                let handle = property.text();
                if handle.is_empty() {
                    continue;
                }
                let entry = OnlineService {
                    service: Some(service.to_owned()),
                    user: Some(handle),
                    uri: None,
                    extra: BTreeMap::new(),
                };
                online_services.insert(entry_key(property, "s", &online_services), entry);
            }
        }
    }

    // The `LABEL` lines after the `ADR` ones, because a label states an
    // address the card may already have named and has to find it first.
    for property in properties
        .iter()
        .filter(|property| property.name == "LABEL")
    {
        let full = property.text();
        if full.is_empty() {
            continue;
        }
        let contexts = read_flags(&CONTEXTS, property);
        let key = label_entry(property, contexts.as_ref(), &addresses);
        addresses
            .entry(key)
            .or_insert_with(|| Address {
                contexts,
                ..Address::default()
            })
            .full = Some(full);
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
        keywords: read_keywords(&properties),
        extra: BTreeMap::new(),
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
/// The bytes come from [`Property::binary`] where the line carried bytes and
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
fn read_photo(property: &Property) -> Option<Media> {
    let states_a_reference = property
        .param("VALUE")
        .is_some_and(|value| value.eq_ignore_ascii_case(URI_VALUE));
    if states_a_reference {
        let uri = property.text();
        // A URI line says what the resource is nowhere: EDS writes no `TYPE` on
        // one and reads none off one, so there is nothing to state.
        return (!uri.is_empty()).then(|| photo_entry(uri, None));
    }

    let bytes = match property.binary() {
        Some(bytes) => bytes.to_vec(),
        None => property.text().into_bytes(),
    };
    if bytes.is_empty() {
        return None;
    }
    let media_type = property
        .param("TYPE")
        .filter(|subtype| !subtype.eq_ignore_ascii_case(UNKNOWN_TYPE))
        .map(|subtype| format!("{IMAGE_PREFIX}{subtype}"));
    let uri = format!(
        "{DATA_SCHEME}{}{BASE64_MARKER},{}",
        media_type.as_deref().unwrap_or_default(),
        BASE64.encode(&bytes)
    );
    Some(photo_entry(uri, media_type))
}

/// A media entry for a picture of the contact — the one kind a `PHOTO` line
/// states, so the only kind this side reads back.
fn photo_entry(uri: String, media_type: Option<String>) -> Media {
    Media {
        kind: Some(PHOTO_KIND.to_owned()),
        uri,
        media_type,
        extra: BTreeMap::new(),
    }
}

/// The title a `TITLE` or `ROLE` line states, or `None` for a line with no
/// text on it.
///
/// The kind is left unsaid when it is the default, so that reading back a
/// card that never named one produces the card that was there — a save then
/// has nothing to patch.
fn read_title(property: &Property) -> Option<Title> {
    let name = property.text();
    if name.is_empty() {
        return None;
    }
    let kind = TITLE_KINDS
        .iter()
        .find(|(_, mapped)| *mapped == property.name)
        .map(|(kind, _)| *kind)
        .filter(|kind| *kind != DEFAULT_TITLE_KIND);
    Some(Title {
        name,
        kind: kind.map(str::to_owned),
        extra: BTreeMap::new(),
    })
}

/// The anniversary a date line states, or `None` for a line no calendar day
/// can be read out of.
///
/// The kind is the line's own: a `BDAY` states a birthday and nothing else,
/// so unlike a title's it is never guessed at and never left unsaid.
fn read_anniversary(property: &Property) -> Option<Anniversary> {
    let (kind, _) = ANNIVERSARY_KINDS
        .iter()
        .find(|(_, name)| *name == property.name)?;
    Some(Anniversary {
        kind: (*kind).to_owned(),
        date: Some(read_day(&property.text())?.json()),
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
/// the world's addresses. See [`restore_shared_fields`] for what is then done
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
/// field holds them both. The treatment is [`restore_shared_fields`]', the same
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

/// The address an `ADR` line states, or `None` when every field of it is
/// empty — the same "nothing was said" an `EMAIL:` with no address is.
fn read_address(property: &Property) -> Option<Address> {
    let fields = property.components();
    let mut components = Vec::new();
    for (kind, index) in ADDRESS_COMPONENTS {
        let Some(value) = fields.get(index).filter(|value| !value.is_empty()) else {
            continue;
        };
        components.push(AddressComponent::new(kind, value));
    }
    if components.is_empty() {
        return None;
    }
    Some(Address {
        components: Some(components),
        contexts: read_flags(&CONTEXTS, property),
        // Filled in by the `LABEL` line, if the card has one for this address.
        full: None,
        extra: BTreeMap::new(),
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
fn read_organization(property: &Property) -> Option<Organization> {
    let components = property.components();
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
    property: &Property,
    contexts: Option<&Value>,
    addresses: &BTreeMap<String, Address>,
) -> String {
    let unlabelled = |address: &Address| address.full.is_none();
    if let Some(key) = property.param(X_JMAP_KEY).filter(|key| !key.is_empty())
        && addresses.get(key).is_none_or(unlabelled)
    {
        return key.to_owned();
    }
    if let Some((key, _)) = addresses
        .iter()
        .find(|(_, address)| unlabelled(address) && address.contexts.as_ref() == contexts)
    {
        return key.clone();
    }
    entry_key(property, "a", addresses)
}

/// The JSContact map key for an entry: the one we round-tripped, or the
/// first free `e1`, `e2`, … for a vCard that never had one.
fn entry_key<T>(property: &Property, prefix: &str, taken: &BTreeMap<String, T>) -> String {
    if let Some(key) = property.param(X_JMAP_KEY).filter(|key| !key.is_empty())
        && !taken.contains_key(key)
    {
        return key.to_owned();
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

fn read_name(properties: &[Property]) -> Option<Name> {
    let find = |name: &str| properties.iter().find(|property| property.name == name);

    let full = find("FN").map(Property::text).filter(|f| !f.is_empty());
    let fields = find("N").map(Property::components).unwrap_or_default();

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

    // No FN and no usable N: the vCard simply does not name anybody. Note
    // that a missing N is never guessed at by splitting FN — a wrong guess
    // would be written back to the server on the next save.
    if full.is_none() && components.is_empty() {
        return None;
    }
    Some(Name {
        components: (!components.is_empty()).then_some(components),
        full,
        extra: BTreeMap::new(),
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

/// The JSContact boolean map for the `TYPE` values present on `property`.
fn read_flags(table: &[(&str, &str)], property: &Property) -> Option<Value> {
    let flags: Map<String, Value> = table
        .iter()
        .filter(|(_, type_name)| property.has_type(type_name))
        .map(|(key, _)| ((*key).to_owned(), Value::Bool(true)))
        .collect();
    (!flags.is_empty()).then_some(Value::Object(flags))
}
