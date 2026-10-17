<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# vCard to JSContact mapping

How `jmap-vcard` maps the vCard that Evolution Data Server (EDS) hands the
address book backend to a JMAP `ContactCard`, and back.

- vCard: 3.0 (RFC 2426) on output; 2.1, 3.0 and 4.0 (RFC 6350, RFC 6474,
  RFC 6473) on input. Parsed and written with `calcard`.
- JSContact: RFC 9553 and RFC 9555. JMAP for Contacts: RFC 9610.
- EDS: the `EContactField` enum of `libebook-contacts` 3.52 (`E_CONTACT_*`),
  as bound by `eds-sys`.

Where the code lives:

| Crate / file | Role |
| :--- | :--- |
| `jmap-vcard/src/contact.rs` | `card_to_vcard`, `vcard_to_card`, the `states_*` and `maps_*` predicates |
| `jmap-book-sync` | Save path: turns an edited vCard into a `ContactCard/set` PatchObject |
| `jmap-proto/src/contacts.rs` | The `ContactCard` type |

---

## 1. Design

1. **Edits are patches, not replacements.** `jmap-vcard` maps only what
   Evolution's contact editor shows. Other JSContact data (for example
   `preferredLanguages`, `localizations`, `cryptoKeys`, `pronouns`, unmapped
   relations) is not written to the vCard. `jmap-book-sync` saves with a
   PatchObject that names only mapped, edited paths, so that data stays on
   the server.
2. **Predicates guard partly shown properties.** A missing field in an
   edited vCard means "deleted" only if the field could be shown. The
   predicates (`states_context`, `states_phone_feature`,
   `states_address_component`, `states_spouse`, `states_keyword` and the other
   `states_*` functions) say whether a value was shown.
3. **Map keys travel with the line.** Multi-valued entries carry
   `X-JMAP-KEY=<key>`. `entry_key` reads it back. An entry without a key gets
   a new deterministic key (`e1`, `p1`, `a1`, ...).
4. **Fixed point.** `card_to_vcard(vcard_to_card(card_to_vcard(c)))` equals
   `card_to_vcard(c)` (section 7).

---

## 2. Property table

