<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# vCard to JSContact mapping

This document describes how `jmap-vcard` maps the vCard that Evolution Data
Server (EDS) gives to the address book backend to a JMAP `ContactCard`, and
back.

- vCard: 3.0 (RFC 2426) on output. 2.1, 3.0 and 4.0 (RFC 6350, RFC 6474,
  RFC 6473) on input. `calcard` parses and writes it.
- JSContact: RFC 9553 and RFC 9555. JMAP for Contacts: RFC 9610.
- EDS: the `EContactField` enum of `libebook-contacts` 3.52 (`E_CONTACT_*`), as
  `eds-sys` binds it.

This table shows where the code lives:

| Crate / file | Role |
| :--- | :--- |
| `jmap-vcard/src/contact.rs` | `card_to_vcard`, `vcard_to_card`, the `states_*` and `maps_*` predicates |
| `jmap-book-sync` | Save path: turns an edited vCard into a `ContactCard/set` PatchObject |
| `jmap-proto/src/contacts.rs` | The `ContactCard` type |

---

## 1. Design

1. **Edits are patches, not replacements.** `jmap-vcard` maps only what the
   contact editor of Evolution shows. It does not write other JSContact data to
   the vCard. Examples of such data are `preferredLanguages`, `localizations`,
   `cryptoKeys`, `pronouns` and unmapped relations. `jmap-book-sync` saves with
   a PatchObject that names only mapped paths that the user edited. The other
   data therefore stays on the server.
2. **Predicates guard properties that the mapping shows only in part.** A
   missing field in an edited vCard means "deleted" only if the mapping could
   show the field. The predicates say whether the mapping showed a value. The
   predicates are `states_context`, `states_phone_feature`,
   `states_address_component`, `states_spouse`, `states_keyword` and the other
   `states_*` functions.
3. **Map keys travel with the line.** Entries with several values carry
   `X-JMAP-KEY=<key>`. `entry_key` reads it back. An entry without a key gets a
   new deterministic key (`e1`, `p1`, `a1`, ...).
4. **Fixed point.** `card_to_vcard(vcard_to_card(card_to_vcard(c)))` equals
   `card_to_vcard(c)` (section 7).

---

## 2. Property table

