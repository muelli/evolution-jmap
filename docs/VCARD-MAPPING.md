<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# vCard ↔ JSContact ↔ EDS Contact Mapping Reference

This document is the authoritative reference specification for contact data translation across:
1. **vCard 3.0 / 4.0** (RFC 2426, RFC 6350, RFC 6474, RFC 6473) as parsed and emitted via `calcard`.
2. **JSContact** (RFC 9553 / RFC 9555) and **JMAP for Contacts** (RFC 9610) as modeled in `jmap-proto`'s [`ContactCard`].
3. **Evolution Data Server (EDS)** (`libebook-contacts` 3.52) as defined in `eds-sys`'s [`EContactField`] enum (`E_CONTACT_*`).

All implementation logic resides in `rust/crates/jmap-vcard/src/contact.rs`.

---

## 1. Architecture & Design Principles

### 1.1 Three-Tier Mapping Architecture

```
┌────────────────────────────────────────┐
│     JMAP Server (RFC 9610 / 9553)      │
│          JSContact ContactCard         │
└───────────────────▲────────────────────┘
                    │
                    │ PatchObject sync (jmap-book-sync)
                    │ (only mapped/edited fields patched)
                    │
┌───────────────────▼────────────────────┐
│      jmap-vcard (contact.rs)           │
│  card_to_vcard()  /  vcard_to_card()   │
└───────────────────▲────────────────────┘
                    │
                    │ vCard 3.0 wire format (calcard)
                    │
┌───────────────────▼────────────────────┐
│   Evolution Data Server (EDS 3.52)     │
│   e_contact_new_from_vcard() / e-book  │
│        E_CONTACT_* Fields / UI         │
└────────────────────────────────────────┘
```

### 1.2 Core Invariants

1. **Selective Mapping & Sync Safety**:
   `jmap-vcard` deliberately maps only the property set that Evolution's address book backend needs to present in UI and edit. Everything else on a JSContact card (e.g., `preferredLanguages`, `localizations`, `cryptoKeys`, `pronouns`, `gender`, unsupported relations) is dropped on vCard emission. This is safe because `jmap-book-sync` saves changes back to the JMAP server using `PatchObject` specifying only mapped and edited paths. Unmapped server properties are never overwritten or deleted.
2. **Predicates Safeguard Server State**:
   Absence of a field from an edited vCard is only interpreted as user deletion if the field was originally eligible for display. Emitter predicates (e.g., [`states_context`], [`states_phone_feature`], [`states_address_component`], [`states_spouse`], [`states_keyword`]) explicitly answer whether a property was visible to the user.
3. **Keying & Identity Preservation**:
   Every multi-valued JSContact entry carries an `X-JMAP-KEY` parameter in vCard 3.0 format. On round-tripping, [`entry_key`] recovers the server key or allocates a deterministic key (`e1`, `p1`, `a1`, etc.) for newly added entries.
4. **Deterministic Fixed-Point Stability**:
   Property transformations reach fixed-point convergence under repeated serialization/deserialization: `card_to_vcard(vcard_to_card(card_to_vcard(c))) == card_to_vcard(c)`.

---

## 2. Master Property Mapping Table