| vCard Property | vCard Parameters | JSContact Field (RFC 9553/9555) | EDS Field (`EContactField`) | Primary Helpers & Predicates | Lossy / Product Decision Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`UID`** | | `card.id` (or `card.uid`) | `E_CONTACT_UID` | `card_to_vcard`, `vcard_to_card` | `UID` carries server JMAP ID for EDS cache indexing; `X-JMAP-UID` carries original JSContact UUID. |
| **`X-JMAP-UID`** | | `card.uid` | | `card_to_vcard`, `vcard_to_card` | Retains client-side JSContact UUID across vCard round-trips. |
| **`FN`** | `ALTID`, `LANGUAGE` | `card.name.full` | `E_CONTACT_FULL_NAME` | `derive_full`, `read_name` | First `FN` in document order is selected; synthesized from `name.components` if omitted. |
| **`N`** | `ALTID`, `LANGUAGE` | `card.name.components` (`surname`, `given`, `given2`, `title`, `credential`) | `E_CONTACT_FAMILY_NAME`, `E_CONTACT_GIVEN_NAME`, `E_CONTACT_ADDITIONAL_NAME`, `E_CONTACT_NAME_PREFIX`, `E_CONTACT_NAME_SUFFIX` | `name_fields`, `read_name`, `states_name_component`, `restore_name_components` | 5-component structured name. Double-barrelled given names share index 1 and are reconstructed via `restore_name_components`. Name `phonetic` is dropped. |
| **`NICKNAME`** | `X-JMAP-KEY`, `ALTID` | `card.nicknames` (`Nickname.name`) | `E_CONTACT_NICKNAME` | `states_nickname`, `entry_text_list` | Emitted as one line per keyed entry (not comma-separated) to preserve `X-JMAP-KEY`. Commas in nicknames are preserved literally. |
| **`EMAIL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.emails` (`ContactEmail.address`, `contexts`, `pref`) | `E_CONTACT_EMAIL_1` .. `_4` | `states_email`, `type_names`, `read_flags` | Positional filing in EDS (`EMAIL_1` is primary). Sorted on emission by `(pref, key)` so lowest `pref` lands in `EMAIL_1`. Multiple contexts allowed. |
| **`TEL`** | `TYPE` (`WORK`, `HOME`, `CELL`, `MOBILE`, `PAGER`, `FAX`, `VOICE`, `VIDEO`, `PREF`), `X-JMAP-KEY` | `card.phones` (`ContactPhone.number`, `contexts`, `features`, `pref`) | `E_CONTACT_PHONE_PRIMARY`, `_BUSINESS`, `_BUSINESS_2`, `_BUSINESS_FAX`, `_HOME`, `_HOME_2`, `_HOME_FAX`, `_MOBILE`, `_PAGER`, `_OTHER`, `_OTHER_FAX`, `_CAR`, `_ISDN`, `_CALLBACK`, `_COMPANY`, `_RADIO`, `_TELEX`, `_TTYTDD`, `_ASSISTANT` | `states_phone`, `context_slot`, `states_context`, `feature_slot`, `states_phone_feature` | Full 19-field EDS matrix. Inbound accepts `MOBILE` synonym for `CELL`; outbound normalizes to `CELL`. Context narrowed to at most 1 (`WORK` > `HOME` -> `DEFAULT_SLOT`); feature narrowed to at most 1 (`CELL`/`MOBILE` > `PAGER` > `FAX` > `VOICE` > `VIDEO`). Unstated features preserved by predicates. |
| **`ADR`** | `TYPE` (`WORK`, `HOME`, `PREF`), `LABEL`, `X-JMAP-KEY` | `card.addresses` (`Address.components`, `contexts`, `extra["pref"]`) | `E_CONTACT_ADDRESS_WORK`, `_HOME`, `_OTHER` (+ 7 subfields per slot) | `address_fields`, `read_address`, `states_address`, `states_address_component`, `restore_address_components` | 7 components: PO Box, Ext, Street, Locality, Region, Postcode, Country. House `number` joins street `name`. `restore_address_components` reconstructs split components. Unmapped kinds (`floor`, `room`) dropped. |
| **`LABEL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.addresses` (`Address.full`) | `E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER` | `address_label`, `label_entry`, `read_address` | Standalone line emitted after `ADR` or on its own. Inbound matched by `X-JMAP-KEY` or context/text fallback to prevent duplicate addresses. |
| **`ORG`** | `X-JMAP-KEY` | `card.organizations` (`Organization.name`, `units`) | `E_CONTACT_ORG` (name), `E_CONTACT_ORG_UNIT` (dept), `E_CONTACT_OFFICE` (office) | `organization_components`, `read_organization`, `states_organization`, `states_org_unit` | Semicolon-delimited list. Index 0 = Name; Index 1 = Department; Index 2 = Office; Index 3+ = trailing units. Nameless orgs retain leading semicolon (`ORG:;Unit`). `sortAs` and `contexts` unmapped on vCard 3.0. |
| **`TITLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "title"`) | `E_CONTACT_TITLE` | `read_title`, `states_title`, `title_kind` | `kind: "title"` (or `None`) maps to `TITLE`. Vendor kinds dropped. |
| **`ROLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "role"`) | `E_CONTACT_ROLE` | `read_title`, `states_title`, `title_kind` | `kind: "role"` maps to `ROLE`. |
| **`NOTE`** | `X-JMAP-KEY` | `card.notes` (`Note.note`) | `E_CONTACT_NOTE` | `states_note` | Free text. First line lands in EDS `E_CONTACT_NOTE`. RFC 9553 `created` and `author` ride in `extra` and are untouched during sync. |
| **`URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: None`) | `E_CONTACT_HOMEPAGE_URL` | `states_link`, `maps_link_kind` | Plain websites (`kind: None`) map to `URL`. `mediaType`, `label`, `contexts`, `pref` ride in `extra`. |
| **`X-EVOLUTION-BLOG-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "blog"`) | `E_CONTACT_BLOG_URL` | `states_link`, `maps_link_kind` | EDS blog URL field. Maps to `links` with `kind: "blog"`. |
| **`X-EVOLUTION-VIDEO-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "video"`) | `E_CONTACT_VIDEO_URL` | `states_link`, `maps_link_kind` | EDS video stream URL field. Maps to `links` with `kind: "video"`. |
| **`CALURI`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "calendar"`) | `E_CONTACT_CALENDAR_URI` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. `ICSCALENDAR` excluded. |
| **`FBURL`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "freeBusy"`) | `E_CONTACT_FREEBUSY_URL` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property emitted on vCard 3.0 for EDS 3.52 compatibility. |
| **`ANNIVERSARY`** | `X-JMAP-KEY`, `VALUE=date` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, `Day` | vCard 4.0 standard anniversary property (RFC 6350 §6.4.3). Maps to `kind: "wedding"`. Outbound emits both `X-EVOLUTION-ANNIVERSARY` (EDS 3.52) and `ANNIVERSARY` (EDS 3.60+). |
| **`RELATED`** | `TYPE` (`spouse`, `partner`, `manager`, `assistant`, custom) | `card.related_to` (Key = Person Name, `relation: { "<type>": true }`) | `E_CONTACT_SPOUSE`, `_MANAGER`, `_ASSISTANT` | `names_a_person`, `vcard_to_card` | vCard 4.0 standard relationship property (RFC 6350 §6.6.6). Maps `TYPE=spouse`/`partner` -> `spouse`, `TYPE=manager` -> `manager`, `TYPE=assistant` -> `assistant`, or custom relations when `names_a_person` holds. Outbound normalizes to standard `X-EVOLUTION-*`. |
| **`IMPP`** | `TYPE` (`WORK`, `HOME`), `X-JMAP-KEY` | `card.online_services` (`service`, `user`) | `E_CONTACT_IM_<SERVICE>_HOME_1..3`, `_WORK_1..3` | `SERVICE_SCHEMES`, `plain_handle`, `vcard_to_card` | vCard 4.0 standard instant messaging property (RFC 4770 / RFC 6350 §6.4.3). Maps URI schemes (`xmpp:`, `skype:`, `matrix:`, `aim:`, `icq:`, `msn:`, `yahoo:`, `groupwise:`, `gg:`, `gtalk:`) to matching service handle. Action/query URIs safely rejected. Outbound normalizes to slotted `X-` properties (`X-JABBER`, `X-SKYPE`, `X-MATRIX`, etc.). |
| **`PHOTO`** | `TYPE`, `MEDIATYPE`, `ENCODING=b`, `VALUE=uri`, `X-JMAP-KEY` | `card.media` (`Media.uri`, `media_type`, `kind: "photo"`) | `E_CONTACT_PHOTO` | `photo`, `read_photo`, `states_media`, `same_photo`, `image_subtype` | Only `kind: "photo"` mapped. Inbound accepts RFC 2397 `data:` URIs (direct without double-base64 encoding), direct HTTP/HTTPS URIs without `VALUE=uri`, and `MEDIATYPE` parameters (RFC 6350 §5.7). Outbound uses canonical vCard 3.0 base64 + `ENCODING=b`. Re-paired via `same_photo` since EDS drops `X-JMAP-KEY` on photo edit. |
| **`CATEGORIES`** | | `card.keywords` (`Set<String>`) | `E_CONTACT_CATEGORY_LIST` | `drawn_tags`, `read_keywords`, `states_keyword` | Single sorted line emitted. Comma-separated on wire. Trimming protection: tags with leading/trailing whitespace or carriage returns omitted from emission to prevent EDS corruption. |
| **`BDAY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "birth"`, `date`) | `E_CONTACT_BIRTH_DATE` | `read_anniversary`, `states_anniversary`, `anniversary_date`, `states_a_point_in_time`, `Day` | Single calendar day formatted `YYYY-MM-DD`. Truncated/bare years (`1984`) or years < 1000 omitted to prevent EDS clamping corruption (`1000..=9999`). `Timestamp` converted to UTC day. |
| **`X-EVOLUTION-ANNIVERSARY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, `states_anniversary`, `anniversary_date` | EDS 3.52 wedding anniversary field. Same date validation and year >= 1000 clamping rules as `BDAY`. Outbound emits both `X-EVOLUTION-ANNIVERSARY` and `ANNIVERSARY` for dual-version compatibility. `kind: "death"` dropped. |
| **`X-EVOLUTION-SPOUSE`** | | `card.related_to` (Key = Person Name, `relation: { "spouse": true }`) | `E_CONTACT_SPOUSE` | `states_spouse`, `states_nothing_but_the_marriage`, `names_a_person` | Key in JSContact `related_to` is the spouse name (RFC 9555 §2.9.5). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-EVOLUTION-MANAGER`** | | `card.related_to` (Key = Person Name, `relation: { "manager": true }`) | `E_CONTACT_MANAGER` | `states_manager`, `names_a_person` | Key in JSContact `related_to` is the manager name (RFC 9553 §2.1.8). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-EVOLUTION-ASSISTANT`** | | `card.related_to` (Key = Person Name, `relation: { "assistant": true }`) | `E_CONTACT_ASSISTANT` | `states_assistant`, `names_a_person` | Key in JSContact `related_to` is the assistant name (RFC 9553 §2.1.8). No `X-JMAP-KEY` needed. Non-person/URI keys dropped. |
| **`X-AIM`** | `TYPE` (`WORK`, `HOME`), `X-JMAP-KEY` | `card.online_services` (`service: "AIM"`, `user`, `uri: "aim:..."`) | `E_CONTACT_IM_AIM_HOME_1..3`, `_WORK_1..3` | `drawn_service`, `states_online_service`, `online_service_handle`, `online_service_uri`, `service_slot` | 6 EDS slots per service. Handles extracted from `user` or bare URI schemes (`SERVICE_SCHEMES`). Action/query URIs rejected. `TYPE` mandatory for EDS field visibility. |
| **`X-GADUGADU`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Gadu-Gadu"`, `uri: "gg:..."`) | `E_CONTACT_IM_GADUGADU_HOME_1..3`, `_WORK_1..3` | (same as above) | Matched case/punctuation-insensitively (`same_service`). |
| **`X-GOOGLE-TALK`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Google Talk"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_GOOGLE_TALK_HOME_1..3`, `_WORK_1..3` | (same as above) | Handles use `xmpp` scheme. |
| **`X-GROUPWISE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "GroupWise"`, `uri: "groupwise:..."`) | `E_CONTACT_IM_GROUPWISE_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare handles mapped. |
| **`X-ICQ`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "ICQ"`, `uri: "icq:..."`) | `E_CONTACT_IM_ICQ_HOME_1..3`, `_WORK_1..3` | (same as above) | Numeric UIN handles. |
| **`X-JABBER`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Jabber"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_JABBER_HOME_1..3`, `_WORK_1..3` | (same as above) | XMPP JID handles. |
| **`X-MSN`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "MSN"`, `uri: "msn:... / msnim:..."`) | `E_CONTACT_IM_MSN_HOME_1..3`, `_WORK_1..3` | (same as above) | Supports `msn` and `msnim` schemes. |
| **`X-MATRIX`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Matrix"`, `uri: "matrix:..."`) | `E_CONTACT_IM_MATRIX_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Matrix handles (`matrix:@user:domain` without action queries). |
| **`X-SKYPE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Skype"`, `uri: "skype:..."`) | `E_CONTACT_IM_SKYPE_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Skype usernames (`skype:echo123?call` action rejected). |
| **`X-YAHOO`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Yahoo"`, `uri: "yahoo:... / ymsgr:..."`) | `E_CONTACT_IM_YAHOO_HOME_1..3`, `_WORK_1..3` | (same as above) | Supports `yahoo` and `ymsgr` schemes. |
| **`GEO`** | | `Address.coordinates` (RFC 9553) | `E_CONTACT_GEO` (no UI) | | Dropped by design on vCard 3.0 import/export. Evolution has no UI for coordinates. Server-side `Address.coordinates` preserved by `PatchObject`. |
| **`TZ`** | | `card.time_zone` (RFC 9553) | | | Dropped by design on vCard 3.0 import/export. Text/Olson timezones in vCard 4.0 (`TZ:Europe/Berlin`) are safely ignored on import. Server `time_zone` preserved by `PatchObject`. |
| **`MAILER`** | | | `E_CONTACT_MAILER` (legacy) | | Dropped by design. Deprecated in RFC 6350 (vCard 4.0). Legacy email client software metadata. |
| **`PRODID`** | | `card.prod_id` (RFC 9553) | | | Dropped by design on import/export. Generator metadata belongs to serialization envelope; foreign `PRODID` not preserved across saves. |
| **`REV`** | | `card.updated` (RFC 9553) | `E_CONTACT_REV` | | Dropped by design on import/export. Revision timestamp is strictly owned by the JMAP server upon commit. |
| **`GENDER`** | | | | | vCard 4.0 property (RFC 6350 §6.5.1). Safely ignored by reader; does not pollute `card.extra`. |
| **`CLIENTPIDMAP`** | | | | | vCard 4.0 property (RFC 6350 §6.7.7). Safely ignored by reader; does not pollute `card.extra`. |
| **`MEMBER`** | | | | | vCard 4.0 group member property (RFC 6350 §6.6.5). Safely ignored for individual contacts. |
| **`SORT-STRING`** | | `Name.sortAs` / `Org.sortAs` | | | Dropped by design from vCard 3.0 emission. Replaced in RFC 6350 by `SORT-AS` parameter. JSContact `sortAs` preserved on server by `PatchObject` without clobbering `fileAs`. |
| **`X-EVOLUTION-FILE-AS`** | | `Name.extra["fileAs"]` / `card.extra["fileAs"]` | `E_CONTACT_FILE_AS` | `states_file_as` | Evolution "File Under" field. Inbound accepts `X-EVOLUTION-FILE-AS`, `FILE-AS`, and `X-FILE-AS`. Outbound normalizes to `X-EVOLUTION-FILE-AS`. Coexists with `sortAs` without clobbering. |
| **`CLASS`** | | `card.privacy` (RFC 9553) | | | Dropped by design. Deprecated/removed in RFC 6350. Legacy access classification with no Evolution editor UI. |
| **`SOUND`** | `TYPE`, `ENCODING=b`, `VALUE=uri`, `MEDIATYPE` | `card.media` (`kind: "sound"`) | | `states_media` | Dropped by design from vCard 3.0. `states_media` permits only `kind: "photo"`. Inbound audio payloads and URIs safely ignored without corrupting photos. Server `sound` media entries preserved by `PatchObject`. |
| **`AGENT`** | `VALUE=uri`, text, nested vCard | `card.related_to` (`relation: { "agent": true }`) | | `states_spouse`, `states_manager`, `states_assistant` | Dropped by design from vCard 3.0 emission. Deprecated/removed in RFC 6350. Inbound nested vCards, URIs, and text safely ignored without corrupting surrounding card or polluting `card.extra`. Server `relatedTo` agent entries preserved by `PatchObject`. |
| **`LOGO`** | `TYPE`, `ENCODING=b`, `VALUE=uri` | `card.media` (`kind: "logo"`) | `E_CONTACT_LOGO` (no UI) | `states_media` | Dropped by design from vCard 3.0. Evolution editor supports only personal photo (`E_CONTACT_PHOTO`). Server `logo` entries preserved by `PatchObject`. |
| **`KEY`** | `TYPE` (`X509`, `PGP`), `ENCODING=b`, `VALUE=uri` | `card.extra["cryptoKeys"]` (RFC 9553 §2.7.1) | `E_CONTACT_X509_CERT`, `E_CONTACT_PGP_CERT` (no UI) | | Dropped by design from vCard 3.0. Evolution editor provides no contact certificate UI. Server `cryptoKeys` entries preserved by `PatchObject`. |
| **`itemN.PROPERTY`** | `X-ABLabel` companion | (associated property) | (associated EDS slot) | `clean_apple_label`, `vcard_to_card` | Apple property groups (RFC 2426 §2.1.1). Group prefix is parsed by `calcard`; companion `X-ABLabel` maps contexts (`Work`, `Home`), features (`Mobile`, `Pager`, `Fax`), or custom labels. |
| **`X-ABLabel`** | | `extra["label"]` or mapped context/feature | | `clean_apple_label`, `vcard_to_card` | Apple label annotation. Markers (`_$!<Label>!$_`) unwrapped. Standard labels map to JSContact contexts/features; custom labels preserved in `extra["label"]`. |
| **`X-ABRELATEDNAMES`** | `X-ABLabel` | `card.related_to` (Key = Person Name) | `E_CONTACT_SPOUSE`, `_MANAGER`, `_ASSISTANT` | `clean_apple_label`, `vcard_to_card` | Apple relationship property. Group companion `X-ABLabel` selects relation type (`spouse`, `manager`, `assistant`, custom). Outbound normalizes to standard `X-EVOLUTION-*`. |
| **`X-ABDATE`** | `X-ABLabel` | `card.anniversaries` (`kind`, `date`) | `E_CONTACT_ANNIVERSARY` | `clean_apple_label`, `read_day` | Apple date property. Group companion `X-ABLabel` selects anniversary kind (`wedding`, `birth`, custom). Outbound normalizes to `X-EVOLUTION-ANNIVERSARY` / `BDAY`. |
---

