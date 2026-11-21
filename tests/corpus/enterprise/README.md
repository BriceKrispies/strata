# Enterprise tenant corpus

A scale corpus for WG-5292: one large enterprise CMS and automation tenant, written as 14 product files, one per
module. Each file is tenant metadata on the Strata platform (ARC-113, D-6546), imports the platform specification
`specifications/spec.jsonc` and declares only its own namespaced entities. Together the files declare 1,290 entities,
above the 500 the corpus is for.

## Modules

- analytics (`analytics.jsonc`): Dashboards, tiles, metrics, KPI thresholds and scheduled digests for the tenant's reporting.
- approvals (`approvals.jsonc`): Multi-stage legal, brand and regional approval chains with delegations, SLAs and escalations.
- assets (`assets.jsonc`): The media library of the enterprise tenant: assets, renditions, licences, usage rights and collections.
- automation (`automation.jsonc`): Connectors, webhook subscriptions, schedules and an API call budget that run tenant automations.
- commerce (`commerce.jsonc`): Manages the product catalogue, regional prices, promotions, orders and returns for the tenant.
- content (`content.jsonc`): The article, revision, page and block model of the enterprise content tenant, with publishing and embargoes.
- crm (`crm.jsonc`): Contacts, accounts and leads with consent and campaign history.
- forms (`forms.jsonc`): Form builder for collecting submissions, consent and attachments.
- localisation (`localisation.jsonc`): Locales, translation jobs, glossaries and translators that bring the tenant's content into other languages.
- marketing (`marketing.jsonc`): Runs campaigns, customer journeys, email templates and audience segments for the tenant.
- people (`people.jsonc`): Employees, departments, cost centres, agency assignments and leave requests for the enterprise tenant.
- publishing (`publishing.jsonc`): Schedules, channels, syndication targets and publish jobs for releases of content.
- service (`service.jsonc`): Customer cases, queues, macros, knowledge articles and SLA clocks for the support desk.
- taxonomy (`taxonomy.jsonc`): Categories, tags, regions and audiences that other modules classify content and people by.

## Entity counts

Counted by a node one-liner over the parsed files (JSONC with whole-line comments stripped). Every entry of every
section counts once. Records and objects are separate kinds, so a module's data model appears in both.

|module|records|types|roles|capabilities|objects|policies|queries|views|workflows|segments|settings|ledgerUnits|ledgerPools|ledgerLimits|screens|forms|actions|liveBindings|contracts|events|errorMessages|claims|mailTemplates|noticeTemplates|triggers|automations|operations|eventKinds|total|
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
|analytics|5|8|6|6|5|11|3|2|1|0|2|0|0|0|6|4|3|12|9|3|2|2|1|0|1|1|1|0|94|
|approvals|5|8|6|4|5|6|2|1|1|0|2|0|0|0|6|4|3|12|8|3|2|2|1|1|1|1|2|0|86|
|assets|5|9|3|3|5|5|3|1|1|0|2|0|0|0|6|4|3|12|8|3|4|2|1|1|1|1|1|1|85|
|automation|5|8|3|3|5|5|3|1|1|1|2|1|1|1|6|4|3|12|8|3|2|2|1|2|4|5|2|1|95|
|commerce|8|8|4|2|9|8|2|1|1|0|1|2|1|2|6|4|4|12|8|3|2|2|1|1|1|1|3|1|98|
|content|6|9|5|7|5|6|3|1|1|1|2|0|0|0|6|4|4|12|8|3|4|2|1|1|1|1|2|1|96|
|crm|5|9|4|5|5|5|2|1|1|0|2|0|0|0|6|4|5|12|11|3|2|2|1|1|1|1|2|1|91|
|forms|6|8|3|3|5|5|2|1|1|0|2|0|0|0|6|4|3|12|11|3|2|2|1|1|2|1|2|1|87|
|localisation|5|8|6|4|5|6|2|1|1|0|2|0|0|0|7|4|4|14|8|3|2|2|1|1|1|1|1|0|89|
|marketing|5|9|3|3|5|5|2|1|1|3|2|0|0|0|6|4|4|12|9|3|2|2|3|1|3|3|2|1|94|
|people|6|9|6|5|6|6|3|1|1|0|2|0|0|0|7|5|5|14|13|3|3|2|2|1|1|1|2|1|105|
|publishing|5|8|3|4|4|4|2|1|1|1|0|0|0|0|6|4|3|12|6|3|2|2|1|1|3|3|2|1|82|
|service|5|10|4|4|5|6|3|1|1|0|2|0|0|0|7|4|4|14|11|3|3|2|2|1|1|1|2|1|97|
|taxonomy|5|9|4|3|4|5|8|1|1|3|2|0|0|0|6|4|3|12|8|3|2|2|1|1|1|1|2|0|91|
|**total**|76|120|60|56|73|83|40|15|14|9|25|3|2|3|87|57|51|174|126|42|34|28|18|14|22|22|26|10|**1290**|

Kinds the brief named, against the totals: 120 types, 76 records, 60 roles, 87 screens, 57 forms, 174 live bindings,
51 actions, 42 events. Behaviours are not produced (see below).