| vCard Property | vCard Parameters | JSContact Field (RFC 9553/9555) | EDS Field (`EContactField`) | Primary Helpers & Predicates | Lossy / Product Decision Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`UID`** | — | `card.id` (or `card.uid`) | `E_CONTACT_UID` | `card_to_vcard`, `vcard_to_card` | `UID` carries server JMAP ID for EDS cache indexing; `X-JMAP-UID` carries original JSContact UUID. |
| **`X-JMAP-UID`** | — | `card.uid` | — | `card_to_vcard`, `vcard_to_card` | Retains client-side JSContact UUID across vCard round-trips. |
| **`FN`** | `ALTID`, `LANGUAGE` | `card.name.full` | `E_CONTACT_FULL_NAME` | `derive_full`, `read_name` | First `FN` in document order is selected; synthesized from `name.components` if omitted. |
| **`N`** | `ALTID`, `LANGUAGE` | `card.name.components` (`surname`, `given`, `given2`, `title`, `credential`) | `E_CONTACT_FAMILY_NAME`, `E_CONTACT_GIVEN_NAME`, `E_CONTACT_ADDITIONAL_NAME`, `E_CONTACT_NAME_PREFIX`, `E_CONTACT_NAME_SUFFIX` | `name_fields`, `read_name`, [`states_name_component`], [`restore_name_components`] | 5-component structured name. Double-barrelled given names share index 1 and are reconstructed via `restore_name_components`. Name `phonetic` is dropped. |
| **`NICKNAME`** | `X-JMAP-KEY`, `ALTID` | `card.nicknames` (`Nickname.name`) | `E_CONTACT_NICKNAME` | `states_nickname`, `entry_text_list` | Emitted as one line per keyed entry (not comma-separated) to preserve `X-JMAP-KEY`. Commas in nicknames are preserved literally. |
| **`EMAIL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.emails` (`ContactEmail.address`, `contexts`, `pref`) | `E_CONTACT_EMAIL_1` .. `_4` | `states_email`, `type_names`, `read_flags` | Positional filing in EDS (`EMAIL_1` is primary). Sorted on emission by `(pref, key)` so lowest `pref` lands in `EMAIL_1`. Multiple contexts allowed. |
| **`TEL`** | `TYPE` (`WORK`, `HOME`, `CELL`, `MOBILE`, `PAGER`, `FAX`, `VOICE`, `VIDEO`, `PREF`), `X-JMAP-KEY` | `card.phones` (`ContactPhone.number`, `contexts`, `features`, `pref`) | `E_CONTACT_PHONE_PRIMARY`, `_BUSINESS`, `_BUSINESS_2`, `_BUSINESS_FAX`, `_HOME`, `_HOME_2`, `_HOME_FAX`, `_MOBILE`, `_PAGER`, `_OTHER`, `_OTHER_FAX`, `_CAR`, `_ISDN`, `_CALLBACK`, `_COMPANY`, `_RADIO`, `_TELEX`, `_TTYTDD`, `_ASSISTANT` | `states_phone`, `context_slot`, [`states_context`], `feature_slot`, [`states_phone_feature`] | Full 19-field EDS matrix. Inbound accepts `MOBILE` synonym for `CELL`; outbound normalizes to `CELL`. Context narrowed to at most 1 (`WORK` > `HOME` -> `DEFAULT_SLOT`); feature narrowed to at most 1 (`CELL`/`MOBILE` > `PAGER` > `FAX` > `VOICE` > `VIDEO`). Unstated features preserved by predicates. |
| **`ADR`** | `TYPE` (`WORK`, `HOME`, `PREF`), `LABEL`, `X-JMAP-KEY` | `card.addresses` (`Address.components`, `contexts`, `extra["pref"]`) | `E_CONTACT_ADDRESS_WORK`, `_HOME`, `_OTHER` (+ 7 subfields per slot) | `address_fields`, `read_address`, `states_address`, [`states_address_component`], [`restore_address_components`] | 7 components: PO Box, Ext, Street, Locality, Region, Postcode, Country. House `number` joins street `name`. `restore_address_components` reconstructs split components. Unmapped kinds (`floor`, `room`) dropped. |
| **`LABEL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.addresses` (`Address.full`) | `E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER` | `address_label`, `label_entry`, `read_address` | Standalone line emitted after `ADR` or on its own. Inbound matched by `X-JMAP-KEY` or context/text fallback to prevent duplicate addresses. |
| **`ORG`** | `X-JMAP-KEY` | `card.organizations` (`Organization.name`, `units`) | `E_CONTACT_ORG` (name), `E_CONTACT_ORG_UNIT` (dept), `E_CONTACT_OFFICE` (office) | `organization_components`, `read_organization`, [`states_organization`], [`states_org_unit`] | Semicolon-delimited list. Index 0 = Name; Index 1 = Department; Index 2 = Office; Index 3+ = trailing units. Nameless orgs retain leading semicolon (`ORG:;Unit`). `sortAs` and `contexts` unmapped on vCard 3.0. |
| **`TITLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "title"`) | `E_CONTACT_TITLE` | `read_title`, [`states_title`], [`title_kind`] | `kind: "title"` (or `None`) maps to `TITLE`. Vendor kinds dropped. |
| **`ROLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "role"`) | `E_CONTACT_ROLE` | `read_title`, [`states_title`], [`title_kind`] | `kind: "role"` maps to `ROLE`. |
| **`NOTE`** | `X-JMAP-KEY` | `card.notes` (`Note.note`) | `E_CONTACT_NOTE` | `states_note` | Free text. First line lands in EDS `E_CONTACT_NOTE`. RFC 9553 `created` and `author` ride in `extra` and are untouched during sync. |
| **`URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: None`) | `E_CONTACT_HOMEPAGE_URL` | `states_link`, `maps_link_kind` | Plain websites (`kind: None`) map to `URL`. `mediaType`, `label`, `contexts`, `pref` ride in `extra`. |
| **`X-EVOLUTION-BLOG-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "blog"`) | `E_CONTACT_BLOG_URL` | `states_link`, `maps_link_kind` | EDS blog URL field. Maps to `links` with `kind: "blog"`. |
| **`X-EVOLUTION-VIDEO-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "video"`) | `E_CONTACT_VIDEO_URL` | `states_link`, `maps_link_kind` | EDS video stream URL field. Maps to `links` with `kind: "video"`. |
| **`CALURI`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "calendar"`) | `E_CONTACT_CALENDAR_URI` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. `ICSCALENDAR` excluded. |
| **`FBURL`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "freeBusy"`) | `E_CONTACT_FREEBUSY_URL` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. |
| **`PHOTO`** | `TYPE`, `ENCODING=b`, `VALUE=uri`, `X-JMAP-KEY` | `card.media` (`Media.uri`, `media_type`, `kind: "photo"`) | `E_CONTACT_PHOTO` | `photo`, `read_photo`, [`states_media`], [`same_photo`], `image_subtype` | Only `kind: "photo"` mapped. Inline data uses base64; `TYPE` states subtype only (e.g. `JPEG` -> `image/jpeg`). URI references use `VALUE=uri`. Re-paired via `same_photo` since EDS drops `X-JMAP-KEY` on photo edit. |
| **`CATEGORIES`** | — | `card.keywords` (`Set<String>`) | `E_CONTACT_CATEGORY_LIST` | `drawn_tags`, `read_keywords`, [`states_keyword`] | Single sorted line emitted. Comma-separated on wire. Trimming protection: tags with leading/trailing whitespace or carriage returns omitted from emission to prevent EDS corruption. |
| **`BDAY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "birth"`, `date`) | `E_CONTACT_BIRTH_DATE` | `read_anniversary`, [`states_anniversary`], [`anniversary_date`], [`states_a_point_in_time`], `Day` | Single calendar day formatted `YYYY-MM-DD`. Truncated/bare years (`1984`) or years < 1000 omitted to prevent EDS clamping corruption (`1000..=9999`). `Timestamp` converted to UTC day. |
| **`X-EVOLUTION-ANNIVERSARY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, [`states_anniversary`], [`anniversary_date`] | EDS wedding anniversary field. Same date validation and year >= 1000 clamping rules as `BDAY`. `kind: "death"` dropped. |
| **`X-EVOLUTION-SPOUSE`** | — | `card.related_to` (Key = Person Name, `relation: { "spouse": true }`) | `E_CONTACT_SPOUSE` | [`states_spouse`], [`states_nothing_but_the_marriage`], `names_a_person` | Key in JSContact `related_to` is the spouse name (RFC 9555 §2.9.5). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-EVOLUTION-MANAGER`** | — | `card.related_to` (Key = Person Name, `relation: { "manager": true }`) | `E_CONTACT_MANAGER` | [`states_manager`], `names_a_person` | Key in JSContact `related_to` is the manager name (RFC 9553 §2.1.8). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-EVOLUTION-ASSISTANT`** | — | `card.related_to` (Key = Person Name, `relation: { "assistant": true }`) | `E_CONTACT_ASSISTANT` | [`states_assistant`], `names_a_person` | Key in JSContact `related_to` is the assistant name (RFC 9553 §2.1.8). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-AIM`** | `TYPE` (`WORK`, `HOME`), `X-JMAP-KEY` | `card.online_services` (`service: "AIM"`, `user`, `uri: "aim:..."`) | `E_CONTACT_IM_AIM_HOME_1..3`, `_WORK_1..3` | `drawn_service`, [`states_online_service`], [`online_service_handle`], [`online_service_uri`], `service_slot` | 6 EDS slots per service. Handles extracted from `user` or bare URI schemes (`SERVICE_SCHEMES`). Action/query URIs rejected. `TYPE` mandatory for EDS field visibility. |
| **`X-GADUGADU`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Gadu-Gadu"`, `uri: "gg:..."`) | `E_CONTACT_IM_GADUGADU_HOME_1..3`, `_WORK_1..3` | (same as above) | Matched case/punctuation-insensitively (`same_service`). |
| **`X-GOOGLE-TALK`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Google Talk"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_GOOGLE_TALK_HOME_1..3`, `_WORK_1..3` | (same as above) | Handles use `xmpp` scheme. |
| **`X-GROUPWISE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "GroupWise"`, `uri: "groupwise:..."`) | `E_CONTACT_IM_GROUPWISE_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare handles mapped. |
| **`X-ICQ`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "ICQ"`, `uri: "icq:..."`) | `E_CONTACT_IM_ICQ_HOME_1..3`, `_WORK_1..3` | (same as above) | Numeric UIN handles. |
| **`X-JABBER`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Jabber"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_JABBER_HOME_1..3`, `_WORK_1..3` | (same as above) | XMPP JID handles. |
| **`X-MSN`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "MSN"`, `uri: "msn:... / msnim:..."`) | `E_CONTACT_IM_MSN_HOME_1..3`, `_WORK_1..3` | (same as above) | Supports `msn` and `msnim` schemes. |
| **`X-MATRIX`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Matrix"`, `uri: "matrix:..."`) | `E_CONTACT_IM_MATRIX_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Matrix handles (`matrix:@user:domain` without action queries). |
| **`X-SKYPE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Skype"`, `uri: "skype:..."`) | `E_CONTACT_IM_SKYPE_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Skype usernames (`skype:echo123?call` action rejected). |
| **`X-YAHOO`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Yahoo"`, `uri: "yahoo:... / ymsgr:..."`) | `E_CONTACT_IM_YAHOO_HOME_1..3`, `_WORK_1..3` | (same as above) | Supports `yahoo` and `ymsgr` schemes. |
| **`GEO`** | — | `Address.coordinates` (RFC 9553) | `E_CONTACT_GEO` (no UI) | — | Dropped by design on vCard 3.0 import/export. Evolution has no UI for coordinates. Server-side `Address.coordinates` preserved by `PatchObject`. |
| **`TZ`** | — | `card.time_zone` (RFC 9553) | — | — | Dropped by design on vCard 3.0 import/export. Evolution has no per-contact timezone field. Server `time_zone` preserved by `PatchObject`. |
| **`MAILER`** | — | — | `E_CONTACT_MAILER` (legacy) | — | Dropped by design. Deprecated in RFC 6350 (vCard 4.0). Legacy email client software metadata. |
| **`PRODID`** | — | `card.prod_id` (RFC 9553) | — | — | Dropped by design on import/export. Generator metadata belongs to serialization envelope; foreign `PRODID` not preserved across saves. |
| **`REV`** | — | `card.updated` (RFC 9553) | `E_CONTACT_REV` | — | Dropped by design on import/export. Revision timestamp is strictly owned by the JMAP server upon commit. |
| **`SORT-STRING`** | — | `Name.sortAs` / `Org.sortAs` | — | — | Dropped by design from vCard 3.0 emission. Replaced in RFC 6350 by `SORT-AS` parameter. JSContact `sortAs` preserved on server by `PatchObject` without clobbering `fileAs`. |
| **`X-EVOLUTION-FILE-AS`** | — | `Name.extra["fileAs"]` / `card.extra["fileAs"]` | `E_CONTACT_FILE_AS` | `states_file_as` | Evolution "File Under" field. Inbound accepts `X-EVOLUTION-FILE-AS`, `FILE-AS`, and `X-FILE-AS`. Outbound normalizes to `X-EVOLUTION-FILE-AS`. Coexists with `sortAs` without clobbering. |
| **`CLASS`** | — | `card.privacy` (RFC 9553) | — | — | Dropped by design. Deprecated/removed in RFC 6350. Legacy access classification with no Evolution editor UI. |
| **`SOUND`** | `TYPE`, `ENCODING=b`, `VALUE=uri` | `card.media` (`kind: "sound"`) | — | [`states_media`] | Dropped by design from vCard 3.0. [`states_media`] permits only `kind: "photo"`. Server `sound` media entries preserved by `PatchObject`. |
| **`LOGO`** | `TYPE`, `ENCODING=b`, `VALUE=uri` | `card.media` (`kind: "logo"`) | `E_CONTACT_LOGO` (no UI) | [`states_media`] | Dropped by design from vCard 3.0. Evolution editor supports only personal photo (`E_CONTACT_PHOTO`). Server `logo` entries preserved by `PatchObject`. |
| **`itemN.PROPERTY`** | `X-ABLabel` companion | (associated property) | (associated EDS slot) | `clean_apple_label`, `vcard_to_card` | Apple property groups (RFC 2426 §2.1.1). Group prefix is parsed by `calcard`; companion `X-ABLabel` maps contexts (`Work`, `Home`), features (`Mobile`, `Pager`, `Fax`), or custom labels. |
| **`X-ABLabel`** | — | `extra["label"]` or mapped context/feature | — | `clean_apple_label`, `vcard_to_card` | Apple label annotation. Markers (`_$!<Label>!$_`) unwrapped. Standard labels map to JSContact contexts/features; custom labels preserved in `extra["label"]`. |
| **`X-ABRELATEDNAMES`** | `X-ABLabel` | `card.related_to` (Key = Person Name) | `E_CONTACT_SPOUSE`, `_MANAGER`, `_ASSISTANT` | `clean_apple_label`, `vcard_to_card` | Apple relationship property. Group companion `X-ABLabel` selects relation type (`spouse`, `manager`, `assistant`, custom). Outbound normalizes to standard `X-EVOLUTION-*`. |
| **`X-ABDATE`** | `X-ABLabel` | `card.anniversaries` (`kind`, `date`) | `E_CONTACT_ANNIVERSARY` | `clean_apple_label`, `read_day` | Apple date property. Group companion `X-ABLabel` selects anniversary kind (`wedding`, `birth`, custom). Outbound normalizes to `X-EVOLUTION-ANNIVERSARY` / `BDAY`. |

---

## 3. Detailed Field & Subsystem Specifications

### 3.1 Identifiers & UIDs
- **`UID`**: vCard 3.0 standard identifier. Maps to `card.id` (JMAP contact ID) or fallback to `card.uid`. EDS indexes its internal SQLite cache by `UID`.
- **`X-JMAP-UID`**: Proprietary parameter preserving `card.uid` (JSContact UUID) when distinct from the JMAP server ID.
- **`X-JMAP-KEY`**: Added as a parameter on multi-valued entries (`EMAIL`, `TEL`, `ADR`, `LABEL`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `CALURI`, `FBURL`, `PHOTO`, `online_services`, `anniversaries`). Allows lossless synchronization back to JSContact map keys.
- **Local Invention Stripping**: When Evolution creates a contact, it assigns a local temporary UID. `jmap-book-sync` strips this local UID before issuing a JMAP `ContactCard/set create` call.