## 3. Fields

### 3.1 Identity

- `UID` carries the JMAP `id`, because EDS keys its cache on `UID`.
  `X-JMAP-UID` carries the JSContact `uid` when it differs.
- A contact created in Evolution has a local `UID`. `jmap-book-sync` does
  not send it as the `id`; the server assigns one.

### 3.2 Names (`FN`, `N`, `NICKNAME`)

- `FN`: the first one in document order. When the card has no `full` name,
  `derive_full` builds one from the components (title, given, given2,
  surname, credential).
- `N`: `[surname, given, given2, title, credential]`. Several components of
  one kind (for example two `given` parts) are joined with a space. On save,
  `restore_name_components` puts the parts back if the field still reads as
  the joined parts; if the user edited it, the new text replaces them.
  `phonetic` is not written and stays on the server.
- `NICKNAME`: one line per keyed entry, so that each keeps its
  `X-JMAP-KEY`. EDS shows the first line. A foreign
  `NICKNAME:Rob,Robbie,Boss` becomes one nickname `"Rob,Robbie,Boss"`,
  because EDS hands the value back as one string.

### 3.3 Email and phone (`EMAIL`, `TEL`)

- EDS files `EMAIL` lines by position into `E_CONTACT_EMAIL_1` to `_4`, and
  keeps all of them in the `E_CONTACT_EMAIL` list. `card_to_vcard` sorts by
  `(pref, key)`, so the preferred address lands in `EMAIL_1`.