| vCard Property | vCard Parameters | JSContact Field (RFC 9553/9555) | EDS Field (`EContactField`) | Primary Helpers & Predicates | Lossy / Product Decision Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **`UID`** | | `card.id` (or `card.uid`) | `E_CONTACT_UID` | `card_to_vcard`, `vcard_to_card` | `UID` carries the JMAP ID of the server for the EDS cache index. `X-JMAP-UID` carries the original JSContact UUID. |
| **`X-JMAP-UID`** | | `card.uid` | | `card_to_vcard`, `vcard_to_card` | Keeps the client-side JSContact UUID across vCard round trips. |
| **`FN`** | `ALTID`, `LANGUAGE` | `card.name.full` | `E_CONTACT_FULL_NAME` | `derive_full`, `read_name` | The mapping selects the first `FN` in document order. If `FN` is absent, the mapping synthesizes it from `name.components`. |
| **`N`** | `ALTID`, `LANGUAGE` | `card.name.components` (`surname`, `given`, `given2`, `title`, `credential`) | `E_CONTACT_FAMILY_NAME`, `E_CONTACT_GIVEN_NAME`, `E_CONTACT_ADDITIONAL_NAME`, `E_CONTACT_NAME_PREFIX`, `E_CONTACT_NAME_SUFFIX` | `name_fields`, `read_name`, `states_name_component`, `restore_name_components` | Structured name with 5 components. Double-barrelled given names share index 1. `restore_name_components` reconstructs them. The mapping drops the name `phonetic`. |
| **`NICKNAME`** | `X-JMAP-KEY`, `ALTID` (input also accepts `TYPE` and `PREF`) | `card.nicknames` (`Nickname.name`, `contexts`, `pref`) | `E_CONTACT_NICKNAME` | `states_nickname`, `entry_text_list`, `read_flags` | The mapping emits one line for each keyed entry (not comma-separated) to keep `X-JMAP-KEY`. Input reads `TYPE` into `contexts` and `PREF` into `pref`. Output writes neither parameter, because the EDS profile has none. The mapping keeps commas in nicknames literally. |
| **`EMAIL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.emails` (`ContactEmail.address`, `contexts`, `pref`) | `E_CONTACT_EMAIL_1` .. `_4` | `states_email`, `type_names`, `read_flags` | EDS files emails by position (`EMAIL_1` is primary). On output, the mapping sorts by `(pref, key)`. The lowest `pref` therefore lands in `EMAIL_1`. Several contexts are allowed. |
| **`TEL`** | `TYPE` (`WORK`, `HOME`, `CELL`, `MOBILE`, `PAGER`, `FAX`, `VOICE`, `VIDEO`, `PREF`), `X-JMAP-KEY` | `card.phones` (`ContactPhone.number`, `contexts`, `features`, `pref`) | `E_CONTACT_PHONE_PRIMARY`, `_BUSINESS`, `_BUSINESS_2`, `_BUSINESS_FAX`, `_HOME`, `_HOME_2`, `_HOME_FAX`, `_MOBILE`, `_PAGER`, `_OTHER`, `_OTHER_FAX`, `_CAR`, `_ISDN`, `_CALLBACK`, `_COMPANY`, `_RADIO`, `_TELEX`, `_TTYTDD`, `_ASSISTANT` | `states_phone`, `context_slot`, `states_context`, `feature_slot`, `states_phone_feature` | Full matrix of 19 EDS fields. Input accepts `MOBILE` as a synonym for `CELL`. Output normalizes it to `CELL`. The mapping narrows the context to at most 1 (`WORK` > `HOME` -> `DEFAULT_SLOT`). It narrows the feature to at most 1 (`CELL`/`MOBILE` > `PAGER` > `FAX` > `VOICE` > `VIDEO`). The predicates preserve unstated features. |
| **`ADR`** | `TYPE` (`WORK`, `HOME`, `PREF`), `LABEL`, `X-JMAP-KEY` | `card.addresses` (`Address.components`, `contexts`, `extra["pref"]`) | `E_CONTACT_ADDRESS_WORK`, `_HOME`, `_OTHER` (+ 7 subfields per slot) | `address_fields`, `read_address`, `states_address`, `states_address_component`, `restore_address_components` | 7 components: PO Box, Ext, Street, Locality, Region, Postcode, Country. The house `number` joins the street `name`. `restore_address_components` reconstructs split components. The mapping drops unmapped kinds (`floor`, `room`). |
| **`LABEL`** | `TYPE` (`WORK`, `HOME`, `PREF`), `X-JMAP-KEY` | `card.addresses` (`Address.full`) | `E_CONTACT_ADDRESS_LABEL_WORK`, `_HOME`, `_OTHER` | `address_label`, `label_entry`, `read_address` | A separate line, after `ADR` or on its own. On input, the mapping matches by `X-JMAP-KEY` or by context and text. This prevents duplicate addresses. |
| **`ORG`** | `X-JMAP-KEY` | `card.organizations` (`Organization.name`, `units`) | `E_CONTACT_ORG` (name), `E_CONTACT_ORG_UNIT` (dept), `E_CONTACT_OFFICE` (office) | `organization_components`, `read_organization`, `states_organization`, `states_org_unit` | Semicolon-delimited list. Index 0 = Name. Index 1 = Department. Index 2 = Office. Index 3 and higher = trailing units. An organization without a name keeps the leading semicolon (`ORG:;Unit`). `sortAs` and `contexts` are unmapped on vCard 3.0. |
| **`TITLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "title"`) | `E_CONTACT_TITLE` | `read_title`, `states_title`, `title_kind` | `kind: "title"` (or `None`) maps to `TITLE`. The mapping drops vendor kinds. |
| **`ROLE`** | `X-JMAP-KEY` | `card.titles` (`Title.name`, `kind: "role"`) | `E_CONTACT_ROLE` | `read_title`, `states_title`, `title_kind` | `kind: "role"` maps to `ROLE`. |
| **`NOTE`** | `X-JMAP-KEY` | `card.notes` (`Note.note`) | `E_CONTACT_NOTE` | `states_note` | Free text. The first line lands in EDS `E_CONTACT_NOTE`. RFC 9553 `created` and `author` ride in `extra`. Sync does not touch them. |
| **`URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: None`) | `E_CONTACT_HOMEPAGE_URL` | `states_link`, `maps_link_kind` | Plain websites (`kind: None`) map to `URL`. `mediaType`, `label`, `contexts` and `pref` ride in `extra`. |
| **`X-EVOLUTION-BLOG-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "blog"`) | `E_CONTACT_BLOG_URL` | `states_link`, `maps_link_kind` | EDS blog URL field. Maps to `links` with `kind: "blog"`. |
| **`X-EVOLUTION-VIDEO-URL`** | `X-JMAP-KEY` | `card.links` (`Link.uri`, `kind: "video"`) | `E_CONTACT_VIDEO_URL` | `states_link`, `maps_link_kind` | EDS video stream URL field. Maps to `links` with `kind: "video"`. |
| **`CALURI`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "calendar"`) | `E_CONTACT_CALENDAR_URI` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property. The mapping emits it on vCard 3.0 for compatibility with EDS 3.52. `ICSCALENDAR` is excluded. |
| **`FBURL`** | `X-JMAP-KEY` | `card.calendars` (`Calendar.uri`, `kind: "freeBusy"`) | `E_CONTACT_FREEBUSY_URL` | `states_calendar`, `calendar_property`, `calendar_kind` | vCard 4.0 property. The mapping emits it on vCard 3.0 for compatibility with EDS 3.52. |
| **`ANNIVERSARY`** | `X-JMAP-KEY`, `VALUE=date` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, `Day` | Standard anniversary property of vCard 4.0 (RFC 6350 §6.4.3). Maps to `kind: "wedding"`. Output emits both `X-EVOLUTION-ANNIVERSARY` (EDS 3.52) and `ANNIVERSARY` (EDS 3.60 and later). |
| **`RELATED`** | `TYPE` (`spouse`, `partner`, `manager`, `assistant`, custom) | `card.related_to` (Key = Person Name, `relation: { "<type>": true }`) | `E_CONTACT_SPOUSE`, `_MANAGER`, `_ASSISTANT` | `names_a_person`, `vcard_to_card` | Standard relationship property of vCard 4.0 (RFC 6350 §6.6.6). `TYPE=spouse` or `partner` maps to `spouse`. `TYPE=manager` maps to `manager`. `TYPE=assistant` maps to `assistant`. A custom relation maps if `names_a_person` holds. Output normalizes to the standard `X-EVOLUTION-*` properties. |
| **`IMPP`** | `TYPE` (`WORK`, `HOME`), `X-JMAP-KEY` | `card.online_services` (`service`, `user`) | `E_CONTACT_IM_<SERVICE>_HOME_1..3`, `_WORK_1..3` | `SERVICE_SCHEMES`, `plain_handle`, `vcard_to_card` | Standard instant messaging property of vCard 4.0 (RFC 4770 / RFC 6350 §6.4.3). The mapping maps the URI schemes (`xmpp:`, `skype:`, `matrix:`, `aim:`, `icq:`, `msn:`, `yahoo:`, `groupwise:`, `gg:`, `gtalk:`) to the matching service handle. It rejects action and query URIs. Output normalizes to the slotted `X-` properties (`X-JABBER`, `X-SKYPE`, `X-MATRIX`, and so on). |
| **`PHOTO`** | `TYPE`, `MEDIATYPE`, `ENCODING=b`, `VALUE=uri`, `X-JMAP-KEY` | `card.media` (`Media.uri`, `media_type`, `kind: "photo"`) | `E_CONTACT_PHOTO` | `photo`, `read_photo`, `states_media`, `same_photo`, `image_subtype` | The mapping maps only `kind: "photo"`. Input accepts RFC 2397 `data:` URIs (directly, without double base64 encoding), direct HTTP/HTTPS URIs without `VALUE=uri`, and `MEDIATYPE` parameters (RFC 6350 §5.7). Output uses the canonical vCard 3.0 base64 with `ENCODING=b`. EDS drops `X-JMAP-KEY` when the user edits the photo. `same_photo` pairs the photos again. |
| **`CATEGORIES`** | | `card.keywords` (`Set<String>`) | `E_CONTACT_CATEGORY_LIST` | `drawn_tags`, `read_keywords`, `states_keyword` | The mapping emits one sorted line. The values are comma-separated on the wire. Trimming protection: the mapping omits tags with leading or trailing whitespace or carriage returns. This prevents corruption in EDS. |
| **`BDAY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "birth"`, `date`) | `E_CONTACT_BIRTH_DATE` | `read_anniversary`, `states_anniversary`, `anniversary_date`, `states_a_point_in_time`, `Day` | One calendar day, formatted `YYYY-MM-DD`. The mapping omits truncated or bare years (`1984`) and years below 1000. This prevents the corruption that EDS causes when it clamps a date (`1000..=9999`). The mapping converts a `Timestamp` to its UTC day. |
| **`X-EVOLUTION-ANNIVERSARY`** | `X-JMAP-KEY` | `card.anniversaries` (`kind: "wedding"`, `date`) | `E_CONTACT_ANNIVERSARY` | `read_anniversary`, `states_anniversary`, `anniversary_date` | Wedding anniversary field of EDS 3.52. The date validation and the clamping rule for years of 1000 and later are the same as for `BDAY`. Output emits both `X-EVOLUTION-ANNIVERSARY` and `ANNIVERSARY` for compatibility with both versions. The mapping drops `kind: "death"`. |
| **`X-EVOLUTION-SPOUSE`** | | `card.related_to` (Key = Person Name, `relation: { "spouse": true }`) | `E_CONTACT_SPOUSE` | `states_spouse`, `states_nothing_but_the_marriage`, `names_a_person` | The key in the JSContact `related_to` is the name of the spouse (RFC 9555 §2.9.5). No `X-JMAP-KEY` is necessary. The mapping drops keys that are not a person (for example URIs). |
| **`X-EVOLUTION-MANAGER`** | | `card.related_to` (Key = Person Name, `relation: { "manager": true }`) | `E_CONTACT_MANAGER` | `states_manager`, `names_a_person` | The key in the JSContact `related_to` is the name of the manager (RFC 9553 §2.1.8). No `X-JMAP-KEY` is necessary. The mapping drops keys that are not a person (for example URIs). |
| **`X-EVOLUTION-ASSISTANT`** | | `card.related_to` (Key = Person Name, `relation: { "assistant": true }`) | `E_CONTACT_ASSISTANT` | `states_assistant`, `names_a_person` | The key in the JSContact `related_to` is the name of the assistant (RFC 9553 §2.1.8). No `X-JMAP-KEY` is necessary. The mapping drops keys that are not a person (for example URIs). |
| **`X-AIM`** | `TYPE` (`WORK`, `HOME`), `X-JMAP-KEY` | `card.online_services` (`service: "AIM"`, `user`, `uri: "aim:..."`) | `E_CONTACT_IM_AIM_HOME_1..3`, `_WORK_1..3` | `drawn_service`, `states_online_service`, `online_service_handle`, `online_service_uri`, `service_slot` | 6 EDS slots for each service. The mapping extracts handles from `user` or from bare URI schemes (`SERVICE_SCHEMES`). It rejects action and query URIs. `TYPE` is mandatory, so that the field is visible in EDS. |
| **`X-GADUGADU`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Gadu-Gadu"`, `uri: "gg:..."`) | `E_CONTACT_IM_GADUGADU_HOME_1..3`, `_WORK_1..3` | (same as above) | The match ignores case and punctuation (`same_service`). |
| **`X-GOOGLE-TALK`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Google Talk"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_GOOGLE_TALK_HOME_1..3`, `_WORK_1..3` | (same as above) | Handles use the `xmpp` scheme. |
| **`X-GROUPWISE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "GroupWise"`, `uri: "groupwise:..."`) | `E_CONTACT_IM_GROUPWISE_HOME_1..3`, `_WORK_1..3` | (same as above) | The mapping maps bare handles. |
| **`X-ICQ`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "ICQ"`, `uri: "icq:..."`) | `E_CONTACT_IM_ICQ_HOME_1..3`, `_WORK_1..3` | (same as above) | Numeric UIN handles. |
| **`X-JABBER`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Jabber"`, `uri: "xmpp:..."`) | `E_CONTACT_IM_JABBER_HOME_1..3`, `_WORK_1..3` | (same as above) | XMPP JID handles. |
| **`X-MSN`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "MSN"`, `uri: "msn:... / msnim:..."`) | `E_CONTACT_IM_MSN_HOME_1..3`, `_WORK_1..3` | (same as above) | The mapping supports the `msn` and `msnim` schemes. |
| **`X-MATRIX`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Matrix"`, `uri: "matrix:..."`) | `E_CONTACT_IM_MATRIX_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Matrix handles (`matrix:@user:domain` without action queries). |
| **`X-SKYPE`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Skype"`, `uri: "skype:..."`) | `E_CONTACT_IM_SKYPE_HOME_1..3`, `_WORK_1..3` | (same as above) | Bare Skype user names. The mapping rejects an action such as `skype:echo123?call`. |
| **`X-YAHOO`** | `TYPE`, `X-JMAP-KEY` | `card.online_services` (`service: "Yahoo"`, `uri: "yahoo:... / ymsgr:..."`) | `E_CONTACT_IM_YAHOO_HOME_1..3`, `_WORK_1..3` | (same as above) | The mapping supports the `yahoo` and `ymsgr` schemes. |
| **`GEO`** | | `Address.coordinates` (RFC 9553) | `E_CONTACT_GEO` (no UI) | | Dropped by design on vCard 3.0 import and export. Evolution has no UI for coordinates. `PatchObject` preserves the server-side `Address.coordinates`. |
| **`TZ`** | | `card.time_zone` (RFC 9553) | | | Dropped by design on vCard 3.0 import and export. The mapping ignores text and Olson time zones in vCard 4.0 (`TZ:Europe/Berlin`) on import. `PatchObject` preserves the server `time_zone`. |
| **`MAILER`** | | | `E_CONTACT_MAILER` (legacy) | | Dropped by design. RFC 6350 (vCard 4.0) deprecates it. It holds metadata about legacy email client software. |
| **`PRODID`** | | `card.prod_id` (RFC 9553) | | | Dropped by design on import and export. The generator metadata belongs to the serialization envelope. The mapping does not preserve a foreign `PRODID` across saves. |
| **`REV`** | | `card.updated` (RFC 9553) | `E_CONTACT_REV` | | Dropped by design on import and export. The JMAP server alone owns the revision timestamp on commit. |
| **`GENDER`** | | | | | vCard 4.0 property (RFC 6350 §6.5.1). The reader ignores it. It does not pollute `card.extra`. |
| **`CLIENTPIDMAP`** | | | | | vCard 4.0 property (RFC 6350 §6.7.7). The reader ignores it. It does not pollute `card.extra`. |
| **`MEMBER`** | | | | | vCard 4.0 group member property (RFC 6350 §6.6.5). The reader ignores it for individual contacts. |
| **`SORT-STRING`** | | `Name.sortAs` / `Org.sortAs` | | | Dropped by design from vCard 3.0 output. RFC 6350 replaces it with the `SORT-AS` parameter. `PatchObject` preserves the JSContact `sortAs` on the server. It does not overwrite `fileAs`. |
| **`X-EVOLUTION-FILE-AS`** | | `Name.extra["fileAs"]` / `card.extra["fileAs"]` | `E_CONTACT_FILE_AS` | `states_file_as` | The "File Under" field of Evolution. Input accepts `X-EVOLUTION-FILE-AS`, `FILE-AS` and `X-FILE-AS`. Output normalizes to `X-EVOLUTION-FILE-AS`. It coexists with `sortAs` and does not overwrite it. |
| **`CLASS`** | | `card.privacy` (RFC 9553) | | | Dropped by design. RFC 6350 deprecates and removes it. It is a legacy access classification. Evolution has no editor UI for it. |
| **`SOUND`** | `TYPE`, `ENCODING=b`, `VALUE=uri`, `MEDIATYPE` | `card.media` (`kind: "sound"`) | | `states_media` | Dropped by design from vCard 3.0. `states_media` permits only `kind: "photo"`. The reader ignores audio payloads and URIs on input. They do not corrupt photos. `PatchObject` preserves the server `sound` media entries. |
| **`AGENT`** | `VALUE=uri`, text, nested vCard | `card.related_to` (`relation: { "agent": true }`) | | `states_spouse`, `states_manager`, `states_assistant` | Dropped by design from vCard 3.0 output. RFC 6350 deprecates and removes it. The reader ignores nested vCards, URIs and text on input. They do not corrupt the surrounding card. They do not pollute `card.extra`. `PatchObject` preserves the server `relatedTo` agent entries. |
| **`LOGO`** | `TYPE`, `ENCODING=b`, `VALUE=uri` | `card.media` (`kind: "logo"`) | `E_CONTACT_LOGO` (no UI) | `states_media` | Dropped by design from vCard 3.0. The Evolution editor supports only the personal photo (`E_CONTACT_PHOTO`). `PatchObject` preserves the server `logo` entries. |
| **`KEY`** | `TYPE` (`X509`, `PGP`), `ENCODING=b`, `VALUE=uri` | `card.extra["cryptoKeys"]` (RFC 9553 §2.7.1) | `E_CONTACT_X509_CERT`, `E_CONTACT_PGP_CERT` (no UI) | | Dropped by design from vCard 3.0. The Evolution editor has no contact certificate UI. `PatchObject` preserves the server `cryptoKeys` entries. |
| **`itemN.PROPERTY`** | `X-ABLabel` companion | (associated property) | (associated EDS slot) | `clean_apple_label`, `vcard_to_card` | Apple property groups (RFC 2426 §2.1.1). `calcard` parses the group prefix. The companion `X-ABLabel` maps contexts (`Work`, `Home`), features (`Mobile`, `Pager`, `Fax`) or custom labels. |
| **`X-ABLabel`** | | `extra["label"]` or mapped context/feature | | `clean_apple_label`, `vcard_to_card` | Apple label annotation. The mapping unwraps the markers (`_$!<Label>!$_`). Standard labels map to JSContact contexts and features. The mapping preserves custom labels in `extra["label"]`. |
| **`X-ABRELATEDNAMES`** | `X-ABLabel` | `card.related_to` (Key = Person Name) | `E_CONTACT_SPOUSE`, `_MANAGER`, `_ASSISTANT` | `clean_apple_label`, `vcard_to_card` | Apple relationship property. The group companion `X-ABLabel` selects the relation type (`spouse`, `manager`, `assistant`, custom). Output normalizes to the standard `X-EVOLUTION-*` properties. |
| **`X-ABDATE`** | `X-ABLabel` | `card.anniversaries` (`kind`, `date`) | `E_CONTACT_ANNIVERSARY` | `clean_apple_label`, `read_day` | Apple date property. The group companion `X-ABLabel` selects the anniversary kind (`wedding`, `birth`, custom). Output normalizes to `X-EVOLUTION-ANNIVERSARY` / `BDAY`. |