### 3.2 Names (`FN`, `N`, `NICKNAME`)
- **`FN`**: Required by RFC 2426. Parsed by `read_name`. If absent on outbound serialization, `derive_full` constructs a formatted name from `name.components` in reading order (`title` -> `given` -> `given2` -> `surname` -> `credential`).
- **`N` (5 Components)**: Structured name array corresponding to `[surname, given, given2, title, credential]`.
  - Double-barrelled given names (`given: "Jean"`, `given: "Luc"`) are joined with a space in `N[1]` on emission.
  - `restore_name_components` compares edited text against joined parts; if identical, it restores individual JSContact components; if edited, it preserves the user's single updated string.
  - `phonetic` name components (RFC 9553 §2.2.1) are omitted on vCard 3.0 and kept intact on the server.

### 3.3 Electronic Communication (`EMAIL`, `TEL`)
- **`EMAIL` Positional Filing & Attribute List**:
  - Evolution files `EMAIL` lines by position into `E_CONTACT_EMAIL_1` .. `_4` (fields 8..11), with additional lines (5+) and all entries maintained in the `E_CONTACT_EMAIL` (field 97) `GList` attribute list.
  - `card_to_vcard` sorts emails by `(pref.unwrap_or(u32::MAX), key)` to guarantee the primary preferred address (`pref: 1` or lowest rank) lands on the first `EMAIL` line (`E_CONTACT_EMAIL_1`).
  - Unranked emails (`pref: None`) follow preferred emails in deterministic key order.
  - Inbound unkeyed vCards allocate sequential keys `e1`, `e2`, `e3`, `e4`, `e5`, ... preserving document order.

#### Master EDS Email Mapping Matrix (4 Slots + Attribute List)

| EDS `EContactField` (ID) | Evolution UI Slot | Inbound vCard `EMAIL` | JSContact `ContactEmail` | Outbound vCard 3.0 | Slot Resolution & Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `E_CONTACT_EMAIL_1` (8) | Primary Email | 1st `EMAIL` line | Lowest `pref` (e.g. `pref: 1`) or 1st key | `EMAIL;X-JMAP-KEY=...;TYPE=PREF:...` | Primary email in Evolution editor. Promoted by `(pref, key)` sorting. |
| `E_CONTACT_EMAIL_2` (9) | Email 2 | 2nd `EMAIL` line | 2nd `emails` entry | `EMAIL;X-JMAP-KEY=...:...` | Secondary email field in Evolution editor. |
| `E_CONTACT_EMAIL_3` (10) | Email 3 | 3rd `EMAIL` line | 3rd `emails` entry | `EMAIL;X-JMAP-KEY=...:...` | Tertiary email field in Evolution editor. |
| `E_CONTACT_EMAIL_4` (11) | Email 4 | 4th `EMAIL` line | 4th `emails` entry | `EMAIL;X-JMAP-KEY=...:...` | Quaternary email field in Evolution editor. |
| `E_CONTACT_EMAIL` (97) | Email Attribute List | All `EMAIL` lines (1..=4 and 5+) | `card.emails` (`BTreeMap`) | All `EMAIL;X-JMAP-KEY=...` lines | Full list of all email addresses. Lines beyond 4 are safely preserved on wire format and server. |

- **`TEL` Slot Narrowing & Complete EDS Phone Matrix**:
  - EDS defines 19 distinct phone fields (`E_CONTACT_FIRST_PHONE_ID` 16 to `E_CONTACT_LAST_PHONE_ID` 34) in `libebook-contacts`. EDS matches incoming vCard lines to fields by evaluating their `TYPE` parameters.
  - **Context Narrowing**: EDS matches `TYPE` sets to fields. A line carrying `TYPE=WORK,HOME` would satisfy both `E_CONTACT_PHONE_BUSINESS` and `E_CONTACT_PHONE_HOME`, causing one phone number to occupy two separate UI blocks that overwrite each other on edit. `context_slot` chooses exactly one slot: `WORK` > `HOME` -> `DEFAULT_SLOT` (`HOME`).
  - **Feature Narrowing**: `feature_slot` ranks phone features: `mobile` (`CELL`/`MOBILE`) > `pager` (`PAGER`) > `fax` (`FAX`) > `voice` (`VOICE`) > `video` (`VIDEO`).
  - **Synonym Normalization**: Real-world vCard generators (Android, iOS, Outlook, feature phones) widely emit `TYPE=MOBILE` in place of standard vCard 3.0 `TYPE=CELL`. Inbound parsing (`read_phone_flags`) accepts both `CELL` and `MOBILE` as `features: {"mobile": true}`; outbound emission normalizes to standard `TYPE=CELL`.
  - **Predicate Safeguards**: `states_context` and `states_phone_feature` ensure omitted contexts/features are not considered deleted by the user when computing diffs (`jmap-book-sync`).

#### Master EDS Phone Mapping Matrix (19 Fields)

| EDS `EContactField` (ID) | Evolution UI Slot | Inbound vCard `TEL;TYPE=` | JSContact `ContactPhone` | Outbound vCard 3.0 | Slot Resolution & Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `E_CONTACT_PHONE_PRIMARY` (31) | Primary Phone | `TEL;TYPE=PREF` | `pref: Some(1)` | `TEL;...;TYPE=PREF` | Sorted first on emission by `(pref, key)` so lowest `pref` populates the primary slot. |
| `E_CONTACT_PHONE_BUSINESS` (17) | Business Phone | `TEL;TYPE=WORK,VOICE` or `TEL;TYPE=WORK` | `contexts: {"work": true}`, `features: {"voice": true}` | `TEL;TYPE=WORK,VOICE` | Primary work voice number. Default feature when context is work and feature omitted. |
| `E_CONTACT_PHONE_BUSINESS_2` (18) | Business Phone 2 | 2nd `TEL;TYPE=WORK` | 2nd `phones` entry with `contexts: {"work": true}` | 2nd `TEL;TYPE=WORK` | Positional secondary work phone; emitted in sorted key order. |
| `E_CONTACT_PHONE_BUSINESS_FAX` (19) | Business Fax | `TEL;TYPE=WORK,FAX` | `contexts: {"work": true}`, `features: {"fax": true}` | `TEL;TYPE=WORK,FAX` | Work fax line. |
| `E_CONTACT_PHONE_HOME` (23) | Home Phone | `TEL;TYPE=HOME,VOICE` or `TEL;TYPE=HOME` | `contexts: {"private": true}`, `features: {"voice": true}` | `TEL;TYPE=HOME,VOICE` | Primary private voice number. Default feature when context is private and feature omitted. |
| `E_CONTACT_PHONE_HOME_2` (24) | Home Phone 2 | 2nd `TEL;TYPE=HOME` | 2nd `phones` entry with `contexts: {"private": true}` | 2nd `TEL;TYPE=HOME` | Positional secondary home phone; emitted in sorted key order. |
| `E_CONTACT_PHONE_HOME_FAX` (25) | Home Fax | `TEL;TYPE=HOME,FAX` | `contexts: {"private": true}`, `features: {"fax": true}` | `TEL;TYPE=HOME,FAX` | Private fax line. |
| `E_CONTACT_PHONE_MOBILE` (27) | Mobile Phone | `TEL;TYPE=CELL` or `TEL;TYPE=MOBILE` | `features: {"mobile": true}` (+ optional context) | `TEL;TYPE=CELL` (or `TYPE=WORK,CELL` / `HOME,CELL`) | Mobile phone. Inbound accepts `MOBILE` synonym; outbound normalizes to RFC 2426 `CELL`. |
| `E_CONTACT_PHONE_PAGER` (30) | Pager | `TEL;TYPE=PAGER` | `features: {"pager": true}` (+ optional context) | `TEL;TYPE=PAGER` (or `TYPE=WORK,PAGER` / `HOME,PAGER`) | Pager device. Outranks `voice`/`fax` in feature slotting. |
| `E_CONTACT_PHONE_OTHER` (28) | Other Phone | `TEL;TYPE=VOICE` or bare `TEL:` | `contexts: None`, `features: {"voice": true}` (or `None`) | `TEL;TYPE=VOICE` or bare `TEL:` | Unqualified voice line or bare phone without context/feature. |
| `E_CONTACT_PHONE_OTHER_FAX` (29) | Other Fax | `TEL;TYPE=FAX` | `contexts: None`, `features: {"fax": true}` | `TEL;TYPE=FAX` | Unqualified fax line without work/private context. |
| `E_CONTACT_PHONE_CAR` (21) | Car Phone | `TEL;TYPE=CAR` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | RFC 2426 §3.3.1 car phone. Carried with key across round-trips; server properties untouched by `PatchObject`. |
| `E_CONTACT_PHONE_ISDN` (26) | ISDN Phone | `TEL;TYPE=ISDN` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | RFC 2426 §3.3.1 ISDN line. Carried with key across round-trips. |
| `E_CONTACT_PHONE_CALLBACK` (20) | Callback Phone | `TEL;TYPE=CALLBACK` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | EDS callback phone extension. Carried with key across round-trips. |
| `E_CONTACT_PHONE_COMPANY` (22) | Company Phone | `TEL;TYPE=COMPANY` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | Company switchboard main number. Carried with key across round-trips. |
| `E_CONTACT_PHONE_RADIO` (32) | Radio Phone | `TEL;TYPE=RADIO` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | EDS radio phone extension. Carried with key across round-trips. |
| `E_CONTACT_PHONE_TELEX` (33) | Telex Phone | `TEL;TYPE=TELEX` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | EDS telex terminal extension. Carried with key across round-trips. |
| `E_CONTACT_PHONE_TTYTDD` (34) | TTY/TDD Phone | `TEL;TYPE=TTYTDD` or `TEL;TYPE=TTY` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | TTY/TDD text telephone device for the hearing impaired. Carried with key. |
| `E_CONTACT_PHONE_ASSISTANT` (16) | Assistant Phone | `TEL;TYPE=ASSISTANT` | `ContactPhone.number` | `TEL;X-JMAP-KEY=...` | Executive assistant telephone line. Carried with key across round-trips. |