- EDS has 19 phone fields and picks one by the line's `TYPE` set. A line
  with `TYPE=WORK,HOME` would fill two fields that overwrite each other, so
  `context_slot` writes at most one context (`WORK`, else `HOME`) and
  `feature_slot` at most one feature (`CELL` > `PAGER` > `FAX` > `VOICE` >
  `VIDEO`). `TYPE=MOBILE` is read as `CELL`; output always says `CELL`.
- `states_context` and `states_phone_feature` keep a context or feature that
  was not written from being read as deleted.

### 3.4 Addresses (`ADR`, `LABEL`)

- `ADR` has seven components: post office box, extended (apartment),
  street, locality, region, postcode, country. The house `number` is joined
  to the street `name`; `restore_address_components` splits them again if
  the street was not edited.
- EDS has three address slots (work, home, other), each with a label field
  that it writes as a separate `LABEL;TYPE=...` line. `label_entry` pairs a
  `LABEL` with its `ADR` by `X-JMAP-KEY`, or by context and text. A vCard 4.0
  `ADR;LABEL=...` parameter is also read.
- Components that `ADR` has no place for (`floor`, `room`) are not written.

### 3.5 Organizations and titles (`ORG`, `TITLE`, `ROLE`)

- `ORG` components: name (`E_CONTACT_ORG`), first unit
  (`E_CONTACT_ORG_UNIT`), second unit (`E_CONTACT_OFFICE`), then further
  units. An organization without a name keeps the leading semicolon
  (`ORG:;Engineering`), so that the unit does not move into the name.