---

## 3. Fields

### 3.1 Identity

- `UID` carries the JMAP `id`, because EDS keys its cache on `UID`.
  `X-JMAP-UID` carries the JSContact `uid` if it differs.
- A contact that the user creates in Evolution has a local `UID`.
  `jmap-book-sync` does not send it as the `id`. The server assigns an `id`.

### 3.2 Names (`FN`, `N`, `NICKNAME`)

- `FN`: the first `FN` in document order. If the card has no `full` name,
  `derive_full` builds one from the components (title, given, given2, surname,
  credential).
- `N`: `[surname, given, given2, title, credential]`. The mapping joins several
  components of one kind (for example two `given` parts) with a space. On save,
  `restore_name_components` puts the parts back if the field still reads as the
  joined parts. If the user edited the field, the new text replaces the parts.
  The mapping does not write `phonetic`. It stays on the server.
- `NICKNAME`: one line for each keyed entry, so that each entry keeps its
  `X-JMAP-KEY`. EDS shows the first line. A foreign `NICKNAME:Rob,Robbie,Boss`
  becomes one nickname `"Rob,Robbie,Boss"`, because EDS gives the value back as
  one string.

### 3.3 Email and phone (`EMAIL`, `TEL`)

- EDS files `EMAIL` lines by position into `E_CONTACT_EMAIL_1` to `_4`. It keeps
  all of them in the `E_CONTACT_EMAIL` list. `card_to_vcard` sorts by
  `(pref, key)`, so the preferred address lands in `EMAIL_1`.
