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
| **`TEL`** | `TYPE` (`WORK`, `HOME`, `CELL`, `PAGER`, `FAX`, `VOICE`, `VIDEO`, `PREF`), `X-JMAP-KEY` | `card.phones` (`ContactPhone.number`, `contexts`, `features`, `pref`) | `E_CONTACT_PHONE_BUSINESS`, `_HOME`, `_MOBILE`, `_BUSINESS_FAX`, `_HOME_FAX`, `_PAGER`, `_OTHER`, `_OTHER_FAX`, `_PRIMARY` | `states_phone`, `context_slot`, [`states_context`], `feature_slot`, [`states_phone_feature`] | Slot narrowing: EDS matches `TYPE` to slots. Context narrowed to at most 1 (`WORK` > `HOME` -> `DEFAULT_SLOT`); feature narrowed to at most 1 (`CELL` > `PAGER` > `FAX` > `VOICE` > `VIDEO`). Unstated features preserved by predicates. |
| **`ADR`** | `TYPE` (`WORK`, `HOME`, `PREF`), `LABEL`, `X-JMAP-KEY` | `card.addresses` (`Address.components`, `contexts`, `extra["pref"]`) | `E_CONTACT_ADDRESS_WORK`, `_HOME`, `_OTHER` (+ 7 subfields per slot) | `address_fields`, `read_address`, `states_address`, [`states_address_component`], [`restore_address_components`] | 7 components: PO Box, Ext, Street, Locality, Region, Postcode, Country. House `number` joins street `name`. `restore_address_components` reconstructs split components. Unmapped kinds (`floor`, `room`) dropped. |
| **`LABEL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.addresses` (`Address.full`) | `E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER` | `address_label`, `label_entry`, `read_address` | Standalone line emitted after `ADR` or on its own. Inbound matched by `X-JMAP-KEY` or context/text fallback to prevent duplicate addresses. |
| **`ORG`** | `X-JMAP-KEY` | `card.organizations` (`Organization.name`, `units`) | `E_CONTACT_ORG` (name), `E_CONTACT_ORG_UNIT` (dept), `E_CONTACT_OFFICE` (office) | `organization_components`, `read_organization`, [`states_organization`], [`states_org_unit`] | Semicolon-delimited list. Index 0 = Name; Index 1 = Department; Index 2 = Office; Index 3+ = trailing units. Nameless orgs retain leading semicolon (`ORG:;Unit`). `sortAs` and `contexts` unmapped on vCard 3.0. |
| **`TITLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "title"`) | `E_CONTACT_TITLE` | `read_title`, [`states_title`], [`title_kind`] | `kind: "title"` (or `None`) maps to `TITLE`. Vendor kinds dropped. |
| **`ROLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "role"`) | `E_CONTACT_ROLE` | `read_title`, [`states_title`], [`title_kind`] | `kind: "role"` maps to `ROLE`. |
| **`NOTE`** | `X-JMAP-KEY` | `card.notes` (`Note.note`) | `E_CONTACT_NOTE` | `states_note` | Free text. First line lands in EDS `E_CONTACT_NOTE`. RFC 9553 `created` and `author` ride in `extra` and are untouched during sync. |
| **`URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: None`) | `E_CONTACT_HOMEPAGE_URL` | `states_link`, `maps_link_kind` | Only plain websites (`kind: None`) map to `URL`. `kind: "contact"` (`CONTACT-URI`) and vendor kinds dropped. `mediaType`, `label`, `contexts`, `pref` ride in `extra`. |
| **`CALURI`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "calendar"`) | `E_CONTACT_CALENDAR_URI` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. `ICSCALENDAR` excluded. |
| **`FBURL`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "freeBusy"`) | `E_CONTACT_FREEBUSY_URL` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. |
| **`PHOTO`** | `TYPE`, `ENCODING=b`, `VALUE=uri`, `X-JMAP-KEY` | `card.media` (`Media.uri`, `media_type`, `kind: "photo"`) | `E_CONTACT_PHOTO` | `photo`, `read_photo`, [`states_media`], [`same_photo`], `image_subtype` | Only `kind: "photo"` mapped. Inline data uses base64; `TYPE` states subtype only (e.g. `JPEG` -> `image/jpeg`). URI references use `VALUE=uri`. Re-paired via `same_photo` since EDS drops `X-JMAP-KEY` on photo edit. |
| **`CATEGORIES`** | — | `card.keywords` (`Set<String>`) | `E_CONTACT_CATEGORY_LIST` | `drawn_tags`, `read_keywords`, [`states_keyword`] | Single sorted line emitted. Comma-separated on wire. Trimming protection: tags with leading/trailing whitespace or carriage returns omitted from emission to prevent EDS corruption. |
| **`BDAY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "birth"`, `date`) | `E_CONTACT_BIRTH_DATE` | `read_anniversary`, [`states_anniversary`], [`anniversary_date`], [`states_a_point_in_time`], `Day` | Single calendar day formatted `YYYY-MM-DD`. Truncated/bare years (`1984`) or years < 1000 omitted to prevent EDS clamping corruption (`1000..=9999`). `Timestamp` converted to UTC day. |
| **`X-EVOLUTION-ANNIVERSARY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, [`states_anniversary`], [`anniversary_date`] | EDS wedding anniversary field. Same date validation and year >= 1000 clamping rules as `BDAY`. `kind: "death"` dropped. |
| **`X-EVOLUTION-SPOUSE`** | — | `card.related_to` (Key = Person Name, `relation: { "spouse": true }`) | `E_CONTACT_SPOUSE` | `spouse_named`, [`states_spouse`], [`states_nothing_but_the_marriage`], `names_a_person` | Key in JSContact `related_to` is the spouse name (RFC 9555 §2.9.5). No `X-JMAP-KEY` needed. 19 non-spouse relation types and URI keys dropped. |
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
- **`EMAIL` Positional Filing**: Evolution files `EMAIL` lines by position into `E_CONTACT_EMAIL_1` .. `_4`. `card_to_vcard` sorts emails by `(pref.unwrap_or(u32::MAX), key)` to guarantee the primary preferred address lands in `EMAIL_1`.
- **`TEL` Slot Narrowing**:
  - Context narrowing: EDS matches `TYPE` sets to fields. A line carrying `TYPE=WORK,HOME` would occupy two separate UI blocks that overwrite each other. `context_slot` chooses exactly one slot: `WORK` > `HOME` -> `DEFAULT_SLOT` (`HOME`).
  - Feature narrowing: `feature_slot` ranks phone features: `mobile` (`CELL`) > `pager` (`PAGER`) > `fax` (`FAX`) > `voice` (`VOICE`) > `video` (`VIDEO`).
  - `states_context` and `states_phone_feature` ensure omitted contexts/features are not considered deleted by the user on sync.

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
- **`LABEL` Pairing**: In vCard 3.0, `LABEL` is a standalone property; in vCard 4.0, it is a parameter on `ADR`. `read_address` parses `ADR;LABEL=...` parameters directly, and `label_entry` pairs standalone `LABEL` lines with their preceding `ADR` entries using `X-JMAP-KEY` or context matching.

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
- **`URL` Kind Filtering & Contact URI Omission**:
  - RFC 9553 §2.6.3 defines `kind: "contact"` as a URI for communicating with the person (e.g. contact forms, mailto links), which RFC 9555 §2.6.3 states on vCard 4.0's `CONTACT-URI`.
  - vCard 3.0 has no `CONTACT-URI` property. Emitting `kind: "contact"` or vendor kinds (`kind: "blog"`, `"video"`, `"feed"`) on a vCard 3.0 `URL` would populate Evolution's `E_CONTACT_HOMEPAGE_URL` and mislead the user into seeing a contact form or feed as the person's homepage.
  - Therefore, [`states_link`] and [`maps_link_kind`] restrict vCard 3.0 emission **strictly to `kind: None`** (plain websites). All other kinds are omitted on the wire format and remain safely preserved on the server.
- **EDS Blog & Video URLs vs JSContact Links**:
  - EDS defines `E_CONTACT_BLOG_URL` (`X-EVOLUTION-BLOG-URL`) and `E_CONTACT_VIDEO_URL` (`X-EVOLUTION-VIDEO-URL`).
  - `jmap-vcard` deliberately does NOT map these non-standard properties into `links` or `extra` to prevent polluting standard JSContact schemas. `vcard_to_card` safely ignores them on parse, leaving them as unmapped EDS extensions.
- **URI Punctuation & Escaping**:
  - URIs with query strings containing semicolons, commas, ampersands, hashes, and percent-encodings (e.g. `https://api.example.com/search?q=a,b;c#top`) are formatted without backslash escaping per RFC 3986 and RFC 2426 §3.6.8, round-tripping with 100% fidelity.