- `Title` with `kind: "title"` (or none) is `TITLE`; `kind: "role"` is
  `ROLE`. Other kinds are not written.
- `Title.organizationId` is not filled: vCard does not say which
  organization a title belongs to.

### 3.6 Dates (`BDAY`, `ANNIVERSARY`, `X-EVOLUTION-ANNIVERSARY`)

- `BDAY` is `kind: "birth"`. `ANNIVERSARY` and `X-EVOLUTION-ANNIVERSARY` are
  `kind: "wedding"`.
- EDS 3.52 reads the wedding date only from `X-EVOLUTION-ANNIVERSARY`;
  EDS 3.60 reads it only from `ANNIVERSARY` (see `docs/eds-version-matrix.md`,
  (B)). Output writes both lines with the same key, and input merges them
  into one entry.
- `libebook-contacts` clamps a date to year 1000 to 9999, and a date that it
  cannot parse (a bare year, `1984-05`, `--05-20`) becomes year 0. An edit in
  Evolution would then save `1000-01-01`. So `anniversary_date` writes only
  complete dates with a year of 1000 or later. `states_anniversary` is false
  for other dates and for other kinds (`death`, custom), so the save leaves
  them alone on the server.
- A `Timestamp` is written as its UTC day. `states_a_point_in_time` tells the
  save path to replace the whole value if the day is edited.