### 3.4 Postal Addresses (`ADR`, `LABEL`)
- **7 Structured Components**:
  - `0`: Post Office Box (`postOfficeBox`) -> `E_CONTACT_ADDRESS_*_PO_BOX`
  - `1`: Extended Address / Apartment (`apartment`) -> `E_CONTACT_ADDRESS_*_EXTENDED`
  - `2`: Street Name & House Number (`name`, joined with `number`) -> `E_CONTACT_ADDRESS_*_STREET`
  - `3`: Locality / City (`locality`) -> `E_CONTACT_ADDRESS_*_LOCALITY`
  - `4`: Region / State (`region`) -> `E_CONTACT_ADDRESS_*_REGION`
  - `5`: Postal Code (`postcode`) -> `E_CONTACT_ADDRESS_*_CODE`
  - `6`: Country (`country`) -> `E_CONTACT_ADDRESS_*_COUNTRY`
- **Street & Number Joining**: `JOINED_COMPONENTS` pairs `number` with `name`. `restore_address_components` restores discrete street and number components if the joined string was not altered in Evolution.
- **`LABEL` Pairing & Synthetic EDS Fields**:
  - In vCard 3.0, `LABEL` is a standalone property; in vCard 4.0, it is a parameter on `ADR`. `read_address` parses `ADR;LABEL=...` parameters directly, and `label_entry` pairs standalone `LABEL` lines with their preceding `ADR` entries using `X-JMAP-KEY` or context matching.
  - EDS models address labels as synthetic string fields (`E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER`). When EDS serializes a contact, it emits standalone `LABEL;TYPE=...` lines matching the address slots.
  - In-place modifications to synthetic label fields in EDS update the label text while retaining `X-JMAP-KEY` and context pairing.

#### Master EDS Address & Label Mapping Matrix (3 Slots + 3 Synthetic Labels)

| EDS Address Slot | Structured `EContactField` (ID) | Synthetic Label Field (ID) | Inbound `ADR` / `LABEL` | JSContact `Address` | Outbound vCard 3.0 | Resolution & Notes |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Work Address** | `E_CONTACT_ADDRESS_WORK` (5) + Subfields 42..48 | `E_CONTACT_ADDRESS_LABEL_WORK` (14) | `ADR;TYPE=WORK:...` / `LABEL;TYPE=WORK:...` | `contexts: {"work": true}`, `components`, `full` | `ADR;TYPE=WORK` + `LABEL;TYPE=WORK` | Primary business postal address and envelope label. |
| **Home Address** | `E_CONTACT_ADDRESS_HOME` (4) + Subfields 35..41 | `E_CONTACT_ADDRESS_LABEL_HOME` (13) | `ADR;TYPE=HOME:...` / `LABEL;TYPE=HOME:...` | `contexts: {"private": true}`, `components`, `full` | `ADR;TYPE=HOME` + `LABEL;TYPE=HOME` | Primary private postal address and envelope label. |
| **Other Address** | `E_CONTACT_ADDRESS_OTHER` (6) + Subfields 49..55 | `E_CONTACT_ADDRESS_LABEL_OTHER` (15) | `ADR;TYPE=OTHER:...` (or bare) / `LABEL;TYPE=OTHER:...` | `contexts: None`, `components`, `full` | `ADR` (unslotted) + `LABEL` (unslotted) | Unqualified postal address and label slot. |



### 3.5 Organizations, Titles & Roles (`ORG`, `TITLE`, `ROLE`)
- **`ORG` Component Hierarchy**:
  - Component 0: Organization Name -> `E_CONTACT_ORG`
  - Component 1: Unit / Department -> `E_CONTACT_ORG_UNIT`
  - Component 2: Office -> `E_CONTACT_OFFICE`
  - Components 3+: Trailing organizational hierarchy levels.
  - Nameless organizations with departments emit a leading semicolon (`ORG:;Engineering`) to prevent the department from shifting into the employer name slot.
- **`TITLE` and `ROLE`**:
  - `kind: "title"` (default) -> `TITLE` -> `E_CONTACT_TITLE`
  - `kind: "role"` -> `ROLE` -> `E_CONTACT_ROLE`
  - Vendor kinds (e.g. `x-honorific`) are dropped from vCard 3.0 and preserved on the server.

### 3.6 Anniversaries & Birthdays (`BDAY`, `X-EVOLUTION-ANNIVERSARY`)
- **`BDAY`**: `kind: "birth"` -> `E_CONTACT_BIRTH_DATE`.
- **`X-EVOLUTION-ANNIVERSARY`**: `kind: "wedding"` -> `E_CONTACT_ANNIVERSARY`.
- **Clamping Protection**: EDS's `e_contact_date_to_string` clamps years to `1000..=9999`. Incomplete dates (bare years `1984` or partial months) and years < 1000 are omitted from vCard emission by `Day::survives_the_field_it_lands_in` to prevent EDS from corrupting them into January 1, 1000.
- **`kind: "death"`**: Dropped from vCard 3.0 because EDS has no corresponding field.

### 3.7 Instant Messaging (`ONLINE_SERVICES`)
- **10 Supported Slotted Services**: AIM, Gadu-Gadu, Google Talk, GroupWise, ICQ, Jabber, MSN, Matrix, Skype, Yahoo.
- **URI Scheme Mapping**: `SERVICE_SCHEMES` translates bare URIs (`xmpp:`, `aim:`, `gg:`, `groupwise:`, `icq:`, `msn:`, `msnim:`, `matrix:`, `skype:`, `yahoo:`, `ymsgr:`) into plain handles.
- **Unslotted / Unmapped Services**: `X-TWITTER` and `X-SIP` are defined in EDS as `EContactAttrList` (`GList*` of `char*`) without `HOME`/`WORK` slots and are deliberately unmapped in `jmap-vcard`.