- EDS has 19 phone fields. It picks one field from the `TYPE` set of the line.
  A line with `TYPE=WORK,HOME` would fill two fields that overwrite each other.
  `context_slot` therefore writes at most one context (`WORK`, else `HOME`).
  `feature_slot` writes at most one feature (`CELL` > `PAGER` > `FAX` > `VOICE`
  > `VIDEO`). The mapping reads `TYPE=MOBILE` as `CELL`. The output always
  says `CELL`.
- `states_context` and `states_phone_feature` prevent a context or feature that
  the mapping did not write from being read as deleted.

### 3.4 Addresses (`ADR`, `LABEL`)

- `ADR` has seven components: post office box, extended (apartment), street,
  locality, region, postcode, country. The mapping joins the house `number` to
  the street `name`. `restore_address_components` splits them again if the user
  did not edit the street.
- EDS has three address slots (work, home, other). Each slot has a label field
  that EDS writes as a separate `LABEL;TYPE=...` line. `label_entry` pairs a
  `LABEL` with its `ADR` by `X-JMAP-KEY`, or by context and text. The mapping
  also reads a vCard 4.0 `ADR;LABEL=...` parameter.
- The mapping does not write the components that `ADR` has no place for
  (`floor`, `room`).

### 3.5 Organizations and titles (`ORG`, `TITLE`, `ROLE`)

- `ORG` components: name (`E_CONTACT_ORG`), first unit (`E_CONTACT_ORG_UNIT`),
  second unit (`E_CONTACT_OFFICE`), then further units. An organization without
  a name keeps the leading semicolon (`ORG:;Engineering`). The unit therefore
  does not move into the name.
- `Title` with `kind: "title"` (or none) is `TITLE`. `kind: "role"` is `ROLE`.
  The mapping does not write other kinds.
- The mapping does not fill `Title.organizationId`. vCard does not say to which
  organization a title belongs.

### 3.6 Dates (`BDAY`, `ANNIVERSARY`, `X-EVOLUTION-ANNIVERSARY`)

- `BDAY` is `kind: "birth"`. `ANNIVERSARY` and `X-EVOLUTION-ANNIVERSARY` are
  `kind: "wedding"`.
- EDS 3.52 reads the wedding date only from `X-EVOLUTION-ANNIVERSARY`. EDS 3.60
  reads it only from `ANNIVERSARY` (see `docs/eds-version-matrix.md`, (B)). The
  output writes both lines with the same key. The input merges them into one
  entry.
- `libebook-contacts` clamps a date to the years 1000 to 9999. A date that it
  cannot parse (a bare year, `1984-05`, `--05-20`) becomes year 0. An edit in
  Evolution would then save `1000-01-01`. `anniversary_date` therefore writes
  only complete dates with a year of 1000 or later. `states_anniversary` is
  false for other dates and for other kinds (`death`, custom). The save then
  leaves them alone on the server.
- The mapping writes a `Timestamp` as its UTC day. `states_a_point_in_time`
  tells the save path to replace the whole value if the user edits the day.
- `normalize_vcard_dates` rewrites input dates in extended form (`1985-04-12`)
  to basic form (`19850412`) before `calcard` parses them. `calcard` reads the
  extended form of a date-only value only to the month. The function does not
  change partial dates, text values and other properties. The output uses the
  extended form.

### 3.7 Instant messaging (`X-AIM`, `X-JABBER`, ..., `IMPP`)

- Ten services have EDS fields: AIM, Gadu-Gadu, Google Talk, GroupWise, ICQ,
  Jabber, MSN, Matrix, Skype, Yahoo. Each service has six fields
  (`E_CONTACT_IM_<SERVICE>_HOME_1..3` and `_WORK_1..3`). `service_slot` chooses
  `HOME` or `WORK` from the context of the entry.
- `SERVICE_SCHEMES` maps URI schemes to services, including aliases (`jabber:`,
  `gtalk:`, `aol:`, `gadugadu:`, `gadu:`, `novell:`, `msnim:`, `ymsgr:`).
  `plain_handle` refuses URIs with a query, a path or an action (for example
  `skype:echo123?call`).
- The mapping reads vCard 4.0 `IMPP` lines into `onlineServices`. It writes them
  back as the slotted `X-` properties.
- `X-TWITTER` and `X-SIP` are lists in EDS without slots. Other services
  (Telegram, Signal, Mastodon, ...) have no EDS field. The mapping does not
  write them. They stay on the server.

### 3.8 Categories (`CATEGORIES`)

- `keywords` is a set. The output is one sorted `CATEGORIES` line. The mapping
  escapes commas and other special characters. It merges several input lines.
- EDS trims spaces when a user edits categories. `states_keyword` refuses empty
  tags, tags with leading or trailing whitespace and tags with a carriage
  return. EDS therefore cannot rename them on the server.

### 3.9 Links (`URL`, `X-EVOLUTION-BLOG-URL`, `X-EVOLUTION-VIDEO-URL`, `CALURI`, `FBURL`)