- Input dates in extended form (`1985-04-12`) are rewritten to basic form
  (`19850412`) by `normalize_vcard_dates` before `calcard` parses them,
  because `calcard` reads the extended form of a date-only value only to
  the month. Partial dates, text values and other properties are not
  changed. Output uses the extended form.

### 3.7 Instant messaging (`X-AIM`, `X-JABBER`, ..., `IMPP`)

- Ten services have EDS fields: AIM, Gadu-Gadu, Google Talk, GroupWise, ICQ,
  Jabber, MSN, Matrix, Skype, Yahoo. Each has six fields
  (`E_CONTACT_IM_<SERVICE>_HOME_1..3` and `_WORK_1..3`). `service_slot`
  chooses `HOME` or `WORK` from the entry's context.
- `SERVICE_SCHEMES` maps URI schemes to services, including aliases
  (`jabber:`, `gtalk:`, `aol:`, `gadugadu:`, `gadu:`, `novell:`, `msnim:`,
  `ymsgr:`). `plain_handle` refuses URIs with a query, path or action (for
  example `skype:echo123?call`).
- vCard 4.0 `IMPP` lines are read into `onlineServices` and written back as
  the slotted `X-` properties.
- `X-TWITTER` and `X-SIP` are lists in EDS without slots, and other services
  (Telegram, Signal, Mastodon, ...) have no EDS field. They are not written
  and stay on the server.