### 3.8 Categories & Keywords (`CATEGORIES` ↔ `E_CONTACT_CATEGORY_LIST`)
- **Set vs List Mapping**: JSContact `keywords` (RFC 9553 §2.8.2) is a mathematical `Set` (JSON map with `true` values). vCard 3.0 `CATEGORIES` (RFC 2426 §3.7.1) is a comma-separated list of text values. EDS maps this line to `E_CONTACT_CATEGORY_LIST` (Evolution's Categories field).
- **Lexicographical Sorting & Stability**: Because sets possess no intrinsic order, `drawn_tags` sorts keyword tags lexicographically before emitting the `CATEGORIES` line. This guarantees deterministic vCard output across serialization cycles, preventing spurious diffs during JMAP sync.
- **Delimiter & Character Escaping**:
  - Commas within a category name (e.g. `"Acme, Inc."` -> `Acme\, Inc.`, `"Software, Core"` -> `Software\, Core`) are backslash-escaped on emission and unescaped on parse, preventing categories from splitting into multiple tags.
  - Semicolons (`\;`), newlines (`\n`), and backslashes (`\\`) are similarly escaped and preserved with 100% roundtrip fidelity.
- **Multi-Line Inbound Merging & Deduplication**:
  - RFC 2426 allows multiple `CATEGORIES` lines in a single vCard, but Evolution/EDS only renders the first `CATEGORIES` line in its UI.
  - `read_keywords` reads all `CATEGORIES` lines across the vCard, flattening `entry_items` and deduplicating identical tags into a unified `keywords` map. Outbound serialization consolidates all tags into a single canonical `CATEGORIES` line.
- **Whitespace Defense & Refusal Invariants**:
  - EDS trims leading and trailing whitespace when users edit categories in Evolution.
  - [`states_keyword`] refuses empty tags (`""`), non-boolean values (`Value::Bool(false)` or strings/numbers), carriage returns (`\r`), and tags with leading or trailing ASCII whitespace (`edged_with_whitespace`). This prevents emitting tags that EDS would silently trim and rename on the server.
- **Empty / Absent Categories**: Cards with `keywords: None`, empty sets, or only unstated tags emit no `CATEGORIES` line. Inbound vCards with absent `CATEGORIES`, empty values (`CATEGORIES:`), or delimiter-only lines (`CATEGORIES:,,,`) parse to `keywords: None`.

### 3.9 Nicknames & URLs (`NICKNAME`, `URL` ↔ `E_CONTACT_NICKNAME`, `E_CONTACT_HOMEPAGE_URL`)
- **`NICKNAME` Cardinality & Identity Preservation**:
  - RFC 2426 §3.1.3 specifies `NICKNAME` as a single comma-separated `text-list` on the wire format (`NICKNAME:Rob,Robbie,Boss`).
  - JSContact (RFC 9553 §2.2.2) models nicknames as a keyed map (`nicknames: { "k1": { "name": "Rob" }, "k2": { "name": "Robbie" } }`).
  - `jmap-vcard` emits **one line per keyed entry** (`NICKNAME;X-JMAP-KEY=k1:Rob\r\nNICKNAME;X-JMAP-KEY=k2:Robbie\r\n`) so that each entry carries its unique `X-JMAP-KEY` parameter across synchronization cycles.
  - EDS 3.52 (`libebook-contacts`) reads `E_CONTACT_NICKNAME` from the first `NICKNAME` line. When editing the nickname in Evolution, EDS rewrites that first line's value in place while leaving parameters intact, and passes subsequent `NICKNAME` lines through untouched.
- **`NICKNAME` Comma Handling & Single-String Parsing**:
  - Inbound vCards from third-party clients containing comma-separated lists on a single line (`NICKNAME:Rob,Robbie,Boss`) are parsed via `entry_text_list` into a single `Nickname { name: "Rob,Robbie,Boss" }`.
  - *Rationale*: EDS 3.52 hands the entire value back as one string and does not split it on commas. Splitting it into multiple JSContact entries would create synthetic entries that Evolution's UI cannot display individually.
  - Outbound re-emission escapes literal commas as `\,` (`NICKNAME;X-JMAP-KEY=k1:Rob\,Robbie\,Boss`), and subsequent parse passes read it back as `"Rob,Robbie,Boss"`, guaranteeing deterministic fixed-point convergence.
  - Unmodeled `contexts` and `pref` in `Nickname.extra` ride untouched on the JMAP layer.
- **`URL` Mapping & EDS Homepage Slotting**:
  - Plain website links (`kind: None` in JSContact `links`) map directly to RFC 2426 §3.6.8 `URL` lines carrying `X-JMAP-KEY`.
  - In EDS, `E_CONTACT_HOMEPAGE_URL` maps to the **first `URL` line** in the vCard. Subsequent `URL` lines pass through intact on the vCard stream and are parsed back into `card.links` with their respective keys (`l1`, `l2`, `l3`).
- **EDS Blog & Video URLs Mapping**:
  - EDS defines `E_CONTACT_BLOG_URL` (`X-EVOLUTION-BLOG-URL`) and `E_CONTACT_VIDEO_URL` (`X-EVOLUTION-VIDEO-URL`).
  - `jmap-vcard` maps these directly to JSContact `links` entries with `kind: Some("blog")` and `kind: Some("video")` respectively, carrying `X-JMAP-KEY`.
  - Generic vendor properties without the `X-EVOLUTION-` prefix (e.g. `X-BLOG-URL`, `X-VIDEO-URL`) are ignored on parse to avoid synthetic vendor drift.
- **`URL` Kind Filtering & Contact URI Omission**:
  - RFC 9553 §2.6.3 defines `kind: "contact"` as a URI for communicating with the person (e.g. contact forms, mailto links), which RFC 9555 §2.6.3 states on vCard 4.0's `CONTACT-URI`.
  - vCard 3.0 has no `CONTACT-URI` property. Emitting `kind: "contact"` or unmapped vendor kinds (`kind: "feed"`, `"profile"`) on a vCard 3.0 `URL` would populate Evolution's `E_CONTACT_HOMEPAGE_URL` and mislead the user into seeing a contact form or feed as the person's homepage.
  - Therefore, [`states_link`] and [`maps_link_kind`] restrict vCard emission strictly to `kind: None` (`URL`), `kind: Some("blog")` (`X-EVOLUTION-BLOG-URL`), and `kind: Some("video")` (`X-EVOLUTION-VIDEO-URL`). All other kinds are omitted on the wire format and remain safely preserved on the server.
- **URI Punctuation & Escaping**:
  - URIs with query strings containing semicolons, commas, ampersands, hashes, and percent-encodings (e.g. `https://api.example.com/search?q=a,b;c#top`) are formatted without backslash escaping per RFC 3986 and RFC 2426 §3.6.8, round-tripping with 100% fidelity.
- **Unmodeled `Link` Properties**:
  - JSContact `Link` fields `mediaType`, `contexts`, `pref`, and `label` ride in `extra` and are untouched during `jmap-book-sync`'s `PatchObject` synchronization.

### 3.10 Photos & Media (`PHOTO` ↔ `E_CONTACT_PHOTO`)
- **Inline Binary Data (`ENCODING=b`)**:
  - Encoded using standard base64 per RFC 2426 §3.1.4.
  - The MIME subtype is extracted by `image_subtype` (e.g. `image/jpeg` -> `TYPE=jpeg`, `image/png` -> `TYPE=png`, `image/svg+xml` -> `TYPE=svg+xml`) and emitted via `VCardParameter::typ`.
  - Non-image data URIs (e.g. `data:application/pdf;base64,...` or `data:;base64,...`) are emitted without a `TYPE` parameter (`PHOTO;ENCODING=b:...`), which EDS accepts and reports without a MIME type.
- **Remote URI References (`VALUE=uri`)**:
  - Emitted as `PHOTO;X-JMAP-KEY=m1;VALUE=uri:<uri>`.
  - In vCard 3.0, the `VALUE=uri` parameter is mandatory for EDS to populate `E_CONTACT_PHOTO` (`EContactPhotoType::URI`). Lines omitting `VALUE=uri` are not recognized by EDS as URI photos.
  - No `TYPE` or `ENCODING` parameter is emitted on URI lines (RFC 2426 §3.1.4).
- **Non-Photo Media Filtering**:
  - JSContact (RFC 9553 §2.6.4) groups `photo`, `logo`, and `sound` under `card.media`.
  - [`states_media`] and `photo` filter strictly for `kind: Some("photo")`. Logos, sounds, documents, and unmapped kinds get no `PHOTO` line, preserving UI separation in Evolution.
  - Unmapped media entries remain safe on the server because `jmap-book-sync` patches only mapped/edited properties.

### 3.11 Relationships: Spouse, Manager, Assistant (`X-EVOLUTION-SPOUSE`, `X-EVOLUTION-MANAGER`, `X-EVOLUTION-ASSISTANT`)
- **EDS Relation Fields**:
  - Evolution's contact editor provides dedicated text fields for **Spouse** (`E_CONTACT_SPOUSE`), **Manager** (`E_CONTACT_MANAGER`), and **Assistant** (`E_CONTACT_ASSISTANT`).
  - In EDS vCard 3.0 representation, these are stored on `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-MANAGER`, and `X-EVOLUTION-ASSISTANT` property lines.
- **JSContact `relatedTo` Mapping (RFC 9553 §2.1.8 / RFC 9555 §2.9.5)**:
  - JSContact models relationships in `card.related_to` as a map keyed by entity name (for free-text vCard entries) with a `relation` set (`{"spouse": true}`, `{"manager": true}`, `{"assistant": true}`).
  - When parsing from vCard, `vcard_to_card` inserts or updates `related_to[name]` with the corresponding relation type if `names_a_person(name)` holds.
  - Outbound serialization emits lines for each stated relation type: [`states_spouse`], [`states_manager`], and [`states_assistant`].
  - If a single person holds multiple roles (e.g. both manager and assistant), multiple lines are emitted (`X-EVOLUTION-MANAGER:Alex`, `X-EVOLUTION-ASSISTANT:Alex`) and round-trip into a unified `Relation.relation` map.
- **Name Validation & URI Identifier Defense**:
  - `names_a_person` rejects empty names, URI scheme identifiers (`urn:uuid:...`, `mailto:...`, `http:...`), leading/trailing ASCII whitespace, and carriage returns.
  - This ensures that entity UIDs are not mistakenly rendered as human person names in Evolution's text fields.

---

## 4. Special Semantics & Product Decision Catalog

All product decisions and behavioral findings documented in `docs/AGY-LOG.md` are codified below:

### 4.1 Dropped-by-Design Rationale for Unknown `X-` Properties
`jmap-vcard` deliberately ignores unmapped vendor `X-` properties (e.g., `X-MOZILLA-HTML`, `X-APPLE-*`, `X-MS-*`, `X-SIGNAL`, `X-DISCORD`, `X-TELEGRAM`, `X-SLACK`, `X-TWITTER`, `X-SIP`, `X-MANAGER`, `X-ASSISTANT`, `X-EVOLUTION-FILE-AS`, `X-EVOLUTION-CALLBACK`, `X-EVOLUTION-RADIO`, `X-EVOLUTION-TELEX`, `X-EVOLUTION-TTYTDD`):
1. **Contract Integrity**: Prevents polluting standard JSContact (RFC 9553) models with raw non-standard vCard lines in `extra`.
2. **Sync Safety**: `jmap-book-sync`'s `PatchObject` issues patches only for mapped/edited fields. Dropping unmapped properties on parse ensures the server's existing unmodeled attributes remain completely untouched.
3. **UI Isolation**: Evolution has no UI fields for unsupported properties. Inventing fake mappings would confuse users and corrupt server records.

### 4.2 Group Cards & Distribution Lists (`KIND:group`, `MEMBER`)
- RFC 6473 / RFC 6350 group cards (`KIND:group`, `MEMBER:urn:uuid:...`) and EDS contact distribution lists (`X-EVOLUTION-LIST:TRUE` / `E_CONTACT_IS_LIST`, `X-EVOLUTION-DEST-EMAIL`) are not mapped to individual `ContactCard` records.
- `vcard_to_card` safely ignores list markers to prevent misinterpreting list member emails as personal addresses of a single contact.

### 4.3 Multilingual Alternates (`ALTID`, `LANGUAGE`)
- **Singleton Properties (`FN`, `N`)**: Deterministically select the first representation in document order.
- **Multi-Valued Properties (`NOTE`, `TITLE`, `ROLE`, `ORG`, `ADR`, `NICKNAME`, `URL`, `EMAIL`, `TEL`)**: Preserve all language alternates as distinct keyed entries with separate `X-JMAP-KEY`s.

### 4.4 Preference Ranking (`PREF`)
- vCard 3.0 represents preference as a boolean flag (`TYPE=PREF`). Inbound lines with `TYPE=PREF` are parsed as `pref: 1` (or `extra["pref"] = 1`).
- Outbound lines are sorted by `(pref.unwrap_or(u32::MAX), key)` to guarantee preferred entries land in EDS primary positions (`E_CONTACT_EMAIL_1`, `E_CONTACT_PHONE_PRIMARY`, `E_CONTACT_ADDRESS_HOME`).

### 4.5 Line Folding & Unfolding (RFC 2426 §2.6)
- **Outbound Emission**: Handled automatically by `calcard` via `entry.write_to(out, true)`. Physical content lines target the standard 75-octet limit and fold using CRLF followed by a single space (`\r\n `). Because `calcard` iterates over Unicode scalar values (`char`) and evaluates `char::len_utf8()` before writing, multi-byte UTF-8 sequences (2-byte umlauts, 3-byte CJK/Devanagari, 4-byte emoji) are **never split** across a fold.
- **Boundary Characterization**: Due to parameter `:` delimiters and 2-byte escape sequences (`\n`, `\\`, `\;`) checked against 1-byte char sizes, physical lines on the wire may measure up to 76–77 octets before folding, fully compliant with RFC 2426 §2.6 ("lines of more than 75 characters SHOULD be folded").
- **Inbound Unfolding**: `vcard_to_card` losslessly unfolds pre-folded input delimited by CRLF + space (`\r\n `) or CRLF + tab (`\r\n\t`), stripping the CRLF and the leading continuation whitespace while preserving any subsequent spaces or tabs as literal data.
- **Large Binary / Inline Media**: Long values such as multi-line `NOTE`s and inline base64-encoded `PHOTO;ENCODING=b;TYPE=...` payloads fold across multiple continuation lines and round-trip to `Media` / `Note` with 100% binary and text fidelity and fixed-point stability (`card2 == card` and `vcard2 == vcard3`).

### 4.6 Value Escaping & Unescaping (RFC 2426 §2)
- **Special Character Escaping**: Free-text and structured values containing newlines (`\n` or `\r\n`), commas (`,`), semicolons (`;`), and backslashes (`\`) are escaped on emission and unescaped on parsing:
  - `\n` or `\N`: Represent newlines in text values (e.g. `NOTE`, `LABEL`, `NICKNAME`, `TITLE`, `ROLE`). `vcard_to_card` unescapes both lowercase `\n` and uppercase `\N` losslessly.
  - `\,`: Escapes literal commas. In structured properties like `ORG` (e.g. `"Acme, Inc."` -> `Acme\, Inc.`) and `ADR` (e.g. `"Apt 4B, Room 12"` -> `Apt 4B\, Room 12`), and list properties like `CATEGORIES`, commas within an item are escaped to prevent premature item splitting.
  - `\;`: Escapes literal semicolons. In structured properties like `ADR` (e.g. `street: "Suite 100; Building A"` -> `Suite 100\; Building A`) and `ORG` (e.g. `unit: "Hardware; Systems"` -> `Hardware\; Systems`), semicolons within a component are escaped to prevent component shifting into wrong positional slots.
  - `\\`: Escapes literal backslashes. Consecutive backslashes (e.g. `\\` -> `\\\\`) and escaped backslashes preceding delimiters (e.g. `\;` -> `\\\;`) are preserved with exact round-trip fidelity.
- **No Double-Escaping Invariant**: Serialization and deserialization passes are idempotent and achieve fixed-point convergence (`card_to_vcard(vcard_to_card(vcard)) == vcard`). Repeated serialization cycles never accumulate redundant backslashes (`\\` remains `\\`, not growing into `\\\\`).
- **Whitespace Defense**: Tags and handles with leading/trailing whitespace or carriage returns are filtered by [`states_keyword`] and [`drawn_service`] to prevent EDS from silently trimming them and triggering unwanted server renames.

### 4.7 Non-ASCII, `CHARSET`, and `ENCODING` Parameters (RFC 2426 §2.1.2 & §2.1.3)
- **vCard 3.0 Character Set & Transport Contract**:
  - **Character Set**: RFC 2426 §2.1.2 mandates that vCard 3.0 is unconditionally UTF-8. The `CHARSET` parameter is not supported / deprecated for text properties.
  - **Transport Encoding**: RFC 2426 §2.1.3 mandates that vCard 3.0 uses 8-bit MIME transport encoding. `ENCODING=QUOTED-PRINTABLE`, `ENCODING=8BIT`, and `ENCODING=7BIT` are not supported on text properties. Binary properties (`PHOTO`) use `ENCODING=b` (or `b`).
  - **Outbound Emission (`card_to_vcard`)**: Always conforms strictly to RFC 2426 by emitting native UTF-8 strings directly without redundant `CHARSET` or `ENCODING` parameters on text properties.
- **Inbound Compatibility & Robustness (Postel's Law)**:
  - Older exporters (vCard 2.1, Evolution 2.x/3.x, Outlook, Apple Address Book, Thunderbird) frequently export vCard 3.0 with redundant `;CHARSET=UTF-8` or legacy `;ENCODING=QUOTED-PRINTABLE`.
  - **`CHARSET` Parameter**: `vcard_to_card` (via `calcard`) accepts case-insensitive `CHARSET` parameters (`UTF-8`, `utf-8`, `ISO-8859-1`, `WINDOWS-1252`) across all properties.
  - **`ENCODING=QUOTED-PRINTABLE`**: Properties carrying `ENCODING=QUOTED-PRINTABLE` are automatically decoded according to the specified `CHARSET` (defaulting to ISO-8859-1 if `CHARSET` is omitted, per RFC 2045 / vCard 2.1). Soft line breaks (`=\r\n` and `=\n`) and hex byte escapes (`=XX`, e.g. `=3D`, `=3B`, `=2C`, `=0D=0A`) are decoded losslessly into native UTF-8 text in JSContact fields.
  - **`ENCODING=8BIT` / `7BIT`**: Accepted on input and parsed as plain text without modification.
  - **`ENCODING=b` / `BASE64` / `B`**: Decoded as binary data for inline `PHOTO`s into standard data URIs (`data:image/<subtype>;base64,<payload>`).
- **Outbound Normalization & Fixed-Point Stability**:
  - Legacy inputs parsed with `CHARSET` or `ENCODING=QUOTED-PRINTABLE` are normalized on subsequent save operations into clean, standard vCard 3.0 UTF-8 format.
  - Subsequent roundtrips achieve fixed-point stability (`card_to_vcard(parsed) == card_to_vcard(vcard_to_card(card_to_vcard(parsed)))`).
- **Multilingual Script Coverage**:
  - Full roundtrip fidelity is verified across diverse world writing systems: Latin with diacritics (French, German, Spanish, Icelandic, Polish), Cyrillic, Greek, Hebrew, Arabic (RTL), East Asian (Chinese Hanzi, Japanese Kanji/Kana, Korean Hangul), South Asian (Hindi Devanagari), Southeast Asian (Vietnamese), and Emoji/symbols (`🧑‍💻`, `🚀`, `🌟`).

### 4.8 Inline `PHOTO` (base64) vs URI Semantics, Media Type Lossy-by-Design Finding & EDS Contract
- **Inline Photo Media Type Normalization**:
  - EDS 3.52 prepends `image/` to the `TYPE` parameter value (e.g. `TYPE=jpeg` -> `image/jpeg`).
  - [`image_subtype`] strips `image/` prefixes to ensure `TYPE` contains only the subtype.
  - When EDS exports a photo without a known MIME type, it emits `TYPE="X-EVOLUTION-UNKNOWN"`. `read_photo` filters this out so `media_type` becomes `None` and the URI becomes `data:;base64,...`, preventing phantom MIME types like `image/X-EVOLUTION-UNKNOWN`.
- **Remote URI Media Type (Lossy by Design across vCard 3.0)**:
  - RFC 2426 §3.1.4 does not define `TYPE` on URI-valued `PHOTO` properties (`PHOTO;VALUE=uri:...`).
  - EDS 3.52 neither writes nor reads `TYPE` on URI photo lines (`E_CONTACT_PHOTO` stores `EContactPhotoType::URI` with just the URI string).
  - Therefore, if a JSContact `Media` entry specifies a remote URI *and* a `media_type` (e.g. `uri: "https://example.com/avatar.jpg"`, `media_type: Some("image/jpeg")`), `card_to_vcard` intentionally omits `TYPE` to conform to RFC 2426 and EDS contracts.
  - Reading the vCard back via `vcard_to_card` results in `media_type: None`.
  - *Sync Safety*: Untouched remote URIs on the JMAP server preserve their original `mediaType` because `jmap-book-sync`'s `PatchObject` issues patches only for modified paths.
- **EDS Photo Field Replacements & Semantic Equality ([`same_photo`])**:
  - When a user changes or crops a photo in Evolution, EDS rewrites the `PHOTO` line and drops the `X-JMAP-KEY` parameter.
  - `vcard_to_card` allocates a new key (`m1`, `m2`), and [`same_photo`] compares image payloads:
    - Normalizes base64 padding (unpadded `data:` URIs match padded vCard base64).
    - Compares MIME subtypes case-insensitively (`image/jpeg` == `image/JPEG`).
    - Compares URI strings directly.
  - This allows the sync layer to detect whether the photo was actually edited by the user, avoiding redundant image re-uploads on every sync.

### 4.9 Deliberate Drop Rationale for Standard vCard 3.0 Properties (`GEO`, `TZ`, `MAILER`, `PRODID`, `REV`, `SORT-STRING`, `CLASS`, `SOUND`, `LOGO`)
`jmap-vcard` deliberately ignores standard vCard 3.0 properties for which Evolution/EDS lacks active UI editing support or for which client-side preservation is architecturally incorrect:
1. **`GEO` (RFC 2426 §3.4.2)**:
   - Evolution's contact editor has no UI controls or display for geographical coordinates.
   - JSContact (RFC 9553 §2.5.1) scopes coordinates to specific postal addresses (`Address.coordinates`), rather than top-level cards.
   - *Rationale*: Dropping top-level `GEO` lines prevents polluting JSContact data with non-standard top-level coordinates or guessing which address the coordinate belongs to. Server-side `Address.coordinates` values are untouched during sync by `PatchObject`.
2. **`TZ` (RFC 2426 §3.4.1)**:
   - Evolution's contact editor has no contact-specific time zone field.
   - JSContact (RFC 9553 §2.1.2) uses IANA Time Zone Database identifiers (`card.time_zone`), whereas vCard 3.0 `TZ` typically contains UTC offsets (`-05:00`) or non-standard abbreviations (`EST`).
   - *Rationale*: Dropped on vCard parse/emission. Server-side `card.time_zone` is preserved untouched by `PatchObject`.
3. **`MAILER` (RFC 2426 §3.6.3)**:
   - Deprecated and removed in RFC 6350 (vCard 4.0 Appendix A.3).
   - Identifies the email software agent of the sender. Evolution has no UI or storage for contact email agents.
   - *Rationale*: Dropped by design. Deprecated legacy client metadata.
4. **`PRODID` (RFC 2426 §3.6.4)**:
   - Identifies the software that created the vCard stream (e.g. `PRODID:-//Apple Inc.//macOS 14.5//EN`).
   - *Rationale*: Generator metadata belongs to the serializing exporter, not the contact record. Carrying over foreign `PRODID` strings across subsequent exports from Evolution/JMAP would misattribute the generator. Dropped by design.
5. **`REV` (RFC 2426 §3.6.5)**:
   - Timestamp of the vCard revision (RFC 9553 §1.4 `updated`).
   - *Rationale*: Revision timestamps are strictly owned and managed by the authoritative store (the JMAP server) upon committing changes. Preserving or emitting stale client-side `REV` timestamps would corrupt server revision tracking. `PatchObject` leaves `updated` to the JMAP server.
6. **`SORT-STRING` (RFC 2426 §3.6.7)**:
   - Family name sort string, replaced in RFC 6350 / JSContact by `sortAs` parameters on `Name` and `Organization`.
   - Evolution uses `X-EVOLUTION-FILE-AS` (`E_CONTACT_FILE_AS`) for filing display names (mapped to `Name.extra["fileAs"]`).
   - *Rationale & Coexistence*: `SORT-STRING` is dropped from vCard 3.0 emission by design. On the JSContact layer, `fileAs` (`X-EVOLUTION-FILE-AS`) and `sortAs` (`SORT-STRING`) are stored under separate keys (`extra["fileAs"]` vs `extra["sortAs"]`), ensuring neither clobbers the other across round-trips. Server `sortAs` properties remain safe and untouched in `extra` via `PatchObject`.
7. **`CLASS` (RFC 2426 §3.7.2)**:
   - Access classification (`PUBLIC`, `PRIVATE`, `CONFIDENTIAL`). Deprecated/removed in vCard 4.0.
   - Evolution contact editor has no access classification controls.
   - *Rationale*: Dropped by design. Server-side privacy settings are preserved untouched by `PatchObject`.
8. **`SOUND` (RFC 2426 §3.6.6)**:
   - Digital audio clips / pronunciation guides (RFC 9553 §2.6.4 `media` with `kind: "sound"`).
   - EDS has no `E_CONTACT_SOUND` field and Evolution has no audio playback in the contact editor.
   - *Rationale*: [`states_media`] filters strictly for `kind: Some("photo")`. Inbound `SOUND` lines are dropped on vCard parse to prevent misparsing as photos. Server-side `sound` media entries remain safe and untouched in `card.media` via `PatchObject`.
9. **`LOGO` (RFC 2426 §3.5.3)**:
   - Organization logo image (RFC 9553 §2.6.4 `media` with `kind: "logo"`).
   - Although `E_CONTACT_LOGO` exists in EDS C enum definitions, Evolution's contact editor provides UI exclusively for personal photos (`E_CONTACT_PHOTO`).
   - *Rationale*: [`states_media`] filters strictly for `kind: Some("photo")`. Inbound `LOGO` lines are dropped on vCard parse to prevent colliding with or replacing the personal photo field. Server-side `logo` media entries remain safe and untouched in `card.media` via `PatchObject`.

### 4.10 vCard 2.1 Legacy Import Tolerance (Asymmetric Compatibility Contract)

Real-world contact exporters (such as older versions of Microsoft Outlook, feature phones from Nokia and Sony Ericsson, and legacy PBX systems) continue to emit vCard 2.1 data. To ensure robust interoperability without compromising modern standards, `jmap-vcard` implements an **asymmetric import tolerance contract**:

```
[ Inbound vCard 2.1 / 3.0 / 4.0 ]
              │
              ▼ vcard_to_card() (Postel's Law: liberal in what we accept)
[ JSContact ContactCard (RFC 9553) ]
              │
              ▼ card_to_vcard() (Strict RFC 2426 vCard 3.0 UTF-8)
[ Outbound Canonical vCard 3.0 ]
```

#### Accepted vCard 2.1 Subset:

1. **Bare Parameter Type Names (No `TYPE=` Prefix)**:
   - In vCard 2.1, parameter values were frequently written as bare words without the `TYPE=` parameter key (e.g. `TEL;WORK;VOICE:+12345` instead of `TEL;TYPE=WORK,VOICE:+12345`).
   - [`vcard_to_card`] accepts all bare type names across telephony, email, and address properties:
     - Phone contexts: `WORK`, `HOME`.
     - Phone features: `VOICE`, `FAX`, `CELL`, `MOBILE`, `PAGER`, `VIDEO`, `CAR`, `ISDN`, `TTYTDD`.
     - Email contexts and types: `INTERNET`, `WORK`, `HOME`, `PREF`.
     - Address contexts and types: `WORK`, `HOME`, `POSTAL`, `PARCEL`, `DOM`, `INTL`, `PREF`.
   - `entry_has_type` matches both standard `TYPE=value` parameters and bare parameter names matching the target token case-insensitively.
2. **Preference Flags (`PREF`)**:
   - Accepts bare `PREF` parameters (e.g. `EMAIL;PREF;INTERNET:alice@example.com`, `TEL;WORK;PREF:+12345`, `ADR;WORK;PREF:...`, `LABEL;WORK;PREF:...`) and maps them to `pref: Some(1)` or `extra["pref"] = 1`.
   - Outbound emission sorts preferred entries to the top (`E_CONTACT_EMAIL_1`, primary phone) and emits standard vCard 3.0 `TYPE=PREF`.
3. **Character Sets & Transport Encodings**:
   - `CHARSET` parameter: Accepts legacy character set declarations (`CHARSET=UTF-8`, `CHARSET=ISO-8859-1`, `CHARSET=WINDOWS-1252`) case-insensitively on any property.
   - `ENCODING=QUOTED-PRINTABLE`: Automatically decodes Quoted-Printable hexadecimal octets (`=C3=BC`, `=FC`, `=80`) into standard UTF-8 text strings according to the declared `CHARSET` (or ISO-8859-1 default per RFC 2045).
   - Soft Line Breaks: Losslessly unfolds Quoted-Printable soft line breaks (`=\r\n` and `=\n`) without introducing extraneous whitespace.
4. **Legacy Photo Formats & Subtype Inference**:
   - Accepts bare image formats in photo parameters: `PHOTO;JPEG;ENCODING=BASE64:...`, `PHOTO;GIF;BASE64:...`, `PHOTO;PNG;ENCODING=BASE64:...`, and `PHOTO;TYPE=JPEG;ENCODING=BASE64:...`.
   - Automatically identifies image subtypes from bare parameter names (`JPEG`, `GIF`, `PNG`, `BMP`, `TIFF`, `WEBP`) and constructs valid `data:image/<subtype>;base64,...` data URIs.
   - Outbound serialization normalizes strictly to canonical vCard 3.0 format (`PHOTO;ENCODING=b;TYPE=<SUBTYPE>:...`).
5. **Outbound Invariant & Fixed-Point Stability**:
   - Outbound serialization via [`card_to_vcard`] is unconditionally RFC 2426 vCard 3.0 in native UTF-8 with standard line folding (75 octets) and backslash value escaping (`\n`, `\,`, `\;`, `\\`).
   - Legacy parameters (`CHARSET`, `QUOTED-PRINTABLE`, `INTERNET`, bare types) are never emitted.
   - Fixed-point stability is guaranteed: importing a 2.1 vCard, emitting as 3.0, and re-parsing reaches exact fixed-point equality (`export2 == export3` and `card2 == card3`).

### 4.11 Apple Property Groups & `X-ABLabel` Semantic Mapping

vCards exported from macOS AddressBook, iOS Contacts, and iCloud use property grouping (RFC 2426 §2.1.1) combined with companion `X-ABLabel` lines to attach custom and localized labels to standard and extended contact properties:

```vcard
item1.TEL:(555) 555-0100
item1.X-ABLabel:_$!<Mobile>!$_
item2.EMAIL;type=INTERNET;type=pref:john.appleseed@work.example.com
item2.X-ABLabel:_$!<Work>!$_
item3.ADR;type=pref:;;1 Infinite Loop;Cupertino;CA;95014;USA
item3.X-ABLabel:_$!<Work>!$_
item4.URL:https://johnappleseed.example.com
item4.X-ABLabel:_$!<HomePage>!$_
item5.X-ABRELATEDNAMES:Jane Appleseed
item5.X-ABLabel:_$!<Spouse>!$_
item6.X-ABDATE:2018-06-20
item6.X-ABLabel:_$!<Anniversary>!$_
```

`jmap-vcard` parses and maps grouped properties with full semantic fidelity:

1. **Apple Label Marker Unwrapping (`clean_apple_label`)**:
   - Strips Apple localization delimiters `_$!<` and `>!$_` from raw label strings (e.g. `_$!<Work>!$_` -> `Work`, `_$!<Mobile>!$_` -> `Mobile`).
   - Custom / user-defined labels without delimiters (e.g. `Direct Line`, `HQ Office`) are trimmed and preserved.
2. **Context & Feature Slot Resolution**:
   - **Emails (`EMAIL`)**:
     - `Work` / `School` -> `contexts: {"work": true}` (filed to `E_CONTACT_EMAIL_1` / `EMAIL_3`).
     - `Home` -> `contexts: {"private": true}` (filed to `E_CONTACT_EMAIL_2` / `EMAIL_4`).
     - Custom labels -> `email.extra["label"] = "<custom>"`.
   - **Telephony (`TEL`)**:
     - `Mobile` / `Cell` / `iPhone` -> `features: {"mobile": true}` (`E_CONTACT_PHONE_MOBILE`).
     - `Pager` -> `features: {"pager": true}` (`E_CONTACT_PHONE_PAGER`).
     - `WorkFAX` / `Work FAX` -> `features: {"fax": true}`, `contexts: {"work": true}` (`E_CONTACT_PHONE_BUSINESS_FAX`).
     - `HomeFAX` / `Home FAX` -> `features: {"fax": true}`, `contexts: {"private": true}` (`E_CONTACT_PHONE_HOME_FAX`).
     - `Main` -> `features: {"voice": true}`, `contexts: {"work": true}` (`E_CONTACT_PHONE_BUSINESS` / `PRIMARY`).
     - `Work` / `Home` -> `contexts: {"work": true}` / `contexts: {"private": true}`.
     - Custom labels -> `phone.extra["label"] = "<custom>"`.
   - **Addresses (`ADR`)**:
     - `Work` / `School` -> `contexts: {"work": true}` (`E_CONTACT_ADDRESS_WORK`).
     - `Home` -> `contexts: {"private": true}` (`E_CONTACT_ADDRESS_HOME`).
     - Custom labels -> `address.extra["label"] = "<custom>"`.
   - **Links (`URL`)**:
     - `HomePage` / `Home Page` -> `kind: None` (`E_CONTACT_HOMEPAGE_URL`).
     - `Blog` -> `kind: Some("blog")` (`E_CONTACT_BLOG_URL`).
     - `Work` / `Home` -> `link.extra["contexts"] = {"work": true}` / `{"private": true}`.
     - Custom labels -> `link.extra["label"] = "<custom>"`.
3. **Apple Extended Relations (`X-ABRELATEDNAMES`)**:
   - `spouse` / `partner` -> `card.related_to[name]` with `relation: {"spouse": true}` (`E_CONTACT_SPOUSE` / `X-EVOLUTION-SPOUSE`).
   - `manager` -> `card.related_to[name]` with `relation: {"manager": true}` (`E_CONTACT_MANAGER` / `X-EVOLUTION-MANAGER`).
   - `assistant` -> `card.related_to[name]` with `relation: {"assistant": true}` (`E_CONTACT_ASSISTANT` / `X-EVOLUTION-ASSISTANT`).
   - Custom relations -> `card.related_to[name]` with `relation: {"<custom>": true}`.
4. **Apple Extended Dates (`X-ABDATE`)**:
   - `anniversary` / `wedding` -> `card.anniversaries` with `kind: "wedding"` (`E_CONTACT_ANNIVERSARY` / `X-EVOLUTION-ANNIVERSARY`).
   - `birthday` / `birth` -> `card.anniversaries` with `kind: "birth"` (`E_CONTACT_BIRTH_DATE` / `BDAY`).
   - Custom dates -> `card.anniversaries` with `kind: "<custom>"`.
5. **Outbound Normalization & Fixed-Point Stability**:
   - Outbound emission normalizes to standard RFC 2426 vCard 3.0 lines (`TYPE=WORK,CELL`, `X-EVOLUTION-SPOUSE`, `X-EVOLUTION-ANNIVERSARY`).
   - Re-parsing emitted vCards achieves byte-identical fixpoints (`Export₂ == Export₃`).

---

## 5. Function & Predicate Index

| Function Name | Visibility | Primary Role / Responsibility |
| :--- | :--- | :--- |
| [`card_to_vcard`] | `pub` | Serializes JSContact [`ContactCard`] into RFC 2426 vCard 3.0 string for EDS consumption. |
| [`vcard_to_card`] | `pub` | Parses RFC 2426 vCard 3.0 string into JSContact [`ContactCard`]. |
| [`states_name_component`] | `pub` | Evaluates if a name component is non-empty and has a valid `N` slot. |
| [`restore_name_components`] | `pub` | Restores split given name components if the joined string was unedited in EDS. |
| [`states_nickname`] | `pub` | Checks if nickname contains non-empty text. |
| [`states_email`] | `pub` | Checks if email contains non-empty address. |
| [`maps_context`] | `pub` | Evaluates whether a context key (`work`, `private`) is supported by vCard `TYPE`. |
| [`states_context`] | `pub` | Checks whether an entry's context was actually stated on the vCard line. |
| [`maps_phone_feature`] | `pub` | Checks whether a phone feature (`mobile`, `pager`, `fax`, `voice`, `video`) is supported. |
| [`states_phone_feature`] | `pub` | Evaluates whether a phone feature was emitted onto the vCard `TEL` line. |
| [`states_phone`] | `pub` | Checks if phone contains non-empty number. |
| [`states_address`] | `pub` | Checks if an address has any structured components or a printable label. |
| [`states_address_component`] | `pub` | Evaluates if an address component kind maps to one of the 7 `ADR` fields. |
| [`address_label`] | `pub` | Extracts non-empty formatted label text (`Address.full`). |
| [`restore_address_components`] | `pub` | Restores split street name and house number components if unedited in EDS. |
| [`states_organization`] | `pub` | Checks if an organization has a name or at least one unit. |
| [`states_org_unit`] | `pub` | Checks if an organizational unit has a non-empty name. |
| [`title_kind`] | `pub` | Resolves title kind with fallback to `"title"`. |
| [`states_title`] | `pub` | Checks if title has non-empty name and supported kind (`title` or `role`). |
| [`states_note`] | `pub` | Checks if note contains non-empty text. |
| [`states_link`] | `pub` | Checks if link contains non-empty URI and mapped kind (`None`, `blog`, `video`). |
| `maps_link_kind` | `private` | Checks if link kind is supported on vCard 3.0 / EDS (`None`, `blog`, `video`). |
| `entry_text_list` | `private` | Reads parsed text values of a multi-valued property and joins them with commas. |
| [`states_calendar`] | `pub` | Checks if calendar has non-empty URI and mapped kind (`calendar` or `freeBusy`). |
| [`states_media`] | `pub` | Evaluates if media entry is a valid photo with supported inline/URI payload. |
| [`same_photo`] | `pub` | Compares two media entries for semantic image equality across base64/MIME representations. |
| `photo` | `private` | Resolves `Media` into inline base64 or URI variant, validating format and media type. |
| `read_photo` | `private` | Parses vCard `PHOTO` entry into JSContact `Media`, extracting subtype and decoding base64 data. |
| `image_subtype` | `private` | Extracts image subtype from `image/*` MIME string (stripping prefix and parameters). |
| `photo_entry` | `private` | Constructs standard `Media` struct for `kind: "photo"`. |
| [`states_online_service`] | `pub` | Checks if online service has an EDS slot, valid handle, and safe whitespace. |
| [`online_service_handle`] | `pub` | Extracts bare handle from `user` field or URI scheme. |
| [`online_service_uri`] | `pub` | Formats canonical URI for a supported service and handle. |
| [`same_service`] | `pub` | Compares service names case- and punctuation-insensitively. |
| [`states_anniversary`] | `pub` | Checks if anniversary is supported (`birth`/`wedding`) and has a valid calendar day >= 1000. |
| [`anniversary_date`] | `pub` | Formats anniversary date as `YYYY-MM-DD` if valid and survives EDS clamping. |
| [`states_a_point_in_time`] | `pub` | Checks if anniversary is dated by a UTC `Timestamp` rather than `PartialDate`. |
| [`states_spouse`] | `pub` | Checks if relation is `spouse` and key names a printable person (not a URI). |
| [`states_manager`] | `pub` | Checks if relation is `manager` and key names a printable person (not a URI). |
| [`states_assistant`] | `pub` | Checks if relation is `assistant` and key names a printable person (not a URI). |
| [`states_nothing_but_the_marriage`] | `pub` | Checks if relation entry contains only the `spouse` relation type. |
| [`states_keyword`] | `pub` | Validates keyword tag for boolean `true`, non-emptiness, and whitespace safety. |

---

## 6. Round-Trip Fixpoint Stability & Regression Net

### 6.1 The Multi-Stage Round-Trip Contract

Every contact property mapping in `jmap-vcard` satisfies a strict multi-pass fixed-point stability invariant across the translation lifecycle:

```
vCard₁ (Inbound / Legacy 2.1 / Foreign Exporter)
  │
  ▼ vcard_to_card()
Card₁ (JSContact representation)
  │
  ▼ card_to_vcard()
vCard₂ (Export₁: Canonical RFC 2426 vCard 3.0)
  │
  ▼ vcard_to_card()
Card₂ (EContact₂: In-Memory EDS representation)
  │
  ▼ card_to_vcard()
vCard₃ (Export₂: Re-Emitted vCard 3.0)
  │
  ▼ vcard_to_card()
Card₃ (EContact₃: Stabilized JSContact Card)
  │
  ▼ card_to_vcard()
vCard₄ (Export₃: Stabilized vCard 3.0)
```

### 6.2 Standing Fixpoint Invariants

1. **Byte-Identical vCard Fixpoint**:
   $$\text{Export}_2 (\text{vCard}_3) \equiv \text{Export}_3 (\text{vCard}_4)$$
   The second and third vCard 3.0 exports are guaranteed to be byte-identical. No line reordering, parameter drift, delimiter re-escaping, or whitespace fluctuation occurs.

2. **Structural JSContact Fixpoint**:
   $$\text{Card}_2 (\text{EContact}_2) \equiv \text{Card}_3 (\text{EContact}_3)$$
   The deserialized JSContact structures reach complete identity by the second roundtrip pass.

3. **Oscillation Diagnosis & Proptest Net**:
   The `proptest_fuzz.rs` suite continuously fuzzes the fixpoint contract across arbitrary raw vCard inputs and arbitrary `ContactCard` instances. When test assertions fail, the oscillation analyzer diagnostic (`identify_oscillating_vcard_property` / `identify_oscillating_card_field`) isolates the specific property name and line difference, enabling instant root-cause identification during proptest test case shrinkage.

### 6.3 Trailing Whitespace & Legacy Parameter Preservation

1. **Trailing Whitespace on Text Values**:
   - RFC 6350 §3.3 / RFC 2426 §2 makes trailing whitespace significant in text property values (`FN`, `N`, `NICKNAME`, `EMAIL`, `TEL`, `ADR`, `ORG`, `TITLE`, `ROLE`, `NOTE`, `URL`, `X-EVOLUTION-*`).
   - `calcard` and `contact.rs` preserve trailing whitespace within property values without stripping or truncation, ensuring that multi-pass roundtrips reach byte-identical fixpoints (`Export₂ == Export₃`).
2. **Whitespace-Only Property Values**:
   - Whitespace-only values in set-based fields (such as `CATEGORIES: `) are cleanly rejected by validator predicates ([`states_keyword`]) to prevent EDS string-trimming bugs.
   - Whitespace-only values in text fields (such as `NICKNAME: ` or `NOTE: `) either parse into structured fields or evaluate to empty, reaching stable fixpoint convergence by Export₂.
3. **Legacy `ENCODING=b` on Text Lines (BACKLOG Regression Pin)**:
   - When legacy/fuzzed vCards declare binary parameters on non-binary properties (e.g. `NICKNAME;ENCODING=b:! `), the parser cleanly rejects non-base64 binary payloads via [`entry_text_list`] and normalizes the representation, reaching fixed-point stability on Export₂.