- A link without `kind` is `URL`. EDS shows the first `URL` as the home page.
  `kind: "blog"` and `"video"` use the EDS properties.
- The mapping does not write other kinds (`contact`, `feed`, ...). On vCard 3.0,
  they could only be a `URL`. Evolution would show it as the home page.
- `Calendar` entries with `kind: "calendar"` and `"freeBusy"` are `CALURI` and
  `FBURL`.

### 3.10 Photo (`PHOTO`)

- The mapping writes only `media` with `kind: "photo"`. Logos, sounds and other
  media stay on the server.
- The mapping writes inline data as `PHOTO;ENCODING=b;TYPE=<subtype>`. The
  subtype comes from `image_subtype`. The mapping writes a URI as
  `PHOTO;VALUE=uri:`. EDS needs `VALUE=uri` to read it. RFC 2426 has no `TYPE`
  there. The `mediaType` of a remote photo is therefore not carried.
- EDS writes `TYPE=X-EVOLUTION-UNKNOWN` for an unknown type. `read_photo` reads
  that as no type.
- EDS builds the media type as `image/` plus the `TYPE` value as written
  (`TYPE=PNG` gives `image/PNG`). `read_photo` does the same. The EDS round trip
  is therefore stable. Media types are case-insensitive (RFC 2045 §5.1).
- If the user changes the photo, EDS drops `X-JMAP-KEY`. `same_photo` compares
  the payloads (base64 padding, subtype case, URI). It finds out whether the
  photo really changed.

### 3.11 Related persons (`X-EVOLUTION-SPOUSE`, `-MANAGER`, `-ASSISTANT`, `RELATED`)

- Evolution has text fields for spouse, manager and assistant. They map to
  `relatedTo` entries. The key is the name of the person. The `relation` is the
  matching relation.
- `names_a_person` refuses empty names, URIs (`urn:uuid:...`, `mailto:...`) and
  names with whitespace at an edge or with a carriage return. An id is
  therefore not shown as a name.
- The mapping reads vCard 4.0 `RELATED;TYPE=spouse|partner|manager|assistant`
  and Apple `X-ABRELATEDNAMES` in the same way. It writes them as the
  `X-EVOLUTION-*` lines. The mapping does not write `AGENT` and other
  relations.

### 3.12 File as (`X-EVOLUTION-FILE-AS`)

This is the "File under" field of Evolution. The mapping reads
`X-EVOLUTION-FILE-AS`, `FILE-AS` and `X-FILE-AS` into `fileAs`. The output is
`X-EVOLUTION-FILE-AS`. The mapping keeps `sortAs` separately and does not write
it.

---

## 4. Not mapped

| Data | Why |
| :--- | :--- |
| Unknown `X-` properties (`X-MOZILLA-HTML`, `X-APPLE-*`, `X-MS-*`, `X-SIGNAL`, ...) | No JSContact meaning and no EDS field. The mapping does not read them into `extra`. |
| `GEO`, `TZ` | No Evolution field. JSContact puts coordinates on addresses. `timeZone` is an IANA name. The server values stay. |
| `MAILER`, `CLASS` | vCard 4.0 removed them. No Evolution field. |
| `PRODID` | It describes the exporter, not the contact. |
| `REV` | `updated` belongs to the server. |
| `SORT-STRING` | `sortAs` stays on the server. |
| `SOUND`, `LOGO` | The mapping writes only `kind: "photo"` media. |
| `KEY` | Evolution has no contact certificate field. `cryptoKeys` stay on the server. |
| `AGENT` | The meaning is wider than "assistant". If the mapping wrote it as an assistant, the meaning would change. |
| `GENDER`, `CLIENTPIDMAP`, `MEMBER`, `KIND:group`, `X-EVOLUTION-LIST` | Groups and lists are not contacts. |

`ALTID` and `LANGUAGE` alternates: `FN` and `N` use the first alternate in
document order. Properties with several values keep each alternate as its own
keyed entry.

---

## 5. Input from other exporters

The output is always vCard 3.0 in UTF-8. The input is accepted more widely:

- **vCard 2.1**:
  - bare type names without `TYPE=` (`TEL;WORK;VOICE`)
  - bare `PREF`
  - `CHARSET` (UTF-8, ISO-8859-1, Windows-1252)
  - `ENCODING=QUOTED-PRINTABLE` (decoded with the given charset, ISO-8859-1 by
    default)
  - bare photo formats (`PHOTO;JPEG;ENCODING=BASE64`)
- **vCard 4.0**:
  - `IMPP`, `RELATED`, `ANNIVERSARY`
  - `PHOTO` as a `data:` or `http(s):` URI without `VALUE=uri`
  - `MEDIATYPE`
  - quoted and comma-separated `TYPE` values
  - `PREF=1`
  - `tel:` URIs
- **Apple** property groups (`itemN.PROPERTY` with `itemN.X-ABLabel`):
  `clean_apple_label` removes the `_$!<...>!$_` markers. Known labels map to
  contexts and features (`Work`, `Home`, `Mobile`, `Pager`, `WorkFAX`,
  `HomeFAX`, `Main`, `HomePage`, `Blog`). Other labels go to `extra["label"]`.
  `X-ABRELATEDNAMES` and `X-ABDATE` map to `relatedTo` and `anniversaries`.
- The mapping unfolds folded input (CRLF followed by a space or tab). `\n` and
  `\N` are both newlines.