- **Unmodeled `Link` Properties**:
  - JSContact `Link` fields `mediaType`, `contexts`, `pref`, and `label` ride in `extra` and are untouched during `jmap-book-sync`'s `PatchObject` synchronization.

---

## 4. Special Semantics & Product Decision Catalog

All product decisions and behavioral findings documented in `docs/AGY-LOG.md` are codified below:

### 4.1 Dropped-by-Design Rationale for Unknown `X-` Properties
`jmap-vcard` deliberately ignores unmapped vendor `X-` properties (e.g., `X-MOZILLA-HTML`, `X-APPLE-*`, `X-MS-*`, `X-SIGNAL`, `X-DISCORD`, `X-TELEGRAM`, `X-SLACK`, `X-EVOLUTION-MANAGER`, `X-EVOLUTION-ASSISTANT`, `X-EVOLUTION-FILE-AS`):
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
| [`states_link`] | `pub` | Checks if link contains non-empty URI and plain website kind (`None`). |
| `maps_link_kind` | `private` | Filters link kind to allow only plain website links (`kind: None`) on vCard 3.0 `URL`. |
| `entry_text_list` | `private` | Reads parsed text values of a multi-valued property and joins them with commas. |
| [`states_calendar`] | `pub` | Checks if calendar has non-empty URI and mapped kind (`calendar` or `freeBusy`). |
| [`states_media`] | `pub` | Evaluates if media entry is a valid photo with supported inline/URI payload. |
| [`same_photo`] | `pub` | Compares two media entries for semantic image equality across base64/MIME representations. |
| [`states_online_service`] | `pub` | Checks if online service has an EDS slot, valid handle, and safe whitespace. |
| [`online_service_handle`] | `pub` | Extracts bare handle from `user` field or URI scheme. |
| [`online_service_uri`] | `pub` | Formats canonical URI for a supported service and handle. |
| [`same_service`] | `pub` | Compares service names case- and punctuation-insensitively. |
| [`states_anniversary`] | `pub` | Checks if anniversary is supported (`birth`/`wedding`) and has a valid calendar day >= 1000. |
| [`anniversary_date`] | `pub` | Formats anniversary date as `YYYY-MM-DD` if valid and survives EDS clamping. |
| [`states_a_point_in_time`] | `pub` | Checks if anniversary is dated by a UTC `Timestamp` rather than `PartialDate`. |
| [`states_spouse`] | `pub` | Checks if relation is `spouse` and key names a printable person (not a URI). |
| [`states_nothing_but_the_marriage`] | `pub` | Checks if relation entry contains only the `spouse` relation type. |
| [`states_keyword`] | `pub` | Validates keyword tag for boolean `true`, non-emptiness, and whitespace safety. |