### 3.8 Categories (`CATEGORIES`)

- `keywords` is a set; output is one `CATEGORIES` line, sorted, with commas
  and other special characters escaped. Several input lines are merged.
- EDS trims spaces when a user edits categories. `states_keyword` refuses
  empty tags, tags with leading or trailing whitespace and tags with a
  carriage return, so that EDS cannot rename them on the server.

### 3.9 Links (`URL`, `X-EVOLUTION-BLOG-URL`, `X-EVOLUTION-VIDEO-URL`, `CALURI`, `FBURL`)

- A link without `kind` is `URL`; EDS shows the first `URL` as the home page.
  `kind: "blog"` and `"video"` use the EDS properties.
- Other kinds (`contact`, `feed`, ...) are not written: on vCard 3.0 they
  could only be a `URL`, which Evolution would show as the home page.
- `Calendar` entries with `kind: "calendar"` and `"freeBusy"` are `CALURI`
  and `FBURL`.

### 3.10 Photo (`PHOTO`)

- Only `media` with `kind: "photo"` is written. Logos, sounds and other
  media stay on the server.
- Inline data is written as `PHOTO;ENCODING=b;TYPE=<subtype>`, with the
  subtype from `image_subtype`. A URI is written as `PHOTO;VALUE=uri:`; EDS
  needs `VALUE=uri` to read it, and RFC 2426 has no `TYPE` there, so a
  remote photo's `mediaType` is not carried.
- EDS writes `TYPE=X-EVOLUTION-UNKNOWN` for an unknown type; `read_photo`
  reads that as no type.
- EDS builds the media type as `image/` plus the `TYPE` value as written
  (`TYPE=PNG` gives `image/PNG`). `read_photo` does the same, so the EDS
  round trip is stable. Media types are case-insensitive (RFC 2045 §5.1).
- When the user changes the photo, EDS drops `X-JMAP-KEY`. `same_photo`
  compares the payloads (base64 padding, subtype case, URI) to tell whether
  the photo really changed.

### 3.11 Related persons (`X-EVOLUTION-SPOUSE`, `-MANAGER`, `-ASSISTANT`, `RELATED`)

- Evolution has text fields for spouse, manager and assistant. They map to
  `relatedTo` entries keyed by the person's name with the matching
  `relation`.
- `names_a_person` refuses empty names, URIs (`urn:uuid:...`, `mailto:...`)
  and names with edge whitespace or a carriage return, so that an id is not
  shown as a name.
- vCard 4.0 `RELATED;TYPE=spouse|partner|manager|assistant` and Apple
  `X-ABRELATEDNAMES` are read the same way and written as the
  `X-EVOLUTION-*` lines. `AGENT` and other relations are not written.

### 3.12 File as (`X-EVOLUTION-FILE-AS`)

Evolution's "File under" field. `X-EVOLUTION-FILE-AS`, `FILE-AS` and
`X-FILE-AS` are read into `fileAs`; output is `X-EVOLUTION-FILE-AS`.
`sortAs` is kept separately and is not written.

---

## 4. Not mapped

| Data | Why |
| :--- | :--- |
| Unknown `X-` properties (`X-MOZILLA-HTML`, `X-APPLE-*`, `X-MS-*`, `X-SIGNAL`, ...) | No JSContact meaning and no EDS field. Not read into `extra`. |
| `GEO`, `TZ` | No Evolution field. JSContact puts coordinates on addresses and `timeZone` is an IANA name. Server values stay. |
| `MAILER`, `CLASS` | Removed in vCard 4.0; no Evolution field. |
| `PRODID` | Describes the exporter, not the contact. |
| `REV` | `updated` belongs to the server. |
| `SORT-STRING` | `sortAs` stays on the server. |
| `SOUND`, `LOGO` | Only `kind: "photo"` media is written. |
| `KEY` | No contact certificate field in Evolution. `cryptoKeys` stay on the server. |
| `AGENT` | Wider than "assistant"; writing it as one would change its meaning. |
| `GENDER`, `CLIENTPIDMAP`, `MEMBER`, `KIND:group`, `X-EVOLUTION-LIST` | Groups and lists are not contacts. |

`ALTID`/`LANGUAGE` alternates: `FN` and `N` use the first in document order;
multi-valued properties keep each alternate as its own keyed entry.