The output escapes `\`, `,`, `;` and newlines. `calcard` folds lines at 75
octets. It does not split a UTF-8 character.

---

## 6. Server behaviour

These differences exist between `jmap-vcard` and `ContactCard/parse` of Stalwart
(`jmap-client/examples/contact-parse-probe.rs`) and CardDAV storage
(`jmap-client/examples/carddav-put-probe.rs`). They need no change:

| Behaviour | Why it is fine |
| :--- | :--- |
| The server chooses its own map keys (`k1`, `k2`, ...) | The producer chooses the map keys (RFC 9553 §1.4.3). See also `docs/ICAL-MAPPING.md` section 1, item 4. |
| `/parse` sets `uid` but not `id` | `/parse` files nothing. There is therefore no `id` (RFC 9610 §3.4). For the `UID`/`id` rule, see `docs/ICAL-MAPPING.md` section 3.1. |
| The server adds `prodId`, `updated` and its own `vCard` conversion notes | The store owns this data. The client does not invent it (`docs/ICAL-MAPPING.md` section 9.1). |
| The server drops `X-EVOLUTION-*` and `X-JABBER`/`X-MATRIX` lines | A generic server does not need to know vendor extensions. `jmap-vcard` must keep what EDS needs. |
| The server adds an `internet` context for `TYPE=INTERNET` | RFC 9553 defines no such context. `jmap-vcard` drops `INTERNET`, because it states a medium, not a context. |
| The server omits `mediaType` on a photo | `mediaType` is optional (RFC 9553 §2.6.4). |
| The server sets `Title.organizationId` to the first organization | vCard has no such link (section 3.5). |
| CardDAV stores the card as vCard 4.0 (basic-form dates, `data:` photos) | RFC 6352 allows a different representation. `vcard_to_card` reads the stored form back to the same card. |

---

## 7. Fixed point and tests

The round trip `vCard1 -> Card1 -> vCard2 -> Card2 -> vCard3 -> Card3 ->
vCard4` must give `vCard3 == vCard4` byte for byte and `Card2 == Card3`.

| Where | What |
| :--- | :--- |
| `jmap-vcard/tests/mapping.rs` | Unit tests for each rule; `real_exporter_fixture_corpus_table_driven_roundtrip` over the fixtures |
| `jmap-vcard/tests/fixtures/` | Exports from Evolution, Google (vCard 3.0 and 4.0), iCloud/macOS, Outlook (vCard 2.1 and 3.0), Nextcloud, SOGo, Thunderbird (vCard 4.0), and `synthetic_two_organizations_bare_role.vcf` |
| `jmap-vcard/tests/proptest_fuzz.rs` | Fixed point over generated cards and vCard lines, for each domain (`prop_fixpoint_*`); `identify_oscillating_vcard_property` and `identify_oscillating_card_field` name the property that does not converge |
| `jmap-vcard/tests/hostile.rs` | Hostile input |
| `jmap-vcard/tests/server_roundtrip.rs` | Round trips against mock server responses |
| `jmap-functional/tests/address-book.rs` | The same rules through real EDS (`docs/functional-tests.md`) |

---

## 8. RFC 9555 conformance

This section compares `jmap-vcard` with the vCard conversion rules of RFC 9555.
The verdict "Match" means the code follows the rule. "Partial" means the code
follows the core rule and narrows a detail. "Deliberate divergence" means the
code differs because the EDS vCard 3.0 profile requires it.

| RFC 9555 | Rule | What `jmap-vcard` does | Verdict |
| :--- | :--- | :--- | :--- |
| 2.3.1 `ALTID` | `ALTID` groups alternate values and can be preserved through section 2.15 extension properties. | Input accepts `ALTID` parse-safely on mapped lines. The profile writes no `localizations` object and drops `ALTID` on normalized emit (`altid_parameter_metadata_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has no localization metadata field. |
| 2.3.13 `LEVEL` | `LEVEL` qualifies `EXPERTISE`, `HOBBY` and `INTEREST`. | Input parses `LEVEL` values and keeps parse safety. The EDS profile keeps no `personalInfo` field and emits none of these properties (`level_parameter_metadata_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has no personal information fields for these properties. |
| 2.3.17 `PREF` | `PREF` ranks preferred entries on multi-valued properties. | The reader accepts both `PREF=n` and `TYPE=PREF`, then maps both forms to `pref: 1` on supported properties (`entry_has_type`, `pref_parameter_rank_is_normalized_to_flag_in_eds_profile`). The writer emits preferred entries with `TYPE=...,PREF` and does not retain numeric rank on normalized export (`pref_parameter_rank_is_normalized_to_flag_in_eds_profile`). | Partial: preferred status maps, but numeric rank collapses to one preferred flag in this EDS profile. |
| 2.3.20 `SERVICE-TYPE` | `SERVICE-TYPE` converts to `onlineServices[*].service`. | For `IMPP` lines, the reader maps known URI schemes to slotted service names and sets `onlineServices[*].service`. The profile does not retain `SERVICE-TYPE` parameter metadata, and normalized emit writes EDS `X-` service slot lines with no `SERVICE-TYPE` (`service_type_parameter_metadata_is_not_retained_in_eds_profile`, `maps_an_online_service_onto_the_x_line_eds_keeps_it_on`). | Partial: service values map, but RFC 9555 `SERVICE-TYPE` metadata is not retained. |
| 2.3.24 `USERNAME` | `USERNAME` converts to `onlineServices[*].user`. | The reader sets `onlineServices[*].user` from `IMPP` URI handles and does not retain `USERNAME` parameter metadata. The writer emits EDS `X-` service lines and emits no `IMPP` or `USERNAME` parameter (`username_parameter_metadata_is_not_retained_in_eds_profile`). | Partial: user values map, but RFC 9555 `USERNAME` metadata is not retained. |
| 2.3.25 `VALUE` | `VALUE` does not convert to an IANA JSContact property. Experimental values can be preserved through section 2.15 extension properties. | The reader maps `TEL;VALUE=uri` and keeps the phone number value, but it stores no `VALUE` metadata. The writer emits `TEL` with mapped key, `TYPE`, and `PREF`, then drops `VALUE` on normalized export (`value_parameter_metadata_is_not_retained_in_eds_profile`). The writer keeps `PHOTO;VALUE=uri` for URI photos because EDS requires it for the contact photo field (`card_to_vcard`, `read_photo`). | Partial: mapped values survive, but generic `VALUE` parameter metadata is not retained in this EDS profile. |
| 2.4.2 `KIND` | `KIND` gives `kind` and allows RFC 6350, RFC 6473 and RFC 6869 values. | Input accepts `KIND` parse-safely while mapped fields still load. The profile keeps no card kind field and normalized emit writes no `KIND` line (`vcard_kind_group_and_member_lines_characterization`, `vcard_non_group_kind_variants_characterization`). | Deliberate divergence: EDS has no card kind field. |
| 2.5.2 `FN` | `FN` gives `name.full`. Derive a full name from the components if `FN` is absent. | The writer emits `name.full`, or builds it with `derive_full`. The reader takes the first `FN` (`read_name`). | Match |
| 2.5.2 `FN` | Several or parameterized `FN`: choose by `LANGUAGE`. Mark derived names with `DERIVED`. | The reader keeps the first `FN` only. The writer emits no `DERIVED` parameter. | Deliberate divergence: EDS has one display name. |
| 2.5.5 `N` | `N` gives `name.components`. | Both directions use the five classic slots. `restore_name_components` splits shared slots on save. | Partial: five slots only. |
| 2.5.5 `N` | Extended parts: `surname2`, `generation`, `JSCOMPS`, `SORT-AS`. | The mapping ignores them. `sortAs` stays in memory and is not written. | Deliberate divergence: vCard 3.0 has no such slots. |
| 2.5.6 `NICKNAME` | `NICKNAME` gives `nicknames[*].name`. | Each line becomes one keyed nickname. Commas stay in the text (`maps_nicknames_onto_nickname_lines`, `a_nickname_that_reads_as_a_list_is_still_one_nickname`). | Match |
| 2.5.6 `NICKNAME` | `TYPE` and `PREF` convert to `contexts` and `pref`. | Input converts both. Output writes neither (`nickname_type_and_pref_parameters_parse_and_normalize_to_eds_profile`). | Deliberate divergence: RFC 2426 `NICKNAME` has no `TYPE` or `PREF`. |
| 2.5.1 `BDAY`, `ANNIVERSARY` | They give `anniversaries` of kind `birth` and `wedding`. | The writer emits `BDAY`, and both `X-EVOLUTION-ANNIVERSARY` and `ANNIVERSARY` for a wedding. The reader accepts all three and Apple date aliases. Only full days cross. | Partial: day values only. |
| 2.5.1 `BIRTHPLACE`, `DEATHDATE`, `DEATHPLACE` | They give anniversary objects of kind `death` and place data. | The reader ignores them. The writer drops other kinds. | Deliberate divergence: EDS has no such fields. |
| 2.5.7 `PHOTO` | `PHOTO` gives a media entry with `kind="photo"`, and `uri` from the line value. `PREF` and `MEDIATYPE` convert by section 2.3. | The reader maps `PHOTO` to one `Media` entry with `kind="photo"` and keeps URI or inline payload form (`read_photo`, `reads_the_inline_photo_line_eds_writes_back_as_a_data_uri`). It reads `MEDIATYPE` on URI input, then normalized emit writes EDS profile `PHOTO` lines with inline `TYPE` or `VALUE=uri` only (`photo_mediatype_parameter_on_value_uri_is_parse_only_in_this_profile`, `maps_a_photo_onto_the_line_evolution_shows_it_on`). | Partial: kind and URI mapping work, but export does not retain RFC 9555 `MEDIATYPE` or `PREF` metadata in this EDS profile. |
| 2.6.1 `ADR` | `ADR` gives `addresses[*].components`. `LABEL` gives `Address.full`. | The writer emits one seven-slot `ADR` line and one matching `LABEL` line. The reader reads `LABEL` on the `ADR` line or as its own line. | Partial: seven slots. |
| 2.6.1 `ADR` | Parameters and extended parts: `PREF`, `TYPE`, RFC 9554 components, `JSCOMPS`, `CC`, `GEO`, `TZ`. | The writer maps `work` and `private` to `TYPE=WORK` and `TYPE=HOME`, and writes preference as `TYPE=PREF`. The mapping ignores the rest. | Deliberate divergence: vCard 3.0 has no such slots. |
| 2.7.1 `EMAIL` | `EMAIL` gives `emails[*].address`. | One keyed line for each address. Empty addresses are skipped in both directions. | Match |
| 2.7.1 `EMAIL` | `TYPE` and `PREF` convert to `contexts` and `pref`. | Contexts convert as `TYPE`. Preference is a flag: input gives `pref: 1`, and output writes `TYPE=PREF`. | Partial: the numeric `PREF` rank collapses to a flag. |
| 2.7.2 `IMPP` | `IMPP` gives `onlineServices[*]`, with `vCardName` set to `impp` and `uri` from the line value. `SERVICE-TYPE`, `USERNAME`, `PREF` and `TYPE` convert by section 2.3. | The reader accepts `IMPP` lines for mapped schemes and sets `onlineServices[*].service` and `onlineServices[*].user` (`username_parameter_metadata_is_not_retained_in_eds_profile`, `service_type_parameter_metadata_is_not_retained_in_eds_profile`). The writer emits EDS `X-` service slot lines and emits no `IMPP`, `USERNAME` or `SERVICE-TYPE` metadata (`maps_an_online_service_onto_the_x_line_eds_keeps_it_on`). | Partial: core handle mapping works, but export uses EDS `X-` service slots and drops RFC 9555 metadata. |
| 2.7.6 `TEL` | `TEL` gives `phones[*].number`. | One keyed line for each number. The text stays as written, including `tel:` URIs. Empty numbers are skipped. | Match |
| 2.7.6 `TEL` | `TYPE` and `PREF` convert, with TEL feature values. | Input reads contexts, `CELL` or `MOBILE`, `PAGER`, `FAX`, `VOICE`, `VIDEO` and `PREF`. Output writes at most one context and one feature. `main-number`, `text` and `textphone` have no slot. | Partial: EDS phone slots only. |
| 2.9.2 `LOGO` | `LOGO` gives `media[*]` with `kind` set to `logo` and `uri` set from the line value. `PREF` and `TYPE` convert by section 2.3. | Input accepts `LOGO` lines parse safely while mapped fields still load, and normalized emit writes no `LOGO` lines (`logo_and_key_vcard_lines_and_server_preservation_characterization`, `key_and_logo_edge_cases_and_malformed_payloads`). The profile reads and writes `PHOTO` only (`vcard_to_card`, `card_to_vcard`, `states_media`). | Deliberate divergence: EDS has one photo slot and no logo field. |
| 2.9.4 `ORG` | `ORG` gives `organizations[*]`: name first, units after it. | The writer emits name and non-empty units. `ORG:;Unit` stands for a nameless organization. Empty values are dropped. | Match |
| 2.9.4 `ORG` | `ALTID`, `LANGUAGE`, `SORT-AS`, `TYPE` convert to organization fields. | The mapping keeps alternates as separate entries. It keeps no parameter data. | Deliberate divergence: EDS has no such fields. |
| 2.9.5 `RELATED` | `RELATED` gives `relatedTo`, keyed by the value, with `TYPE` as the relation. | The reader also accepts Evolution and Apple related-name lines. `spouse`, `partner`, `manager` and `assistant` map to relations. Other types become lowercase labels, and no type gives `contact`. | Partial: input only. |
| 2.9.5 `RELATED` | URI values and other parameters keep their meaning. | The reader rejects URI keys (`names_a_person`). The writer emits only `X-EVOLUTION-SPOUSE`, `-MANAGER` and `-ASSISTANT`. | Deliberate divergence: EDS has three relation slots. |
| 2.9.6 `TITLE`, `ROLE` | They give `titles[*]` with `kind` `title` or `role`. | The reader maps both. The default kind `title` stays unset in the card. The writer picks the property from the kind. | Match |
| 2.9.6 `TITLE`, `ROLE` | Grouped `ORG` gives `organizationId`. `ALTID` and `LANGUAGE` convert. | The mapping does not write `organizationId` or parameter data. Alternates stay separate entries. | Deliberate divergence: vCard 3.0 has no such link. |
| 2.11.1 `CATEGORIES` | `CATEGORIES` gives `keywords`. | The reader merges all lines into a set without duplicates. The writer emits one line from the sorted keys (`maps_keywords_onto_one_categories_line`). | Match |
| 2.11.1 `CATEGORIES` | `PREF` MAY order the values. | The reader ignores `PREF` and the writer does not emit it (`categories_pref_parameter_is_read_and_not_emitted`). | Deliberate divergence: `keywords` is an unordered set. |
| 2.11.2 `CLIENTPIDMAP` | `CLIENTPIDMAP` converts to `vCardProps` metadata. | Input accepts `CLIENTPIDMAP` parse-safely while mapped fields still load. The profile keeps no `vCardProps` metadata object, and normalized emit drops `CLIENTPIDMAP` lines (`clientpidmap_lines_stay_parse_safe_and_unmapped_in_eds_profile`). | Deliberate divergence: EDS has no generic extension-property channel. |
| 2.11.3 `CREATED` | `CREATED` gives `created`. | Input accepts `CREATED` parse-safely while mapped fields still load. The profile keeps no `created` field and normalized emit drops `CREATED` (`standard_vcard_properties_dropped_by_design_characterization_and_rationale`, `standard_properties_individual_variations_and_parameters`). | Deliberate divergence: contact creation time is not an EDS contact field. |
| 2.11.4 `NOTE` | `NOTE` gives `notes[*].note`. | One keyed line for each note. Empty notes are skipped in both directions. | Match |
| 2.11.4 `NOTE` | `ALTID`, `LANGUAGE`, `AUTHOR`, `AUTHOR-NAME`, `CREATED` convert to note metadata. | The reader keeps the text and drops the parameters. The writer emits the key and the text only. | Partial: text only. |
| 2.11.5 `PRODID` | `PRODID` gives `prodId`. | Input accepts `PRODID` parse-safely while mapped fields still load. The profile keeps no `prodId` field and normalized emit drops `PRODID` (`standard_vcard_properties_dropped_by_design_characterization_and_rationale`). | Deliberate divergence: exporter identity is not an EDS contact field. |
| 2.11.6 `REV` | `REV` gives `updated`. | Input accepts `REV` parse-safely while mapped fields still load. The profile keeps no `updated` field and normalized emit drops `REV` (`standard_vcard_properties_dropped_by_design_characterization_and_rationale`, `standard_properties_individual_variations_and_parameters`). | Deliberate divergence: contact revision time is not an EDS contact field. |
| 2.11.7 `SOUND` | `SOUND` gives one `media[*]` object with `kind` set to `sound` and `uri` set from the line value. `TYPE` and `PREF` convert by section 2.3. | Input accepts `SOUND` parse-safely while mapped fields still load. The profile reads and writes contact media through `PHOTO` only, so normalized emit drops `SOUND` (`sound_property_variations_audio_encodings_uris_and_fixpoint`, `standard_vcard_properties_dropped_by_design_characterization_and_rationale`). | Deliberate divergence: EDS stores contact picture media only. |
| 2.11.8 `UID` | `UID` gives `uid`. | The mapping writes `UID` from `id` (or `uid` if there is no `id`). It writes the JSContact `uid` as `X-JMAP-UID` (`uid_is_the_jmap_id_and_the_jscontact_uid_is_kept_aside`). | Deliberate divergence: EDS finds contacts by `UID`, so `UID` holds the JMAP `id`. |
| 2.11.9 `URL` | `URL` gives `links[*].uri`. | The reader keeps punctuation in the URI and invents a key if none exists. The writer emits one keyed line for each link. | Match |
| 2.11.9 `URL` | `TYPE` and `PREF` convert. | The reader maps neither. The writer emits none. Links of kind `contact` have no `URL` line and are dropped. | Deliberate divergence: the EDS profile treats `URL` as a bare URI. |
| 2.11.10 `VERSION` | `VERSION` gives `vCardProps`. | The reader accepts versions 2.1, 3.0 and 4.0 and does not keep the value. The writer always emits `VERSION:3.0` (`emits_a_vcard_30_envelope`). | Deliberate divergence: EDS stores vCard 3.0. |
| 2.11.11 `X-ABLabel` | Grouped `X-ABLabel` gives `label` on the JSContact object from the same property group. | Input reads grouped `X-ABLabel` values and maps known labels to EDS contexts and features. Other labels parse-safe into `extra["label"]` on mapped objects. Normalized emit writes mapped vCard 3.0 lines and emits no `X-ABLabel` (`x_ablabel_group_lines_stay_parse_safe_and_unmapped_in_eds_profile`). | Partial: grouped labels help import, but the profile does not retain `X-ABLabel` metadata on export. |
| 2.12.1 `KEY` | `KEY` gives one `cryptoKeys[*]` object with `uri` set from the line value. `PREF` and `TYPE` convert by section 2.3. | Input accepts `KEY` lines parse-safely while mapped fields still load (`logo_and_key_vcard_lines_and_server_preservation_characterization`). The reader keeps `crypto_keys` empty, and normalized emit writes no `KEY` lines (`vcard_to_card`, `crypto_keys_and_logo_server_state_untouched_characterization`). | Deliberate divergence: this EDS profile does not map JSContact `cryptoKeys`. |
| 2.13.1 `CALADRURI` | `CALADRURI` gives `schedulingAddresses[*].uri`, and `PREF` converts by section 2.3. | The reader maps each `CALADRURI` line to a scheduling address and reads `PREF=1` or `TYPE=PREF` as `pref: 1` (`maps_scheduling_addresses_onto_caladruri_lines`, `scheduling_addresses_read_pref_parameters`). The writer emits `CALADRURI` with a stable key and the URI only (`scheduling_addresses_emit_the_eds_profile_without_pref_parameters`). | Partial: URI and import preference map, but export drops `PREF` in this EDS profile. |
| 2.13.2 `CALURI` | `CALURI` gives `calendars[*]` with `kind` set to `calendar` and `uri` set from the line value. `PREF` and `TYPE` convert by section 2.3. | The reader maps `CALURI` into calendar entries, reads `PREF=1` and `TYPE=PREF` as `pref: 1`, and reads `MEDIATYPE` into `mediaType` (`maps_calendars_onto_the_caluri_and_fburl_lines`, `calendars_read_pref_and_media_type_parameters`). The writer emits `CALURI` with a stable key and URI only (`calendars_emit_the_eds_profile_without_pref_or_media_type_parameters`). | Partial: URI and inbound parameter mapping work, but export drops parameter metadata in this EDS profile. |
| 2.13.3 `FBURL` | `FBURL` gives `calendars[*]` with `kind` set to `freeBusy` and `uri` set from the line value. `PREF` and `TYPE` convert by section 2.3. | The reader maps `FBURL` into calendar entries, reads `PREF=1` and `TYPE=PREF` as `pref: 1`, and reads `MEDIATYPE` into `mediaType` (`maps_calendars_onto_the_caluri_and_fburl_lines`, `calendars_read_pref_and_media_type_parameters`). The writer emits `FBURL` with a stable key and URI only (`calendars_emit_the_eds_profile_without_pref_or_media_type_parameters`). | Partial: URI and inbound parameter mapping work, but export drops parameter metadata in this EDS profile. |
| 2.14 Extended properties and parameters | Extension properties and parameters follow section 2.15 conversion rules. | Input accepts extension parameters and extension properties as parse-safe metadata. Normalized emit drops both surfaces in this EDS profile (`vcardparams_extension_parameters_drop_after_parse_and_emit_in_eds_profile`, `vcardprops_extension_properties_drop_after_parse_and_emit_in_eds_profile`). | Deliberate divergence: EDS has no generic extension-property channel. |
| 2.15.1 `vCardProps` | `vCardProps` preserves vCard properties on the JSContact object as `JCardProp[]` metadata. | Input accepts extension properties parse-safely while it still maps supported fields. The profile keeps no `vCardProps` metadata object and normalized emit drops non-projected properties (`vcardprops_extension_properties_drop_after_parse_and_emit_in_eds_profile`). | Deliberate divergence: EDS has no generic extension-property channel. |
| 2.15.2 `vCardParams` | `vCardParams` preserves vCard parameters on the JSContact object that the vCard property converts to. | Input maps supported parameters into profile fields (`TYPE` into contexts or phone features, `PREF` into `pref`) and parses unknown parameters safely. The profile keeps no `vCardParams` metadata object, and normalized emit drops unknown parameters (`vcardparams_extension_parameters_drop_after_parse_and_emit_in_eds_profile`). | Partial: mapped parameters survive, generic parameter metadata does not. |
| 2.15.3 `vCardName` | `vCardName` preserves which vCard element created a JSContact object when multiple vCard properties map to one type. | The profile emits online services from mapped EDS slot properties only, and normalized output keeps no source-property identity metadata (`vcardname_source_property_identity_is_not_retained_in_eds_profile`, `grouped_property_identity_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has fixed slots and no source-property identity field. |
| 3.2.1 `JSPROP` | `JSPROP` carries JSContact properties with no direct vCard mapping. | Input parses `JSPROP` safely and keeps only fields the profile maps to EDS slots. Normalized emit writes no `JSPROP` line (`jsptr_parameter_metadata_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has no generic extension-property channel. |
| 3.3.1 `JSCOMPS` | `JSCOMPS` preserves ordered components and separators for `N` and `ADR`. | The reader maps classic `N` and `ADR` slots and ignores `JSCOMPS` metadata. The writer emits classic vCard 3.0 `N` and `ADR` lines and emits no `JSCOMPS` parameter (`jscomps_parameter_metadata_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has no ordered-component metadata field. |
| 3.3.2 `JSPTR` | `JSPTR` on `JSPROP` points to the JSContact property that owns the value. | Input accepts `JSPROP` parse-safely and keeps mapped properties only. Normalized emit drops `JSPROP` and `JSPTR` metadata in this profile (`jsptr_parameter_metadata_is_not_retained_in_eds_profile`). | Deliberate divergence: EDS has no generic extension-property channel. |
