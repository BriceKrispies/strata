# Enterprise tenant corpus

A scale corpus for WG-5292: one large enterprise CMS and automation tenant, written as 14 product files, one per
module. Each file is tenant metadata on the Strata platform (ARC-113, D-6546), imports the platform specification
`specifications/spec.jsonc` and declares only its own namespaced entities. Together the files declare 1,074 entities,
above the 500 the corpus is for. `crates/corpus` (WG-5398) installs all 14 into one tenant on any store and fills it with
seeded records; every file passes `specc bundle`, the platform's own check for installing a product.

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
section counts once. The sections are the ones a tenant bundle has a place for, plus `claims` (statements about the
product, not tenant metadata: specc leaves them out of the bundle, so they are not installed). The files declared 1,290
entities before they were made installable (see "Made installable" below); the four sections that have no place in a
tenant (`records` 76, `types` 120, `contracts` 126, `events` 42) were removed, and 87 views and 61 operations were
added for the interface to bind to, so the files declare 1,074 now, still above the 500 the corpus is for.

|module|roles|capabilities|objects|policies|queries|views|workflows|segments|settings|ledgerUnits|ledgerPools|ledgerLimits|screens|forms|actions|liveBindings|errorMessages|claims|mailTemplates|noticeTemplates|triggers|automations|operations|eventKinds|total|
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
|analytics|6|6|5|11|3|8|1|0|2|0|0|0|6|4|3|12|2|2|1|0|1|1|5|0|79|
|approvals|6|4|5|6|2|7|1|0|2|0|0|0|6|4|3|12|2|2|1|1|1|1|6|0|72|
|assets|3|3|5|5|3|7|1|0|2|0|0|0|6|4|3|12|4|2|1|1|1|1|5|1|70|
|automation|3|3|5|5|3|7|1|1|2|1|1|1|6|4|3|12|2|2|1|2|4|5|6|1|81|
|commerce|4|2|9|8|2|7|1|0|1|2|1|2|6|4|4|12|2|2|1|1|1|1|7|1|81|
|content|5|7|5|6|3|7|1|1|2|0|0|0|6|4|4|12|4|2|1|1|1|1|7|1|81|
|crm|4|5|5|5|2|7|1|0|2|0|0|0|6|4|5|12|2|2|1|1|1|1|7|1|74|
|forms|3|3|5|5|2|7|1|0|2|0|0|0|6|4|3|12|2|2|1|1|2|1|7|1|70|
|localisation|6|4|5|6|2|8|1|0|2|0|0|0|7|4|4|14|2|2|1|1|1|1|5|0|76|
|marketing|3|3|5|5|2|7|1|3|2|0|0|0|6|4|4|12|2|2|3|1|3|3|6|1|78|
|people|6|5|6|6|3|8|1|0|2|0|0|0|7|5|5|14|3|2|2|1|1|1|8|1|87|
|publishing|3|4|4|4|2|7|1|1|0|0|0|0|6|4|3|12|2|2|1|1|3|3|6|1|70|
|service|4|4|5|6|3|8|1|0|2|0|0|0|7|4|4|14|3|2|2|1|1|1|6|1|79|
|taxonomy|4|3|4|5|8|7|1|3|2|0|0|0|6|4|3|12|2|2|1|1|1|1|6|0|76|
|**total**|60|56|73|83|40|102|14|9|25|3|2|3|87|57|51|174|34|28|18|14|22|22|87|10|**1074**|

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
- Sections: only those a tenant bundle has a place for (roles, capabilities, objects, policies, queries, views,
  workflows, segments, settings, ledger units, pools and limits, mail and notice templates, triggers, automations,
  operations, eventKinds, screens, forms, actions, live bindings, error messages) and claims, which the bundle leaves out.
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
- Ids: every id carries its file's namespace, and no id appears twice in a file.
- References resolve: every `screen`, `viewModel`, `contract` (now an operation), `form`, `automation`, template, `unit`,
  `policy`, `operation`, and `object` names an entity of the same file. `specc bundle` checks this for every file.
- Installable: each file passes `specc bundle --spec specifications/spec.jsonc --product <file> --out <dir>` with no
  diagnostic (WG-5398), and `node` mirrors of the kernel's interface and view rules (routes, names, inputs, route
  parameters) find nothing.

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

### Made installable (WG-5398)

The platform installs a product as a tenant bundle (`specc bundle`), which refused every file: 44 `bundle.section-no-place`,
154 `bundle.ui-unresolved`, 24 `bundle.ui-unsupported`. No check was relaxed; the files were changed:

- `bundle.section-no-place` (44): the sections `records`, `types`, `contracts` and `events` were removed from all 14 files
  (76 records, 120 types, 126 contracts, 42 events). A tenant has no place for them: each `records` entry duplicated an
  object of `objects` (the object is the tenant's record type), a `types` entry is a value type the tenant has no
  section for (the objects spell their field types out), a `contracts` entry is realised as an operation, and the
  platform's events are the platform's (a trigger names one). Nothing a tenant could hold was dropped.
- `bundle.ui-unresolved` (screens, 87): every screen answers a route that some view of the product must answer. The 15
  views the files had answered none of the 87 routes, so 87 views were added, one per screen (`<screen id>_view`, the
  screen's route and title, one section on the product's query that best matches the screen's name, or its first query),
  and each screen's `viewModel` now names its view instead of a contract.
- `bundle.ui-unresolved` (forms and actions, 108 references): a form or action submits to an operation of the product,
  and 61 of the contracts they named were not operations. 61 operations were added under the contract's id, with the
  inputs the contract declared and the inputs the forms and actions fill (types from the contract's types and the
  form controls), open to any signed-in person, with no conditions and no steps. They keep the interface bindable; they
  do nothing when invoked.
- `bundle.ui-unsupported` (24): live bindings whose `when` named a domain notice kind (`event.kind == 'approval'`)
  now name `upsert`, one of the four kinds of the live protocol (upsert, remove, schema, membership).
- `layer.value-invalid` (18): the `field` of an error message is a `ui-error-field` (`{ "name": "email" }`), not a string.
- Route parameters are lower-case names in the kernel (`:category_id`, not `:categoryId`): five routes in localisation and
  taxonomy were renamed, with the `route.<param>` arguments that read them.

What the files still cannot show is whether the kernel accepts the whole installed schema in ways those mirrors miss. `strata-corpus check` installs each product on a fresh tenant and prints every problem the kernel reports.

### Made to pass the kernel's schema rules (WG-5398)

`specc bundle` does not run the kernel's whole-schema check (`TenantSchema::problems`, run by every install). The first
install failed it (`automation analytics__send_digest.mail: address: unknown field recipient_email`), and a read of
`crates/kernel/src/definitions.rs`, `schema.rs`, `segments.rs`, `ui.rs`, `templates.rs`, `settings.rs` and `ledger.rs`
found these classes in the files. Fixed in place, nothing removed:

- Automation expressions read no record fields: their environment is the parameters an event binds (`seq`, `at`, `hop`,
  `transition`, `person`, `segment`, `owner`, `subject`, `body`, `event_id`, `due_at`, `member` and a few more) and the outputs of
  earlier steps. `param.record`, `param.stage`, `param.request`, `param.case`, `param.release`, `param.channel`, `param.connector`,
  `param.employee`, `param.requester` and `param.translator` are bound by no event. Run keys are now `param.seq` (one run per event);
  the record a step works on is `record.$id`; a recipient is `param.owner`, `param.person` (segment entry) or `param.member` (fan-out);
  an operation input and a webhook body take `param.subject`, `param.body` or `param.event_id`. These are placeholders in the
  kernel's event model, which gives a record trigger no record id: they pass the check, and what they read at run time is for the
  worker to say. Also: the analytics digest goes to the literal address `reports@example.com` (the new schedule's address is not
  readable), `automation__scheduled_run` waits until `param.due_at` (a field of the schedule is not readable), the approvals
  escalation branch tests `record.due_at < now` (the stage has no `sla_breached` field), and the marketing win-back fan-out batch is
  100 (the most the kernel allows).
- Eight policies named a role `member`, which a tenant schema does not hold (it holds `public`, `owner` and what products add): each
  of assets, automation, content, forms, marketing, publishing and taxonomy gained a `<module>__member` role and its policies name it.
- A tenant operation's subject input must be a reference to the subject: the 22 operations with a subject now declare it as
  `{ "Ref": { "target": <subject object>, "on-delete": "Restrict" } }`, not `Str`.
- A named query limit is 1 to 200: ten queries asked for 300 to 5000 and now ask for 200.
- A segment names a required unique string field that holds its members' principal (`principal-field`): all nine segments now do
  (connector `name`, article `slug`, audience member `email`, channel `name`, audience `label`, made unique). A principal id is
  letters, digits and `_:.-`, so an email address is no principal and the three marketing segments hold no member until the field
  is a principal-shaped value.

Checked the same way and found clean: trigger and automation step shapes, reachable steps, mail and notice template bodies,
settings, ledger declarations, capabilities, retention and personal-data subjects, workflow guards and sets, policy
conditions, query filters and orders, view arguments, the interface elements. `node` mirrors of these rules (an expression
shape checker among them) find nothing in the 14 files.

## Not verified

specc passes every file. What only the kernel decides at install is unchecked until `strata-corpus load` has run:

- Operation input references (`param.<name>`) and the `record.$id` scope in fan-out CreateRecord steps.
- FanOut steps (`next` key) and the `timeout` on RequestApproval, which has no timed-out branch.
- Object `Ref` with `"on-delete": "Restrict"` only.
- The 61 added operations have no steps.