---

## 5. Input from other exporters

Output is always vCard 3.0 in UTF-8. Input is accepted more widely:

- **vCard 2.1**: bare type names without `TYPE=` (`TEL;WORK;VOICE`), bare
  `PREF`, `CHARSET` (UTF-8, ISO-8859-1, Windows-1252), `ENCODING=QUOTED-PRINTABLE`
  (decoded with the given charset, ISO-8859-1 by default) and bare photo
  formats (`PHOTO;JPEG;ENCODING=BASE64`).
- **vCard 4.0**: `IMPP`, `RELATED`, `ANNIVERSARY`, `PHOTO` as a `data:` or
  `http(s):` URI without `VALUE=uri`, `MEDIATYPE`, quoted and comma-separated
  `TYPE` values, `PREF=1`, and `tel:` URIs.
- **Apple** property groups (`itemN.PROPERTY` with `itemN.X-ABLabel`):
  `clean_apple_label` removes the `_$!<...>!$_` markers. Known labels map to
  contexts and features (`Work`, `Home`, `Mobile`, `Pager`, `WorkFAX`,
  `HomeFAX`, `Main`, `HomePage`, `Blog`); other labels go to
  `extra["label"]`. `X-ABRELATEDNAMES` and `X-ABDATE` map to `relatedTo`
  and `anniversaries`.
- Folded input (CRLF followed by a space or tab) is unfolded. `\n` and `\N`
  are both newlines.

Output escapes `\`, `,`, `;` and newlines, and `calcard` folds lines at 75
octets without splitting a UTF-8 character.

---

## 6. Server behaviour

These differences between `jmap-vcard` and Stalwart's `ContactCard/parse`
(`jmap-client/examples/contact-parse-probe.rs`) and CardDAV storage
(`jmap-client/examples/carddav-put-probe.rs`) need no change:

| Behaviour | Why it is fine |
| :--- | :--- |
| The server chooses its own map keys (`k1`, `k2`, ...) | Map keys are the producer's choice (RFC 9553 §1.4.3). See also `docs/ICAL-MAPPING.md` section 1, item 4. |
| `/parse` sets `uid` but not `id` | `/parse` files nothing, so there is no `id` (RFC 9610 §3.4). See `docs/ICAL-MAPPING.md` section 3.1 for the `UID`/`id` rule. |
| The server adds `prodId`, `updated` and its own `vCard` conversion notes | Store-owned data; the client does not invent it (`docs/ICAL-MAPPING.md` section 9.1). |
| The server drops `X-EVOLUTION-*` and `X-JABBER`/`X-MATRIX` lines | A generic server need not know vendor extensions. `jmap-vcard` must keep what EDS needs. |
| The server adds an `internet` context for `TYPE=INTERNET` | RFC 9553 defines no such context; `jmap-vcard` drops `INTERNET`, which states a medium, not a context. |
| The server omits `mediaType` on a photo | `mediaType` is optional (RFC 9553 §2.6.4). |
| The server sets `Title.organizationId` to the first organization | vCard has no such link (section 3.5). |
| CardDAV stores the card as vCard 4.0 (basic-form dates, `data:` photos) | RFC 6352 allows a different representation. `vcard_to_card` reads the stored form back to the same card. |

---

## 7. Fixed point and tests

The round trip `vCard1 -> Card1 -> vCard2 -> Card2 -> vCard3 -> Card3 ->
vCard4` must give `vCard3 == vCard4` byte for byte and `Card2 == Card3`.

| Where | What |
| :--- | :--- |
| `jmap-vcard/tests/mapping.rs` | Unit tests per rule; `real_exporter_fixture_corpus_table_driven_roundtrip` over the fixtures |
| `jmap-vcard/tests/fixtures/` | Exports from Evolution, Google (vCard 3.0 and 4.0), iCloud/macOS, Outlook (vCard 2.1 and 3.0), Nextcloud, SOGo, Thunderbird (vCard 4.0), plus `synthetic_two_organizations_bare_role.vcf` |
| `jmap-vcard/tests/proptest_fuzz.rs` | Fixed point over generated cards and vCard lines, per domain (`prop_fixpoint_*`); `identify_oscillating_vcard_property` and `identify_oscillating_card_field` name the property that does not converge |
| `jmap-vcard/tests/hostile.rs` | Hostile input |
| `jmap-vcard/tests/server_roundtrip.rs` | Round trips against mock server responses |
| `jmap-functional/tests/address-book.rs` | The same rules through real EDS (`docs/functional-tests.md`) |