## Could not express

Each item is a reference the format refuses, with the rule in `crates/spec-compiler/src/layers.rs` that blocks it.
Nothing below is written into a file.

1. content article to assets asset (cover and lede images): `layer.cross-product-reference`. A product may name only
   its own entities and platform entities; a name that another product file declares is refused.
2. assets usage right to content article or variant: `layer.cross-product-reference`, as above.
3. approvals approval request to content article: `layer.cross-product-reference`.
4. publishing publish job and release to content article: `layer.cross-product-reference`.
5. localisation translation job to content article: `layer.cross-product-reference`.
6. taxonomy category and tag to content article and commerce product (classification): `layer.cross-product-reference`.
7. forms submission, service case, marketing audience member and people employee to the crm contact (the same person
   in several modules): `layer.cross-product-reference`. Each module keeps its own person.
8. commerce order to the crm contact who placed it, and marketing email send to commerce order: `layer.cross-product-reference`.
9. Behaviours (`behaviors`) and scenarios, ui scenarios, evidence and tokens: not produced. A product may declare
   behaviours only as workflow models (level model), and the corpus uses typed `workflows` for the same states.
   `layer.section-invalid` and `layer.kind-not-allowed` cover the rest.

## Rules followed

From `crates/spec-compiler/src/layers.rs`, as the checker reads them:

- Header: `product` with id, name, description, version, platform min 0.2, and `imports.spec` relative to the file
  (`../../../specifications/spec.jsonc`), with `imports.entities` listing every platform id the file names.
- Namespace: every entity id is the product id with hyphens as underscores, then `__`, then a local name
  (`content__article`). Ids are unique across the file, across sections.
- Sections: only the product sections and typed sections (records, types, roles, capabilities, objects, policies,
  queries, views, workflows, segments, settings, ledger units, pools and limits, mail and notice templates, triggers,
  automations, operations, eventKinds, screens, forms, actions, live bindings, contracts, events, error messages, claims).
- Platform power: no platform-level roles, no anonymous, service or external holders, no contract with realisation
  other than facade, no implementedBy, no public anonymous contract, no core capability, no domain, elapsed or
  delivery event, no implementation-level behaviour.
- Retention (D-6584): every object with a personal field declares retention.
- Ledger (WG-4998): counts and stock are ledger units changed by PostLedger, never by a step writing a field from its own value.
- Configuration (WG-5000): no literal record id in an expression; configuration is a declared setting.
- Event kinds (SCH-1576): every eventKinds kind begins with `app.`; at most 16 fields.
- Platform entities used by name (types in `type` keys, capabilities in `requires`, `owner: product-module`) are
  imported; no platform entity is redefined.
- Refused names: a name of another product module is never written.

## Verification

- Every file parses as JSONC (node, whole-line `//` comments stripped, trailing commas removed).
- Totals above are counted by node over the parsed files.
- Ids: every id carries its file's namespace, and no id appears twice in a file. The first pass had record and object
  ids in common (for example `content__article` as both); the records were renamed with a `_rec` suffix (`refersTo`
  updated to match), and the four cross-section clashes (three screens in localisation, publishing and service, and one
  contract in approvals) were renamed with `_screen` and `_contract` suffixes.
- References resolve: every `refersTo`, `screen`, `contract`, `viewModel`, `form`, `automation`, template, `unit`,
  `policy`, `operation`, and `object` names an entity of the same file. The only exceptions are two expression field
  names (`segment` in a fields map), which are not references.

## specc findings

The first specc run refused every file with 342 authoring diagnostics, and nothing was written. They were fixed in
place, with no entity removed, so the total stays at 1,290:

- `layer.import-missing` (300): every file now imports `ui-notice-received`, `ui-session-ended` and `api-error`, which
  the live bindings and the contract error types use.
- `layer.value-invalid` (22): setting defaults are wrapped in the `value` union (`{"Int": n}`, `{"Str": "..."}`,
  `{"Bool": b}`). Five operations gained the `conditions` list the operation definition requires. Three webhook
  subscriptions are uuids. The translator policy compared a Ref with a Str, so the assignment object gained a
  `translator_principal` string field.
- `layer.outside-catalogue` (12): setting defaults that were bare strings are wrapped in `value` variants. The analytics
  view argument is `{"Const": {"Str": "emea"}}`, an arg-source variant.
- `layer.expr-unknown-param` (6) and `layer.expr-type` (1): trigger conditions now read `param.hop < 4`, which every
  trigger's scope binds. The policy comparison is above.

specc is not run again here. The coordinator re-runs it.

## Not verified

The shapes beyond specc's first run are unchecked. Known points to check first when WG-5292 runs it:

- Operation input references (`param.<name>`) and the `record.$id` scope in fan-out CreateRecord steps.
- FanOut steps (`next` key) and the `timeout` on RequestApproval, which has no timed-out branch.
- Form field `input` names against contract input names.
- Object `Ref` with `"on-delete": "Restrict"` only; records use `restrict` or `set-null`.
- Several contract `requires` lists name the module's own capabilities, which the brief allows.
