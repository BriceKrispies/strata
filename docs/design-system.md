# The design system and agentic composition

How a Strata tenant's screens are made, and made by agents: the design tokens and themes, the
primitives, the patterns and how products extend them, the app shell and templates, the contract
that turns data into interface, interaction, wireframes, who sees what (public and private, role by
role), the layouts a tenant arranges, how all of it is versioned, and the loop in which an agent
builds a tenant's screens while the tenant watches. This is a design. Its requirements are in
`specifications/design-system.jsonc` (DS-001 to DS-018, D-DS-1 to D-DS-27), the properties that can
be stated precisely are proved in Lean (`lean/Strata/UI/`), and the work is listed at the end in
dependency order, each item a Workgraph task under WG-5457. It was revised after three adversarial
review rounds; the answers are in the last section, and everything above it is the current design.

It builds on the browser client design (`docs/frontend.md`, WG-5415) and changes none of its
decisions: the kernel stays pure, custom elements draw and the kernel decides (D-FE-4), no third-party
code reaches the browser (D-FE-9), the browser's copies keep their rules (D-FE-1, D-FE-2, D-FE-11).
It amends one copy class (`copy-browser-interface`, see "Serving and caching") and one behaviour of
`acceptManifest` (see "Version skew").

## The recommendation in one page

Nine decisions are the owner's (2026-11-22) and are recorded as decided: a neutral base with tenant
theming through tokens, Fieldnotes becoming one theme (D-DS-1); layouts the tenant arranges, as data
(D-DS-2); agentic first (D-DS-3); wireframing as a first-class concept (D-DS-4); public and private
for every element, with the rule that the interface never discloses what the data layer would refuse
(D-DS-5); role-based views (D-DS-6); per-role shells and screen variants, first match wins (D-DS-20);
audience groups (D-DS-21); and the rendering-major lifecycle, warn then migrate (D-DS-22). Every other
D-DS-* is a proposal of this design.

- **Who a screen is for is declared, never inferred** (D-DS-23, the architecture decision of the second
  review). Audiences are written in roles or audience groups by the owner or the agent; what the
  platform derives from policies only narrows what is served and produces warnings. An admin element,
  or one offering an administrative action, declared for members or the public is refused by
  construction (`admin_needs_role`).
- **One catalogue, three tiers of extension** (D-DS-7): composite patterns in a static, bounded tree
  language a product writes as data; platform blocks; platform-reviewed renderer extensions recorded as
  data; catalogue gaps with a lifecycle. No tenant or product JavaScript at any tier.
- **Tokens and themes** (D-DS-15): semantic tokens in logical properties; a theme is token values,
  derived once from typed seeds and stored, with defaults for tokens added later; contrast checked at
  definition.
- **Primitives are mostly native elements** (D-DS-8), in the light DOM, by a deliberate one-way choice.
- **Screens are compositions** (D-DS-10) of templates, regions, nestable layout containers and widgets
  bound by kind (D-DS-17); every interface element has a draft and a published body; dormant entities
  live in a **draft overlay** of the schema that no non-builder evaluation is given
  (`nonbuilder_reach_active`); publishing is one counted change, `PublishDraft`, refused while any
  placeholder remains (`publishable_no_placeholder`).
- **Interaction is typed** (D-DS-17): gestures with pure pending-write projections and fractional ranks,
  wizards with a stated state home.
- **Display types end raw values** (D-DS-9).
- **Visibility is served, not hidden** (D-DS-11): per interface class, containers pruned when empty or
  when they need content they lack (`served_has_element`), nothing left as a trace
  (`hidden_leaves_no_trace`), no build activity, audiences or group names visible to non-builders,
  withheld values marked by the server for required fields only.
- **Roles vary one composition** (D-DS-12) through declared audiences; shells by first match, screen
  variants by the first that serves the viewer something (`selectServed_first`); row affordances are
  advisory; interface classes are formed from groups only (D-DS-24).
- **Tenant layouts are an overlay** (D-DS-13), by id, orphaned per widget check, reported, reversible,
  with the product's base screen as the fallback, so no upgrade is held for an overlay.
- **The rendering contract is versioned** with objective majors, a warn-then-migrate lifecycle with a
  grace period for reversal, and skew never shows revoked content (D-DS-19, D-DS-22).
- **International and accessible by rule** (D-DS-18).
- **The agent loop** (D-DS-14): classified product steps into the draft, fenced by a lease kept in
  tenant kernel state with an expiry (`no_stale_step_after_acquire`, `no_step_after_expiry`); held
  upgrades bound to the diff the owner saw (`approve_applies_held`); a bounded, typed field copy for
  restructures; diagnostics that point into the product file.
- **Product versions are ordered apart from platform releases; large installs are staged** (D-DS-16).
- **Reserved for the enterprise work** (D-DS-25 to D-DS-27): a task inbox, decision form and `decide`
  action for WG-5496, a process timeline for WG-5507, an acting-as indicator for WG-5499; class keys that
  stay shareable under principal attributes and time-bound grants; display types neutral to WG-5494's
  exact numbers and civil dates.

## Where the framing should change

The request is the right direction. Taken literally, some of it would make the system worse.

1. **"Primitives as Web Components: button, field and inputs, form-associated with ElementInternals"
   is wrong for the inputs that have native elements.** A native `<input>`, `<select>` or
   `<textarea>` is already form-associated, labelled, validated, autofilled and understood by every
   assistive technology. The catalogue keeps every primitive the request names; most are a class
   contract on a native element, and a custom element is used only where behaviour has no native form.
2. **"A display-type catalogue that maps field types to renderings" cannot by itself end raw ids.**
   A principal is a `str`, money an `int`, a status a `str` driven by a workflow
   (`crates/model-core/src/types.rs`, `FieldType`). The display type is declared, and people and
   reference titles are resolved by the server.
3. **"Tenants can arrange layouts" and "products own their screens" conflict unless the arrangement is
   a layer of its own.** D-8250 gives a product ownership of its interface elements. The tenant's
   arrangement is an overlay the tenant owns.
4. **"Possibly before the data model exists" needs a validity rule that allows it**: a placeholder is
   an explicit binding that names nothing, not a dangling reference.
5. **"The tenant watches it being built live" should mean the builders, not the members.** Work in
   progress is a draft, served only to builders, and invisible to everyone else, including the fact
   that it is happening.
6. **The 200-change cap is not a problem for the agent loop**, whose steps carry only what changed;
   it bites only on the first install of a large product, answered by staging.
7. **"Hide what a role may not use" must not be a client rule.** Visibility is decided by the server
   from the same policy and invoker evaluation that decides reads and invocations, and the browser is
   never sent what it must not draw. This changes one rule of D-8250: the manifest is no longer the
   same for every visitor (`crates/backend/src/ui.rs`).
8. **A theme with arbitrary values conflicts with CMS-005 as worded** unless the token schema is typed.
9. **"Virtually any feature" cannot mean "every feature in the platform's own catalogue".** A closed
   list of patterns and binding kinds serves CRUD, community and CMS shapes and nothing else. The
   catalogue is closed against *code* (no tenant JavaScript, ever) but open to *composition* (products
   define composite patterns as data) and to *governed growth* (platform blocks and reviewed renderer
   extensions, with gaps recorded where agents hit them).

## The current state

| What | Where | What it does |
|---|---|---|
| Tokens | `web/src/styles/tokens.css` | Fieldnotes' editorial palette and type: Fraunces, Newsreader, Instrument Sans; papers `linen`, `stone`, `chalk`; accents `moss`, `clay`, `ink`, `ochre`, `plum`; a dark scheme; reduced motion |
| Theme choice | `crates/substrate/src/theme.rs` (`ACCENTS`, `PAPERS`, `resolve`); `web/src/main.js:61-62`; `web/src/kernel/update.js:93,434` | A tenant picks an accent and a paper by name from a settings record; anything else falls back to the default (CMS-005) |
| Component styles | `web/src/styles/components.css` (922 lines), `layout.css`, `base.css`, `lab.css`, layered by `main.css` | Classes for Fieldnotes' pages (masthead, page titles, sections, forms, records); no component catalogue |
| Renderer | `web/src/components/dom.js`, `parts.js` | Virtual nodes patched with DOM APIs; dates as `<time>`, prose blocks, marginal notes |
| Tenant interface definition | D-8250; `crates/kernel/src/ui.rs`; `crates/substrate/src/ui.rs`; `crates/backend/src/ui.rs` | Screens, forms, actions, live bindings, error messages as `ui-element`s in the tenant schema; served as a manifest and parts named by hash; the same for every visitor |
| Element types | `crates/model-core/src/types.rs:9452-9840` (`UiScreen`, `UiScreenKind`, `UiAuth`, `UiForm`, `UiControl`, `UiAction`, ...) | Four layouts (`list`, `detail`, `editor`, `settings`), eight controls, `auth` of `anyone` or `session` |
| Product compiler | `crates/spec-compiler/src/bundle_ui.rs` | Product `screens`, `forms`, `actions`, `liveBindings`, `errorMessages` into the bundle's `ui`; refuses per-row actions, initial values, role-restricted screens (`bundle.ui-unsupported`) |
| Interpreter | `web/src/kernel/tenant-ui.js` (`tenantPage`, `sectionsOf`, `recordOf`, `shown`) | Every section in name order as a field list; a field shown with `String(value)`, an object as JSON |
| Drawing | `web/src/views/tenant.js` | A section is a `<dl>` of field name and value; forms after or before |
| View replies | `crates/substrate/src/project.rs:335-420` | Rows with `id`, `object`, `version`, `created_at`, `updated_at`, `created_by`, `fields`; `refs` for `include` fields (the referenced record, read under policy); `blocks` for `markdown` fields |
| Policies, audiences | `crates/kernel/src/rules.rs` (`roles_of`: anonymous holds `public`, a user holds `public` and its roles), `crates/kernel/src/audience.rs`, `crates/backend/src/aggregates.rs` | Read policies per role with conditions; field read rules (`FieldDef.read`, D-6586); audience classes for shareable copies |
| Operation invokers | `crates/kernel/src/operations.rs:8-12,171` | Stage 1 of an invocation: refused unless a signed-in person the invokers name (`signed-in`, `roles`) |
| Install, upgrade | CT-236 `install-bundle`, CT-835 `upgrade-bundle` (`boundary.jsonc`) | One product change each, with `dry-run`, `idempotency-key`, and for an upgrade `expected-version`; owner and step-up |
| Agents | `tenant-agent` delegation, `change-propose`, `change-approve` (CT-112, CT-113), G-23, G-24, G-25 | Agents act for a person through the agent gateway; risky changes need an approved proposal |
| The cap | `crates/kernel/src/limits.rs:21` `MAX_SCHEMA_CHANGES = 200`, checked at `transition.rs:2154` | One schema change carries at most 200 changes |
| Browser tests | `tests/e2e/generated` (uiScenarios with `axeScan`), `tests/e2e/site.spec.js`, `scripts/shots.mjs` | axe scans and full-page screenshots exist; no screenshot comparison against baselines |

The showcase (WG-5454, `tests/corpus/showcase/README.md` in its worktree) drew every screen as
label and value text. Its UI gaps are 5 (fixed sections, aggregates as JSON, no header slot, no
inbox drawing), 6 (no per-row actions, no prefilled forms, record numbers typed by hand), 9 (no
screen can change a setting) and the screen half of 2 (no role-gated screen: members opened the
admin panel and saw empty sections).

## Layer 1: tokens and themes

### The tokens

Components read semantic tokens only, named by role, never by colour. The catalogue's `tokens`
section lists each with its type:

| Group | Tokens | Type |
|---|---|---|
| Surfaces | `surface`, `surface-sunk`, `surface-raised`, `surface-overlay` | colour |
| Text | `text`, `text-muted`, `text-on-accent`, `text-on-danger`, `link` | colour |
| Lines | `border`, `border-strong`, `control-border`, `focus` | colour |
| Accent | `accent`, `accent-strong`, `accent-wash` | colour |
| Tones | `tone-{neutral,info,success,warning,danger}-{fg,bg}` | colour |
| Type | `font-display`, `font-text`, `font-ui`, `font-code`; `text-xs` to `text-3xl`; `leading-tight`, `leading-text`; `tracking-display` | family (closed list), length, number |
| Space | `space-1` to `space-9`; `density` (`compact` 0.75, `regular` 1, `comfortable` 1.25) | length, enumeration |
| Shape | `radius-1` to `radius-3`, `radius-pill`; `border-hair`, `border-rule` | length |
| Elevation | `shadow-1` to `shadow-3` | shadow (from a scale) |
| Motion | `ease`, `dur-quick`, `dur-settle` (zero under reduced motion) | easing, duration |
| Size | `size-control`, `width-page`, `width-measure`, `width-form` | length |

Each colour token has a light and a dark value; a theme declares which schemes it offers.

**The contrast pairs** are declared in the catalogue, not left to each theme: `text`, `text-muted`
and `link` on each surface (4.5 to 1); `text-on-accent` on `accent` and `text-on-danger` on
`tone-danger-fg` (4.5); each tone's `fg` on its `bg` (4.5); `border-strong`, `control-border` (the edge of an input, checkbox or switch), `focus` and `accent` on
each surface (3 to 1, WCAG 1.4.11, non-text contrast). Every pair is checked in both schemes.

### Themes

- **A preset** gives every token a value. Fieldnotes is a preset: its current values become the
  `fieldnotes` theme with its five accents and three papers as variants, unchanged on screen.
- **The neutral base** is a preset too: the system font stack (`system-ui` for text and interface,
  no web font, so no bytes), a cool grey neutral ramp, a blue accent, `radius-2` corners, regular
  density. It is what a tenant gets before choosing anything, and what an agent's wireframes are
  drawn in.
- **A tenant theme** gives seeds: `accent` (an sRGB value), `neutral` (`warm`, `cool`, `neutral`, or
  an sRGB value it is tinted from), `schemes` (`light`, `dark`, both), `fonts` (one of the vendored
  pairings: `system`, `editorial` (Fieldnotes' three families), `humanist`), `radius` (`none`,
  `small`, `medium`, `large`), `density`, `scale` (a ratio from a list). The full token set is derived
  from the seeds by a pure function, and a tenant may override single tokens, each override typed and
  checked like the rest.

**Derivation.** Ramps of tints are computed from the seeds in OKLCH; an on-colour (text on accent,
text on a tone) is the first tint along the ramp that meets 4.5 to 1, and the ramp ends at black or
white. A search of that ramp always finds one (`search_finds`), because black on a surface of
luminance at least 0.1791, and white on a darker one, always meets 4.5 to 1 (`onColour_meets_aa`;
the worst case is 4.58).

**Validation.** Defining a theme runs the contrast check on the full derived set, both schemes, and
is refused naming each failing pair, its ratio and the minimum (`checkTheme_iff` says the check is
exactly the pairs). The derived set is **stored** with the theme, with its derivation version and
hash, and reads use the stored set, so a later change to the derivation recolours no tenant (see
"Governance"). An agent's styling step that picks an accent too light for links is refused with
the diagnostic, and the derived suggestion in the hint.

**Delivery.** The resolved token values come to the page with the bootstrap and the view model (as
the theme does today, `crates/bff/src/routes.rs:1507-1516`). The main thread sets each with
`document.documentElement.style.setProperty`, after checking the value against the token's type a
second time; no value is concatenated into a stylesheet, so no tenant value can open a rule or a
`url()`. CSSOM property writes are not inline markup and need no CSP change. Fonts are the vendored
families of the catalogue's `fonts` section (WG-5439), grown by platform review (see "International and
accessible by rule").

**Where a theme lives.** A theme is tenant configuration, changed through the settings intents as the
accent and paper are today; the theme of a tenant is not a product entity, so installing a product
never changes how the tenant looks. A product may ship a theme preset for the tenant to choose.

## Layer 2: primitives

No primitive knows anything about Strata data. Each is keyboard-complete and passes axe at WCAG 2.2
AA in every theme, in every state the catalogue lists, in forced-colours mode, in a right-to-left
document, and at 400% zoom.

| Primitive | Implementation | Keyboard and semantics |
|---|---|---|
| button | `<button class="ds-button">`, variants `primary`, `secondary`, `quiet`, `danger`; sizes | native |
| link | `<a class="ds-link">` | native |
| field | `<div class="ds-field">` with `<label>`, the control, hint and error joined by `aria-describedby`; `aria-invalid` | native |
| text, number, date, time inputs, textarea | native elements in a field | native; form-associated already |
| select | `<select>` in a field | native |
| checkbox, radio group | native inputs; a radio group is a `<fieldset>` with `<legend>` | native |
| switch | `<input type="checkbox" role="switch">` | native, announced as a switch |
| record picker | `<ds-record-picker>`, form-associated (ElementInternals), the ARIA combobox pattern | arrows, Enter, Escape, typeahead |
| dialog | `<ds-dialog>` around a native `<dialog>`, opened with `showModal()` when its `open` property is set | focus moves in, is trapped by the browser, returns to the opener; Escape closes |
| sheet | `<ds-sheet>`: a native `<dialog>` anchored to the bottom or the inline end, for touch shells | as dialog; a visible close button, never only a swipe |
| menu | `<ds-menu>`: the menu button pattern | roving tabindex, arrows, Home, End, typeahead, Escape returns focus |
| tabs | `<ds-tabs>`: the tabs pattern, manual activation | arrows move, Enter or Space selects |
| tooltip | `<ds-tooltip>`, on hover and focus, `aria-describedby` | Escape dismisses; never the only carrier of information |
| toast | `<ds-toast-region>`: a `status` live region, `alert` for errors | errors do not dismiss themselves; others pause on hover and focus |
| gesture hosts | `<ds-sortable>`, `<ds-lanes>`, `<ds-selection>`: the interaction gestures (see "Interaction") | every pointer gesture has a keyboard path and a single-pointer, non-drag path |
| avatar | `<img alt>` or initials; decorative (`alt=""`) beside a visible name | none |
| badge, chip | `<span class="ds-badge">`; a removable chip is a button | native |
| progress | native `<progress>` or `<meter>` | native |
| skeleton | `<div class="ds-skeleton" aria-hidden="true">` inside a container with `aria-busy="true"` | none |
| layout classes | `ds-stack`, `ds-cluster`, `ds-grid`, `ds-sidebar`, `ds-switcher`, `ds-center`, `ds-split`: classes with custom-property parameters, intrinsic and container-query based, logical properties only | none |

**Rules.**

- **Light DOM, deliberately and one way.** Labels and ARIA references cannot cross a shadow root, the
  uiScenarios query by role and name, and the theme's tokens and the page's layers must reach every
  element. The price is accepted: no element can encapsulate its styles, so every primitive's styles
  live in the platform's layered stylesheet and a renderer extension (below) cannot bring its own.
  Moving to Shadow DOM later would break every label association and every axe baseline; it is not
  a door left open. The only shadow root remains the QR leaf of D-FE-4.
- **What an element may hold.** A custom element holds no application state (D-FE-4). It may hold
  interaction state no other part of the page depends on and that a redraw may lose (which menu item
  has focus, a drag in progress). Anything that changes what data is shown, or should survive a reload
  or be in the address (a selected tab, a sort, a filter, a wizard step), is kernel state.
- **Properties, not markup.** Elements take properties through `dom.js`'s property channel and build
  their children with DOM APIs; charts are SVG built with `createElementNS`. Nothing is parsed (D18).
- **Forced colours.** Under `forced-colors: active` primitives use system colours, keep every
  boundary as a border (never a background alone), and never carry state by colour alone (a status
  badge has its label; a selected row has a mark).
- **Target size.** Every target is at least 24 by 24 CSS pixels at every density, compact included
  (WCAG 2.5.8); in a touch shell (bottom-tab, below) at least 44 by 44. The density token scales
  padding, never below the floor.
- **Reflow.** Every primitive and pattern reflows at 320 CSS pixels wide (400% zoom of a 1280 window)
  with no scrolling in two directions, except inside a data table, which stacks its rows as cards
  below its `stackBelow` width.

## Layer 3: patterns

A pattern is a composition of primitives bound to a kind of data. It is what an agent places.

| Pattern | Binds to | Shows | Main props |
|---|---|---|---|
| `record-detail` | a single section | the fields of one record, by display type | `fields`, `layout` |
| `record-card` | a single section, or a row of a list | title, subtitle, meta, body, actions | `title`, `subtitle`, `meta`, `body`, `media`, `actions` |
| `record-list` | a list section | a card per row | as `record-card`, `density` |
| `data-table` | a list section | rows and columns, row actions, sorting among the query's declared orders, paging by cursor, select-many | `columns`, `rowActions`, `sort`, `pageSize`, `stackBelow`, `selection` |
| `form` | a form | fields, prefill, client checks from the operation's input types, server diagnostics by field | `prefill`, `layout`, `submit` |
| `dynamic-form` | a record-defined form | fields read from field-definition records, answers submitted as one operation | `definition`, `submit` |
| `wizard` | forms or regions as steps | one step at a time, step state in the kernel, a summary step | `steps`, `submit` |
| `feed` | a list section | actor (person), verb, object (reference), time | `actor`, `verb`, `object`, `time`, `body` |
| `stream` | a list section ordered by time | messages anchored to the end, new ones appended live, a pinned composer | `author`, `body`, `time`, `composer` |
| `comment-thread` | a list section and a form | comments with author and time, and the reply form | `author`, `body`, `time`, `form` |
| `reaction-bar` | an aggregate grouped by a field, and an action | counts per kind, the viewer's own reaction marked | `kinds`, `action` |
| `board` | a grouped section | lanes by the group field, cards, move between lanes | `group`, `card`, `move` |
| `chart` | a series | line, bar, area, sparkline, as SVG with a data table alternative | `kind`, `x`, `y`, `series`, `unit` |
| `time-grid` | a positioned section (start and end fields) | a day, week or month grid of records | `start`, `end`, `title`, `view` |
| `plane` | a positioned section (x and y fields) | records placed on a plane or a tenant image (a floor plan) | `x`, `y`, `image`, `marker` |
| `notification-bell` | the platform inbox | the unread count and a menu of recent notices | `limit` |
| `notification-list` | the platform inbox, or a list section | notices with read state and a mark-read action | `markRead` |
| `profile-header` | a single section | avatar, name, role line, facts, actions | `name`, `avatar`, `facts`, `actions` |
| `stat-tile` | an aggregate or a count | one number with its label and unit | `label`, `unit`, `display` |
| `aggregate-bars` | an aggregate grouped by a field | a ranked bar list | `label`, `limit` |
| `filter-bar` | the screen's filter state | controls that set query parameters shared by the screen's sections | `filters` |
| `task-inbox` (reserved) | the platform source `tasks` (WG-5496's pending-for-me contract) | the viewer's open human tasks, with claim, release and reassign, opening a `decision-form` | `columns`, `group` |
| `decision-form` (reserved) | one task of the `tasks` source | the task's subject (a reference), its declared outcomes as buttons and an optional comment, submitted by a `decide` action | `subject`, `comment` |
| `process-timeline` (reserved) | a list section over WG-5507's run and step objects | a run's steps in order with state, actor (person), start and end times | `run`, `step`, `state`, `actor`, `start`, `end` |
| `markdown-block` | nothing (static text in props), or a `markdown` field | prose blocks | `text` |
| `link-list` | nothing: screens of the tenant | links to screens by id | `links` |
| `nav-menu` | navigation entries | a navigation group | `group` |

**States.** Every pattern that binds to data has `loading` (a skeleton of the pattern's shape with a
fixed number of rows, so it says nothing about how many records there are), `empty`, `error` and
`ready`; `placeholder` is drawn only in drafts. **Confirm flows**: an action's `confirm` prop opens a
`ds-dialog`; a destructive tone puts focus on cancel. **Server diagnostics**: a refused form draws the
refusal through the tenant's error messages, each failed check on its field, focus on the first.

**What is not offered, stated.** A geographic map needs map tiles from an origin of the platform's
own; the platform has none, and D-FE-9 and `img-src 'self'` exclude third-party tiles, so `plane`
places records on a tenant image and a geographic map is a catalogue gap (an open question below).
Presence ("who is online") needs a server feature that does not exist; `stream` works without it.

## Layer 4: the app shell, regions, layout containers and templates

**Shell variants** are catalogue entries:

| Variant | Regions | For |
|---|---|---|
| `top-nav` | `header` (slots `brand`, `nav`, `search`, `actions`, `account`), `main`, `aside`, `footer` | the default |
| `rail` | a vertical icon rail for primary navigation, `header`, `main`, `aside` | tools with many screens |
| `bottom-tab` | `header` (compact), `main`, a bottom tab bar of at most five entries, sheets for secondary navigation; `env(safe-area-inset-*)` respected, 44-pixel targets | phone-first products |
| `full-bleed` | `main` only, with a floating account control | dashboards, kiosks, media |
| `none` | `main` only | embedded and print views |

**Per-role shells (D-DS-20, owner decision).** The tenant lists shells in priority order, each with an
audience (`[{ "audience": { "groups": ["staff"] }, "shell": "rail" }, { "audience": "public", "shell":
"top-nav" }]`); the first whose audience holds the viewer is used (`select_sees`, `select_first`). The
list must end with an entry whose audience holds everyone the app is for, or the kernel refuses it
(`ui.shell.uncovered`). A product never sets the shell.

**The acting-as indicator (reserved in the shell contract).** Every shell variant has a slot
`header.actingAs`, filled by the platform, never by a product or an overlay, whenever the session's
effective principal differs from the person who signed in (delegated acting-as, WG-5499): a persistent
banner naming both people ("You are acting for Ada Lovelace"), announced once in a polite live region,
with a "Stop acting" control, drawn above everything else in the header and in `bottom-tab` and
`full-bleed` shells too (as a top bar). No tenant setting or overlay can hide it. Until WG-5499 exists
the slot is empty.

**Screen variants (D-DS-20).** A route may have an ordered list of screen variants, each with an
audience, its own template, its own composition and its own content lint. The variant served is the
**first in declared order whose audience holds the viewer and that serves the viewer something**
(`selectServed_sound`, `selectServed_first`): a viewer in an earlier variant's audience who may read
none of that variant's content falls through to the next (`variant_falls_through`, the third review's
case), rather than getting a page that is not there. Whether a variant serves a viewer something is
decided per class, from roles and grants, never from rows, so falling through discloses nothing a read
would not. Precedence is the declared order, never a union: a manager who is also staff is served the
first variant that serves them, and never sees the staff variant at that route; the preview says so.
The owner is in every group and so is served the first variant; the preview shows the others on request.
A route whose variants serve a viewer nothing is the not-found page for that viewer. A navigation entry
names a route, and shows the label of the variant the viewer is served (each variant states its label).

Each variant states its responsive behaviour. Products contribute **navigation entries** (`navigation`
section: screen, label, group, order, icon); the tenant's overlay orders and hides them; the
`notification-bell` sits in `header.actions` (or the tab bar), bound to the platform inbox.

**Templates** arrange a screen's main area: `list`, `detail`, `dashboard`, `settings`, `feed`,
`editor`, `board`, `calendar`, `chat` and `canvas`. The catalogue says which patterns each region
allows and how many; the kernel enforces it.

**Layout containers** make a region a tree: a `layout` widget has `kind` (`stack`, `cluster`, `grid`,
`split`, `switcher`, `sidebar`), typed parameters and children, has an id and an audience, may be marked
`needsContent` (it is then served only when an element that counts is served inside it; regions are
marked so by default), and is pruned when nothing in it is served. The **depth limit of four counts the
expanded tree**: a composite placed in a container counts its own body's depth.

**Composite templates** are **saved layouts with region constraints**: a product declares a named
`canvas` arrangement whose regions each state the patterns and composites they allow and their counts,
from the catalogue's vocabulary; screens that use it are checked against those constraints like a
platform template. Alternative templates per audience inside one composition are not offered; screen
variants (above) are the owner's chosen mechanism for that.

Today's four layouts are templates of the same names, and a screen that declares only `kind` is read as
that template with one widget per view section in name order.

## Layer 5: the data-to-UI contract

### Display types

A display type maps a value to a rendering. The catalogue's `displayTypes` section lists, for each,
the field types it applies to, its props, and what the server must provide.

| Display type | Field types | Rendering | Server provides |
|---|---|---|---|
| `text` | str | the text; `multiline` keeps line breaks | |
| `rich-text` | str with `markdown` in the view section | prose blocks | `blocks` (exists) |
| `datetime` | time | `<time datetime>`, `relative` ("3 hours ago", absolute in the tooltip) or `absolute` ("22 Nov 2026, 14:05"), in the request language and the realm's time zone; the kernel relabels relative times on its heartbeat timer | |
| `date` | time (and, when WG-5494 adds it, a civil date type) | the day only; a civil date is formatted as itself, never converted through a UTC instant | |
| `duration` | int (seconds) | "2 h 5 min" | |
| `count` | int | grouped digits, or compact ("1.2k") | |
| `number` | int with a declared scale (and, when WG-5494 adds it, a decimal type) | grouped digits; the formatter takes the value as an exact decimal string and never as a floating-point number | |
| `money` | int of minor units with a currency (and, when WG-5494 adds it, a money type) | `Intl.NumberFormat` currency style, fed an exact decimal string; currency, scale and rounding mode come from the field's type when it has them, otherwise from props (`currency` as a literal or a sibling field, `scale`, `rounding`, default half-even) | |
| `percent` | int | with a declared scale | |
| `boolean` | bool | "Yes"/"No", a check mark with text, or a switch's state | |
| `status` | str (workflow state or choice) | a badge; each value has a label and a tone (`neutral`, `info`, `success`, `warning`, `danger`) | |
| `choice` | str with options | the option's label | |
| `person` | str holding a principal; `created_by` | a person chip: avatar or initials and the display name, linked to the person's profile screen when one is declared | the `people` table of the reply |
| `reference` | ref | a link to the screen that shows the target's detail, its text the target's title field | `refs` (exists, with `include`) and the target's presentation |
| `references` | list of ref | chips of references | as `reference` |
| `tags` | list of str | chips | |
| `language` | language | the language's name (`Intl.DisplayNames`) | |
| `media` | file | an image rendition with alt text from a declared field, or a file link with size and type | the rendition address (D-6589) |
| `url`, `email` | str | a link; external links get `rel="noopener noreferrer"` | |
| `code` | str, int | monospaced, copyable: the only display type that draws an identifier as it is, and only when declared | |

**Fallbacks.** A field with no declared display type is drawn by its type: `time` as `datetime`
relative, `ref` as `reference` (or, when no title is available, the target object's label as the
link text, never the id), `bool` as `boolean`, `list` as chips, `file` as `media`, `int` as `number`,
`str` as `text`, `language` as `language`. `created_by` is always `person`. No fallback draws a
record id, an epoch number or a JSON object. An aggregate is drawn by the pattern bound to it
(`stat-tile`, `aggregate-bars`, `reaction-bar`), never as JSON.

**People.** The gateway resolves every principal a reply carries in a `person` field or `created_by`
to a display name and an avatar, in a `people` table beside the rows, keyed by an opaque per-reply
handle; the principal string itself is no longer sent (today it is). A name is personal data: it is
resolved only for a principal in a field the reader may already read, and only as far as the tenant's
member-name setting allows (default: signed-in members see each other's display names; an anonymous
reader sees a name only for a person whose profile is public); otherwise the chip says "A member".
Without this, showcase gap 8's "messages show the author's principal" cannot be closed by any
client.

**Reference titles.** A `reference` needs the target record, which the view's `include` already reads
under the reader's policies (`project.rs`, `refs`). A target the reader may not read is not included,
and is drawn as plain text "Not available", not as a link and not with its id.

### Presentations

A **presentation** is a new interface element (`ui-presentation`), one per object type, declared in a
product's `presentations` section and owned by the product:

```jsonc
{ "id": "showcase_community__message_presentation", "object": "showcase_community__message",
  "title": "title", "subtitle": "channel", "media": null,
  "fields": {
    "author":     { "display": "person" },
    "created_at": { "display": "datetime", "style": "relative" },
    "channel":    { "display": "choice" },
    "status":     { "display": "status", "tones": { "published": "success", "hidden": "neutral" } },
    "body":       { "display": "rich-text" }
  } }
```

The kernel checks that the object type and every field exist and that each display type applies to
the field's type. A widget's props may override a field's display for that widget. A presentation
keeps the record schema (`ObjectTypeDef`) unchanged: presentation is interface, and interface
elements change through `define-ui-element` like the rest of D-8250.

### Screens as compositions

A screen gains a template, regions and widgets. In a product file:

```jsonc
{ "id": "showcase_community__message_page", "route": "/messages/:id",
  "viewModel": "showcase_community__message_view", "template": "detail",
  "audience": "members", "title": "${data.message.fields.title}",
  "regions": {
    "header": [ { "id": "message", "pattern": "record-card", "bind": { "section": "message" },
                  "props": { "body": "body", "meta": ["author", "created_at", "channel"],
                             "actions": [ { "action": "showcase_community__hide", "whenRefused": "hide",
                                          "audience": { "groups": ["moderators"] } } ] } } ],
    "main":   [ { "id": "comments", "pattern": "comment-thread", "bind": { "section": "comments" },
                  "props": { "author": "author", "body": "body", "time": "created_at",
                             "form": "showcase_community__form_comment" } } ],
    "aside":  [ { "id": "reactions", "pattern": "reaction-bar", "bind": { "section": "reactions" },
                  "props": { "action": "showcase_community__react" } } ]
  } }
```

- A **binding** is one of the kinds of "Structured bindings" (`section`, `grouped`, `series`,
  `positioned`, `form`, `record-form`, `action`, `source`, `static`), or `null` (a placeholder). Every
  widget, layout container and region has an `id` unique in its screen.
- A **placeholder** carries `"sample"`: the shape it will show (`{ "single": false, "rows": 5,
  "fields": [ { "label": "Title", "display": "text" }, { "label": "Posted", "display": "datetime" } ] }`),
  drawn with skeleton shapes and its labels, and the widget's `"note"` (one line saying what it will
  show). A sample holds no data: display types and labels from a closed vocabulary (see "Wireframes
  and fidelity").
- An **action** may take arguments `row.<field>` and `row.id` when it is a row action of a widget
  bound to a list section; the kernel checks the field exists on the section's object and that the
  operation input's type accepts it (a `ref` input whose target is the section's object for
  `row.id`). This closes showcase gap 6, with the gateway coercing a reference input from the
  record id the interpreter sends.
- A **form** may declare `prefill`: `{ "section": <single section>, "fields": { <field>: <record field> } }`
  or, as a row action's form, from `row.<field>`. The person's typed text wins over a prefill that
  arrives later (D-FE-13's drafts keep typed text separate).
- The kernel's check (`crates/kernel/src/ui.rs`, extended): the template and every region exist in the
  catalogue; each pattern is allowed in its region, with the count the template allows; props
  validate against the pattern's schema; every binding names a section of the screen's view, a form or
  action on the screen, or a platform source, of a kind the pattern's `binds` accepts; field names in
  props exist on the bound shape; ids are unique; layout containers nest at most four deep;
  every element's declared audience is no wider than its parent's (see "Visibility"); and the screen
  is refused when an admin element is declared for members or the public; the content lint warns (see
  "Who a screen is for").

**The interpreter** stays in the kernel and stays pure: `compose(screenPart, viewModel, context)`
builds a page model, a tree of pattern nodes with every value already formatted by its display type,
from the screen's part, the shapes of its sections, the view reply and its `people` and `refs` tables.
The main thread draws page-model nodes with one renderer per pattern
(`web/src/views/patterns/*.js`), from primitives. The screen part carries the **shape** of each
section, computed by `strata_substrate::ui` from the schema: the object type, and for each field its
label, type, display type and whether a read rule may withhold it per record. So a change to an
object's fields changes that screen's part and its hash, and the browser redraws.

**Products contain no JavaScript.** Everything above is catalogue entries and data checked by specc
and by the kernel; a product file that names a pattern, display type or template the catalogue does
not have is refused (`ui.catalogue.unknown`). INT-3257 holds unchanged.

## Extending the catalogue

"Virtually any feature" is met in four tiers. None runs tenant or product JavaScript.

1. **Platform patterns and templates**, each a renderer in the web application.
2. **Composite patterns, defined by a product as data** (below).
3. **Platform blocks**: composites the platform ships (a pricing table, a hero, a media gallery, an FAQ).
4. **Platform-reviewed renderer extensions**, for drawings no composite expresses.

### The composite body language

The choice between a static tree and an expression language is one-way, so it is decided now: **a
static tree with a closed set of node kinds and no expressions.**

| Node kind | Holds |
|---|---|
| `primitive` | a catalogue primitive with props whose values are literals of the prop's type, `text` values, or slot references |
| `layout` | a layout container with typed parameters and children |
| `slot` | inserts a slot's value drawn by its display type, or a nested composite slot |
| `heading` | text and a **relative** level (`+0`, `+1`, `+2`): the level is the placement's level plus that, so a composite never fixes heading levels |
| `when` | a condition and children: `present(slot)`, `empty(slot)`, or `equals(slot, literal)` against a choice or status slot; nothing else |
| `each` | children repeated once per item of a list slot, or per row of the bound section's page; at most one `each` on any path, so nesting does not multiply |
| `actions` | the placement's row actions as buttons or a `ds-menu` |

- **Slot references** are paths: `slot.<name>` or `slot.<name>.<field>`, at most three segments; there
  are no functions, operators or arithmetic.
- **Not Turing-complete, with stated bounds:** a composite may not reference itself directly or through
  other composites (the kernel checks the reference graph is acyclic); the expanded body has at most
  200 nodes; `each` repeats at most the bound section's page size (at most 200, `MAX_PAGE_SIZE`). Every
  expansion is a finite tree computed in time linear in its size.
- **States** derive from the binding: the composite shows the binding's `loading`, `empty`, `error` and
  `placeholder` states with the catalogue's skeleton for its body shape; per-slot emptiness is handled
  by `when empty(...)`.
- **Gesture hosts are not allowed in a body**; a gesture is attached where the composite is placed (on
  the pattern that hosts it). `ds-menu` is allowed, only through the `actions` node.
- **Landmarks** (`main`, `nav`, `aside`, `header`, `footer`, `region`) are not allowed in a body; they
  belong to the shell and the template.
- **Accessible-structure checks** at definition: every interactive primitive has an accessible name from
  a literal `text` or a required slot (a slot whose field is required, so it is never empty), or it is
  refused (`ui.a11y.unnamed-control`); every image has an alt slot or is marked decorative
  (`ui.a11y.image-alt`); headings do not skip levels within the body (`ui.a11y.heading-order`).
- **Composite-typed host props.** The catalogue marks which props of a host pattern take a composite:
  `board.card`, `record-list.item`, `data-table.columns[].cell`, `stream.message`, `feed.item`,
  `time-grid.event`, `plane.marker`. A composite in such a prop is drawn once per item.
- **Gestures and focus inside a composite.** The host injects the gesture **handle** as the first
  focusable element of each item; the item is one stop in the host's roving focus; Enter or F2 moves
  focus into the composite's own interactive primitives (Tab cycles inside the item, Escape returns to
  the item); the gesture keys belong to the handle only, so a composite's buttons never compete with
  them.
- **Reserved extensions** (catalogue minor versions, not in the first): `when` operators beyond
  `present`, `empty` and `equals` (`not`, `and`, comparisons `lt`, `le`, `gt`, `ge` against a literal),
  an `image` node and an icon vocabulary (`icon` node naming a catalogue icon), and a `plural` text node
  that chooses among plural categories (`zero`, `one`, `two`, `few`, `many`, `other`, by
  `Intl.PluralRules` in the request language) from a count slot. The node-kind list and the operator list
  are open enums in the schema from the first version, so adding them is a minor change.
- **Computed conditions belong in a derived field.** A condition over computed state ("overdue",
  "over budget") is a derived field of the record type, computed by the kernel, and a composite tests
  that field with `equals`; the body language never computes.
- **Lanes are the host's.** In a `board`, the host iterates lanes and the composite is the card; a
  composite's `each` cannot iterate lanes, so nested iteration over a grouped binding is lane-then-card
  by construction.
- **Blocks hold no interaction state of their own**; a pricing block's monthly/yearly toggle is a screen
  filter (kernel state in the address) the block's `when` tests.

### Registry entries and gaps

- A **renderer registry entry** is data: `{ "id", "version", "moduleHash", "capabilities": [...] }`. The
  table in the web application holds these entries and the built module they name; a module whose hash
  does not match its entry is not registered. **`capabilities` is documentation today** (`svg`,
  `canvas2d`, `pointer`, `keyboard`, `timers`): nothing enforces it in the browser; it becomes enforced
  when the table becomes a verified-module manifest, and is recorded now so that the manifest needs no
  new field. There is still no run-time registration.
- A **catalogue gap** (`POST /api/ui/gaps`) has a status: `open` → `triaged` → `accepted` or `declined`
  → `resolved` (with `resolvedBy`: the catalogue entry id and the catalogue version that added it) or
  `duplicate` (with `duplicateOf`). When a gap is resolved, the builders of each tenant that filed it are
  told, with the placeholders that the new entry could fill.
- A **declined** gap leaves its placeholders in the draft, where they block publication like any
  placeholder; the builders are told and must remove each placeholder or replace it with another
  pattern or composite before publishing.

## Structured bindings

A binding names what a widget shows. The kinds, each with its server contract:

| Kind | Names | Where the work happens | Bounds and parameters |
|---|---|---|---|
| `section` | a section of the screen's view | the kernel's read of the query | page size at most 200; paging by cursor |
| `grouped` | a list section and a field with a **closed** set of values (a choice, a workflow state, a bool), or a **reference field whose lanes the product declares as a section of their own** | the kernel reads the lanes and each lane's cards as the viewer | at most 20 lanes per page, lanes paged when they come from a section, each lane paged by its own cursor; a reference group field without a lanes section is refused (`ui.binding.open-group`); cards whose lane the viewer may not read go to a catch-all lane "Other", whose lane label discloses nothing |
| `series` | an aggregate of a named query, bucketed by a time field or grouped by a field | the kernel's aggregate engine, on the server; the browser receives buckets, never rows | at most 500 points per series and 8 series; time buckets in the realm's time zone with a declared bucket size (hour, day, week, month); week start from the request language |
| `positioned` | a list section and position fields (start and end times, or x and y numbers) | the kernel reads the window the widget asks for | a window is required (time-grid: start, end, time zone, week start; plane: a bounding box); at most 2,000 records per window; beyond it the server returns the first 2,000 in the query's order and the count of records in the window (records the reader may read), and the pattern draws "showing 2,000 of N, narrow the window"; a 50,000-record section never becomes 50,000 nodes |
| `form` | a form of the screen | the operation | |
| `record-form` | a single section of form definitions and a list section of their field records | the operation that takes the answers | at most 100 fields per definition |
| `action` | an action of the screen | the operation | |
| `source` | `inbox`, `navigation` | the platform | |
| `source` (reserved) | `tasks`: the viewer's human tasks (WG-5496) | the platform | the contract's own page bound |
| `static` | nothing | | |
| `placeholder` | nothing yet; a sample shape | | drafts only |

- **Plane images** are bound to a `file` field (the `media` display type) of a single section, or to a
  file of the tenant's file store named in props; never a URL.
- **Why reference lanes come from a section**: lanes by a reference (by assignee, by project) would
  disclose, through the lane list, records the viewer may not read (who has tasks). The lanes section is
  read under the viewer's policies like any section, so the viewer sees exactly the lanes they may read.
- **A chart's data table** is drawn from the same buckets as the SVG, in the same page model; the SVG is
  `aria-hidden` and the table is the accessible representation, shown on request and always present to
  assistive technology.

**Shared filter state.** A screen declares `filters` (named, typed parameters); a `filter-bar` sets
them; they are kernel state carried in the address, and each section that takes a query parameter of
the same name is read again when one changes.

**Record-defined forms (form builder).** The definition shape is fixed by the catalogue:

- a definition record: title, description, an ordered list of **sections** (title, description);
- field records: section, order, label, help, **input type** (from a closed list: short text, long text,
  number, date, choice, multiple choice, yes/no, email, file), required, choices, and **validation**
  from a closed list (minimum, maximum, length range, one of a fixed set of patterns: email, phone,
  postal code by country);
- **conditional visibility**: a field may be shown only when another field of the same definition
  equals a literal or is answered; no other logic;
- answers are **typed per input type from the first version**: each answer is a value variant of its
  input type (`text`, `number`, `date`, `choice`, `choices`, `bool`, `email`, `file`, `reference`), never
  a list of strings, because a string list would be the expensive one-way choice; they are submitted to
  one operation with the definition id, and the operation validates each against its field record;
- a **`reference` input type** takes a record of a section the definition names, chosen with the record
  picker under the respondent's read policies;
- **reserved as minor extensions**: `not equals` and `or` in conditional visibility, and repeating groups;
- a definition's sections may be drawn as **wizard steps**;
- **out of scope, stated plainly**: calculations, scoring, branching to other definitions, and any logic
  beyond the conditions above; a full form builder of that kind would need an expression language,
  which D-DS-7 rules out.

## Interaction

**Gestures** are catalogue entries, each bound to an operation through typed arguments, each with a
keyboard path, a single-pointer non-drag path, and a **pending-write projection**: a pure function the
catalogue states, from the view result and the gesture's arguments to the view result drawn while the
write is pending.

| Gesture | Arguments | Projection | Keyboard path |
|---|---|---|---|
| `reorder` | `row.id`, `target.rank` | the row moves to its new rank in the list; others keep their order | Space to pick up, arrows, Space to drop, Escape; or "Move up/down" |
| `move` | `row.id`, `target.group`, `target.rank` | the row leaves its lane and enters the target lane at its rank, **in one operation**, so there is no intermediate state | the same across lanes; or "Move to…" with groups and positions |
| `inline-edit` | `row.id`, `field.<name>` | the field shows the new value, marked pending | Enter to edit, Enter to save, Escape |
| `select-many` | `rows.ids` to a bulk action | each selected row marked pending for the action's effect | Space, Shift+arrows, Ctrl+A on the page |

Examples are part of each catalogue entry: a view result, arguments, and the projected view result,
checked by a kernel test. **Ranks** are strings in a fractional lexicographic scheme: a rank between two
neighbours is computed from theirs; ties sort by rank, then by record id; when no rank of at most 32
characters fits between two neighbours (a collision after many inserts at one place), the gesture's
operation is preceded by a **rebalance** of that lane's page (one operation that rewrites the ranks of the
page evenly, bounded by the page size), and the projection shows the rebalanced order. A product that
offers `reorder` or `move` declares the rank field. A `reschedule` gesture for `time-grid` (arguments
`row.id`, `target.start`, `target.end`; keyboard path: arrows by the grid's step, Enter to confirm) is
reserved as a minor extension.

**Inline edit inputs by field type:** `str` a text input (a textarea for `multiline`), `int` a number
input (with the display type's scale), `bool` a switch, `time` a date or date-time input per the display
type, a choice or workflow state a select (a workflow offers only its allowed transitions), `ref` the
record picker, `language` a select of languages; `list`, `file` and `rich-text` fields are not edited
inline (`ui.gesture.inline-unsupported`).

**Wizards.** State lives in the kernel and, when the tenant keeps drafts (D-FE-2), in the encrypted
drafts store of the principal's partition, under the rules of D-FE-13 for typed text; for an
**anonymous** visitor it lives in the kernel worker's memory only and is lost on reload. It lasts until
submission, "discard", or the idle session lifetime; it holds at most 64 KB per wizard; the address
names only the wizard and the step. A step that runs its own operation stores its outputs, and later
steps may prefill from `step.<id>.<output>`.

**Optimistic and live state.** Optimistic state lives only in the kernel's pending writes (D-FE-5),
drawn by the gesture's projection and marked pending until the receipt or the refusal; a refusal
restores the view result and shows why. Live state is the view result plus the notices that make it be
read again.

**Collaborative editing** (several people editing one record or one text at once, with merged changes
and presence) is a **named gap**: nothing here offers it, and it is not approximated.

## Wireframes and fidelity

A wireframe is the same composition with placeholders, drawn at once in the neutral base theme or the
tenant's: labelled regions, the pattern's skeleton, the sample's labels and display shapes, and each
widget's note.

**Samples hold no data.** A sample is a single-or-list flag, a row count from 1 to 12, and fields, each
a display type and a label. A label is a word of the catalogue's `sampleLabels`, a word of the tenant's
**sample vocabulary** (declared once by the tenant's builders: "Lease term", "Unit", "Tenant"), a label
of a schema field, or **free text of at most 32 characters**, which is drawn flagged as sample text and
refused when it looks like personal data (`ui.sample.personal`: an email address, a phone number, a run
of digits, or a display name of a member of the tenant). Placeholders are drawn only to builders.

**Valid at each step, mechanically.** A composition is **valid** when widget ids are distinct, every
widget is allowed where it is placed, a static pattern is bound `static`, and a data pattern is a
placeholder or is bound to a binding its pattern accepts and the schema defines (`Composition.Valid`);
**publishable** when valid with no placeholder at all (`Composition.Publishable`). Proved: `check_iff`,
`checked_bindings_resolve`, `publishable_no_placeholder`, `publishable_bound`, `bindAt_valid`,
`bindAt_placeholders`, `refine_valid`, `refine_publishable`, `incremental_valid` (schema and catalogue
changing), `unguarded_removal_breaks`.

**Shapes are part of a binding.** A binding is the name of what it binds together with a **shape
fingerprint**, the SHA-256 of the canonical encoding of: for a section, the object type id and, for each
field the widget reads, the field id, field type, display type, whether it is required, and the class of
its read rule (`none`, `role-only`, `per-record`); for a form or action, the operation id and each input's
name and type; for a structured binding, the section's fingerprint plus its group, bucket or position
fields and their types. A change to any of these is a binding that stops existing and another that
starts.

**What the models do not cover**: props beyond what `accepts` decides, nested layout containers in the
composition model, and the meaning of a catalogue change beyond the answers a screen relies on.

## Visibility: public and private

### Who a screen is for: the architecture decision

"Can an admin panel reach members?" failed in two review rounds through four routes: a conditional Read
grant, an action offered to every signed-in person whose conditions test a role, an own-scope marker an
author (or an agent) adds to pass, and query filters and field read rules the derivation did not read.
Each route is a place where the kernel decides reads or invocations that the derivation of "who is this
screen for" had not looked at. Two architectures were weighed:

1. **Make the derivation complete**: scan every policy condition, query filter, field read rule and
   operation condition with `audience.rs`'s `scan`, treat any role test or negation as conditional, and
   keep deriving the audience. Its weakness is the one the reviews found: completeness must be re-won
   every time the kernel gains a place that decides access (a new kind of rule, a lookup in a condition,
   a segment test), and a miss is silent and discloses.
2. **Declare the audience, and let the derivation lint.** Who a screen, region or element is for is
   always written in roles or audience groups, owner-intended; the derivation from policies narrows what
   is served and warns when a declared audience looks wrong, but never decides who a screen is for.

**Chosen: 2** (D-DS-23). The question becomes "is the declaration right", which an owner can read and an
agent must state; the derivation's incompleteness can then only produce a missed warning or an empty
widget, never a disclosure, because derived audiences only ever narrow. What must be impossible is
refused by construction, not linted:

- **`ui.audience.admin-for-members`**: an **admin** element declared, directly or through its
  containers, for `public`, for `members`, or for any audience that names the realm's **default role**
  (`default_role`, given to every person on first sign-in, `crates/model-core/src/types.rs:3643`) or a
  role the tenant marks as held by everyone. An element is admin when the author marks it, or when the
  kernel derives it:
  - it offers an action whose operation's invokers name roles (not every signed-in person);
  - or whose conditions contain a role test (`has_role`) under a `Not`, or a role test at all (only a
    `Not` around a role test counts as negation: `record.author != principal` in the showcase's
    `react_to_message`, `community.jsonc:362`, is not one, and a test pins that it stays member-facing);
  - or which changes roles, permissions, settings or policies;
  - or **by effect**: one of its steps writes an object type whose write grants (Create, Update, Delete
    policies and permissions) do not include the least viewer of the element's declared audience. This
    catches access decided by data the scan cannot see (`principal in through(record, "project",
    "owners")`), and workflow guards, which do not apply to operations (D-6547).
  The author cannot unmark a derived admin element. A role that every current member holds without
  being the default role is **warned** about when an admin audience names it, at definition and on each
  role assignment that makes it so. Proved: an admin element is served only to a viewer holding a role,
  other than `public` and the default roles, that a strict declared audience above or on it names
  (`admin_needs_role`); an anonymous visitor or a member holding only default roles is never served one
  (`admin_never_to_default_member`); an admin audience naming the default role is refused
  (`admin_default_role_refused`); the first review's admin action is refused
  (`admin_action_for_members_refused`). A Rust differential against the model's admin check is a
  criterion of WG-5465.
- **Group edits re-run the admin check.** Groups, the shell list and variant order are draftable and
  need approval (see the kinds table); when a group's roles change, the kernel re-runs the admin check on
  every screen, variant and shell entry that names the group, refuses the change if one fails, and the
  approval shows who gains and who loses access, by role.
- **Own scope is derived, never asserted**: a widget is own-scoped when every conditional Read grant
  that reaches its data has a condition that mentions `principal` (`scan` reports it); there is no
  `ownScope` field to set. An admin element is never own-scoped.

A member-facing write (posting a message, reacting) is not administrative: its invokers are every
signed-in person and its conditions test no role, so it may be declared for members.

### Declaring it

Every element declares an **audience**: `"public"`, `"members"`, `{ "roles": [...] }`, or
`{ "groups": [...] }`. An absent audience means the parent's, except where a declaration is required.

**Audience groups (D-DS-21, owner decision).** A group is a named set of roles defined once in the
tenant's interface (`groups: { "staff": ["moderator", "editor"], "admins": ["tenant_admin"] }`); the
owner role is a member of every group implicitly. The number of roles is unbounded. Groups are draftable
and need approval.

**The serving cap and why (D-DS-24).** A viewer's **interface class** for a screen is formed from groups
only: the set of the screen's **deciding groups** the viewer is in (a declared roles list counts as one unnamed
group; a group decides when removing it would
change some element's effective declared audience on that screen or its variants), and whether the
viewer is a builder. Data audiences, which depend on roles of grants and invokers and are unbounded, are
**not** part of the class: the gateway caches the class's part, and applies each viewer's data audiences
to it when serving (a pure filter of the cached part by the viewer's roles; the result is named by its own
content hash). So a screen has at most 2^g classes for g deciding groups, which is why the cap is per
screen: it bounds the parts the gateway builds and caches per screen. The cap is a **tenant setting**,
default 6, under a platform maximum that WG-5465 measures (build time and memory per class); 32 groups
per tenant is a readability limit of the builder, stated as a choice. `ui.audience.too-many-groups` names
the screen's deciding groups and suggests merging groups that always appear together, so it converges.

| Level | Where declared | Default |
|---|---|---|
| shell entry | the tenant's ordered shell list | required |
| navigation entry | its entry | its screen's (variant's) audience |
| screen variant | the variant | **required** |
| region | `regionAudience` | the screen's |
| layout container | the container | its parent's |
| widget | the widget | its parent's; **required** when the widget is admin |
| field rendering, column | the widget's props | the widget's |
| action, gesture | its placement | the widget's |
| link in a `link-list` | derived: its target screen's effective audience | |

**What is served.** A viewer is served an element when every declared audience on and above it holds the
viewer and its derived **data audience** does (Read grants on its object type, the public role included;
for an action, its invokers). A container is served when something in it is served, and, if it needs
content, when an element that counts is served in it. Derived audiences only narrow. A child may not
declare an audience wider than its parent's (`ui.audience.wider-than-parent`). **Served parts carry no
declared audiences and no group names**: the gateway removes them, since a member need not learn that a
group called "admins" exists or which roles it holds.

**The content lint.** For each least viewer of a screen's declared audience, the kernel looks for an
element that counts with that viewer in its content audience (unconditional grants, or grants whose
condition mentions `principal`), scanning policy conditions, query filters and field read rules; when
none is found it **warns** (`ui.audience.empty-for`, severity warning), naming the viewer and the
widgets. What it establishes for what it reasons on is `checked_screen_has_content`; it is a lint, and
nothing depends on it for safety.

### The rule: nothing the data layer would refuse

For a viewer outside an element's effective audience:

- the element is not in the manifest, the part, or the view reply the viewer is served. The gateway
  filters a view reply **after the read**: it reads the screen's view as the viewer (so policies apply as
  for any read) and then removes every section no served widget binds;
- a container left with nothing, or one that needs content left without an element that counts, is not
  served (`served_has_element`, `served_needs_content`; `empty_region_pruned`,
  `static_does_not_keep_region`);
- no count says how many elements, rows or fields were left out (a positioned window's count is of
  records the reader may read);
- a screen the viewer is not served answers with the same not-found page, status and text as an address
  nothing answers;
- a column whose field the viewer's roles can never read is not in the viewer's shape;
- **a value withheld by a read rule** of a **required** field is marked so by the server: the row
  carries `withheld: [field ids]` only for required fields a read rule removed for this reader, and the
  interpreter draws them as "Restricted". Optional fields withheld by a rule are simply absent, with no
  marker in the reply (so neither the page nor devtools nor a scripted client learns that a rule withheld
  them); the interpreter never infers withholding from absence, so a subject's processing restriction
  (`subject_rights.rs::hide`, PRV-352) and a field the query did not select are drawn as absent values;
- aggregates count only records the reader may read (D-5061);
- an action the viewer may not invoke is absent.

Proved: `render_sound`, `served_has_element`, `served_needs_content`, `hidden_leaves_no_trace`,
`hidden_leaves_no_trace_in`.

**What is disclosed, stated.** A field's existence to readers of its record type when its read rule is
per-record; that a read rule withheld a required field's value (the "Restricted" mark), by the tenant's
choice of a per-record rule on a required field; the structure of a screen a viewer is served.

### Drafts, published, and dormant entities

**Every interface element has two bodies.** Screens, composites, composite templates, presentations,
navigation entries, forms, actions, filters, error messages, **groups, the shell list and variant order**
each hold a **published** body and, while being changed, a **draft** body. A draft step writes draft
bodies only. The draft's audience is the **builders**: holders of `ui-build`, a capability of its own,
granting reads of interface and schema definitions (object types, policies, invokers), no reads of
active data, and the **draft-data right** below. Overlays apply in preview.

**The effective schema (replacing the earlier "one gate").** The second review proposed one dormancy
gate at every entry point; the third showed there is no one place: `schema.policies` is iterated in
`rules.rs` (four places), `query_plan.rs`, `aggregates.rs`, `audience.rs`, `ledger_policy.rs`,
`approvals.rs` and `lab.rs`; `holds_any` and `effective_permissions` have 47 non-test call sites; and
`authorize_needing`, `visible_in` and `holds_any` exist more than once (in the kernel and in the
generated model). So dormancy is a matter of **what the schema is**, not of who asks:

- `TenantSchema` gains a **draft overlay** (`draft`), per product: the dormant object types (with their
  fields and constraints), queries, views, operations, policies and workflows, and every draft body. The
  maps every consumer reads (`objects`, `policies`, `queries`, `views`, `operations`, ...) hold the
  **active** entities only.
- Every evaluation for a non-builder is given the active schema, which is the schema minus its overlay:
  no consumer, at any call site present or future, can see a dormant entity, because it is not in what
  that consumer is given. For a builder, the gateway builds the **builder schema** (active plus overlay)
  once per request, through one function, and passes it where the builder acts on the draft (preview,
  invoking a dormant operation, reading dormant records).
- **No active entity may reference a dormant one** (an object type's reference target, a policy's
  resource, a view's query, a query's object type, an operation's types, a trigger's type or segment, an
  interface element's view): the classifier refuses a step after which one does. Proved: everything
  reachable by references from the schema a non-builder is given is active (`nonbuilder_reach_active`,
  `dormant_unreachable`), the invariant is kept by accepted steps and by publishing
  (`step_keeps_closed`, `publish_keeps_closed`), and without it a dormant entity is reachable
  (`unclosed_reaches_dormant`).
- A reference **from a dormant type to an active one** may not use on-delete `restrict` (refused): a
  builder's dormant records must never make a member's delete of an active record fail on data the
  member cannot see. `set-null` and `cascade` are allowed.
- **No active trigger can react to a dormant type**, because a trigger names an object type that
  exists in the schema it is defined in, triggers cannot be created in a draft step, and an active
  trigger may not reference a dormant type (the invariant above).
- **Quotas.** Dormant entities count against **draft quotas** (per tenant: objects, policies, queries,
  views, operations, each a fixed fraction of the active limit, stated in `limits.rs`), not against
  `MAX_OBJECTS`, `MAX_POLICIES`, `MAX_QUERIES` or `MAX_TRIGGERS`; publishing checks the active limits on
  the result. An abandoned draft therefore cannot exhaust the limits members' products need.
- **The draft-data right.** A builder may create, read, update and delete records of **dormant** types,
  and invoke dormant operations, under the right `ui-build` carries for dormant types only (policies in
  the overlay are evaluated as written as well, for previewing them as roles). Records of dormant types
  are ordinary records of types active evaluations cannot name; subject-rights paths (export, erasure,
  restriction) cover them like any record; the search index holds nothing of a type until it is active.
- Records builders create are marked `created-in-draft`; publishing requires **keep** or **purge** per
  dormant type, and the approval shows the counts and the activation's reach.

**Which kinds a draft step may create:**

| Kind | In a draft step |
|---|---|
| object types (new) and their fields, constraints and unique constraints | in the overlay (dormant) |
| queries, views (new) | dormant |
| tenant operations (new) | dormant; refused unless every step writes only dormant types and none posts to the ledger or sends mail or notices |
| policies (new) | dormant; refused unless they grant on dormant types only |
| workflows | dormant on a dormant type; otherwise needs approval |
| interface elements, groups, shell list, variant order | draft bodies |
| anything added to an active entity; changes to active operations, invokers, policies, queries, views | needs approval |
| `role_permissions`, roles, capabilities, settings, ledger declarations, mail and notice templates, tokens, triggers, automations, segments | needs approval |
| teams, invitations | not schema changes; never part of a product step |

**Publishing** is one product-level schema change, **`PublishDraft(product)`**, counted as **one** change
against `MAX_SCHEMA_CHANGES` whatever the number of entities it activates: the kernel expands it, inside
the same transition, by moving the product's overlay into the active maps and the product's draft bodies
into the published ones. It is atomic because it is one schema change, one commit, all or none, as any
schema change is; its size is bounded by the draft quotas, not by the cap, and the kernel checks the
active limits and the whole result (closure, admin check, every screen) before the commit. A fixture
publishes a product of 224 entities (the showcase community product's size). It is refused while any
placeholder remains, and needs approval.

**Abandoning.** `discard-draft` removes a product's overlay and draft bodies in one schema change;
dormant types with draft-created records are listed and removed only with the owner's confirmation.

### Relation to today's auth and to policies

`auth: anyone | session` becomes `public | members`; the role variant is what showcase gap 2 asked for,
generalised. Audiences narrow what the interface offers; they never grant data. A widget a viewer is
served still shows only the rows and fields the kernel's reads allow; an action is still decided when
invoked.

### Serving and caching

- **The interface class** (D-DS-24): the screen's deciding groups the viewer is in, and builder status.
  The gateway builds and caches a part per class; per request it applies the viewer's data audiences to
  the cached part (a pure filter by roles), and names the result by its content hash.
- **Parts are role-only**; principal- or clock-dependent presence shows an empty state; per-request
  results never enter the class cache.
- **Served parts carry no declared audiences or group names.**
- **What non-builders see of change**: a class content hash instead of the schema version; notices only
  when their manifest hash changes; draft steps notify builders only.
- **Error messages per class**, hashed per class. **A part is served only to a class whose manifest
  lists it.**
- **Anonymous visitors**: parts `public, immutable`; manifest revalidated with the class hash as entity
  tag. **Everyone else**: `no-store`, kept only in the principal's partition.
- **Other principals**: automation principals are refused by `/internal/ui`; an agent acting for a
  person is served that person's class narrowed by its delegation, and writes only as that person, with
  the agent recorded as actor (G-25).
- **Bounds**: the per-screen deciding-group cap (D-DS-24), 32 groups per tenant, a class cache bounded
  by entries and bytes.

## Role-based views

One composition serves every role through declared audiences; screen variants (D-DS-20) serve a role a
different screen at the same route, first match wins; a person with two roles gets the first variant
that holds them and, inside it, every element whose audiences hold them.

**Fields per role.** A role-only read rule removes the column from the shapes of classes that cannot
satisfy it; a per-record rule leaves the column, and the server marks withheld values explicitly.

**Actions per role: show, disable, hide.**

1. **Invokers** (stage 1): an action whose invokers do not name the viewer is absent (`audOf_sees`).
2. **Per row**, a kernel `preflight` answers `allowed`, `refused` (with the condition's code) or
   `unknown`: it runs stages 1, 3 and 4 with the row's inputs, under the viewer's read rights; `unknown`
   when an input is not supplied by the row, when a condition reads what the viewer may not read, and
   past a per-session budget. It is advisory: the invocation decides, and a refusal at the click refreshes
   the affordance. `decide-tenant-operation` calls the same stage functions; `allowed` never comes from a
   check the invocation does not run. `unknown` is drawn enabled for an ordinary action and **disabled**
   for an admin action (whose audience already limits it to role holders), so an admin control is never
   drawn as available when the platform cannot say it is.
3. **Forms** decide on submit.

**The admin panel, by construction.** An admin panel is a screen (or variant) declared for a group of
admin roles; every admin element on it inherits that audience; declaring it, or any admin element, for
members or the public is refused (`ui.audience.admin-for-members`, `admin_needs_role`). The four routes
by which earlier drafts let an admin panel reach members are closed because none of them is consulted to
decide who the screen is for: a conditional grant, an action with role-testing conditions (derived admin),
an author's own-scope marker (removed; own scope is derived), a filter or read rule (they only narrow).

**Preview as a role.** A builder may ask for the interface **as a set of roles or groups**, or as
anonymous. The interface (manifest, parts, which variants and shells are served) is computed for that set
exactly; the preview says which variant the set is served at each route and that later variants are not
seen by it (a manager who is also staff never sees the staff variant at a route where the manager
variant serves them), and that the owner, being in every group, is served the first variant. Data in a
preview is intersected per row and per field with what the builder may read; `ui-build` grants no reads
of active data, so a section the builder may not read is drawn as its skeleton with "you may not read
this data". Writes are refused; replies are `no-store`.

## Tenant-arranged layouts

The tenant's arrangement is an **overlay** element (`ui-layout`), one per screen (or variant), owned by
the tenant, addressing widgets, layout containers and regions by **id**. It may reorder, move, hide,
change props, add widgets bound to the screen's own sections, forms and actions, and narrow audiences.

**Precedence**: the catalogue constrains everything; the product's base screen first; the overlay on top;
the result is checked as one screen.

**When the product or the catalogue changes.** Every overlay whose base changed is checked again. Only
**overlay entries** are orphaned: the product's own base widgets always pass, because the product's own
check ran on them (an upgrade whose base fails is refused to its author). The model's `orphan` filters
the effective screen, whose base widgets pass by that check, so it removes exactly the failing overlay
entries (`orphan_valid`, `orphan_removes_only_failing`, `orphan_keeps_valid`). Other overlay operations
than dropping are checked by the kernel, not modelled.

**Orphans are reported, and never block an upgrade.**

- The upgrade's result and its dry run list orphans **per tenant**; a product author's dry run against a
  tenant shows the list before anything is applied.
- The tenant's owner and the overlay's author are notified, with "drop" and "rebind" per entry.
- **When orphaning would take a screen away from an audience** (a screen or variant that served an
  audience serves it nothing after orphaning), that audience is served the **product's base screen**
  for that route (the screen without the overlay), which passed its own checks, and the owner is
  notified. The upgrade, security fixes included, is never held for an overlay.
- Automatic dropping, when chosen, records each dropped entry and can restore it.

An owner edits overlays on the **layout editor** (builder-only), or an agent writes them; both go into the
overlay's draft body.

## The catalogue

`specifications/ui-catalogue.jsonc`, read by specc (as `deploy.jsonc` is), has the sections
`version`, `tokens`, `contrastPairs`, `themes`, `fonts`, `primitives`, `layouts`, `patterns`,
`blocks`, `templates`, `shells`, `bindings`, `gestures`, `displayTypes`, `sampleLabels`, `codes`.
Each entry of the entry sections holds:

```jsonc
{ "id": "data-table", "since": "1.0", "summary": { "en": "Rows and columns of a list section ..." },
  "binds": ["section"], "states": ["loading", "empty", "error", "ready", "placeholder"],
  "props": { /* JSON Schema of the props; every human-readable string is of type "text" */ },
  "regions": ["main", "below"], "limits": { "rowActions": 5 },
  "examples": [ { "name": "people", "props": { ... }, "data": { ... } } ],
  "refused": [ { "name": "column of a missing field", "props": { ... }, "code": "ui.binding.unknown-field" } ] }
```

- **Generated from it:** the kernel's validator tables (Rust), the interpreter's tables
  (`web/src/generated/catalogue.js`), the token stylesheet of each preset, and the gallery's fixtures.
- **Served:** `GET /api/ui/catalogue` (public, named by its hash) and the agent gateway's catalogue
  read, with the JSON Schemas of the product file sections that hold interface (`screens`,
  `composites`, `templates`, `presentations`, `navigation`, `forms`, `actions`, `filters`).
- **Checked:** every accepted example validates and every refused example is refused with its code,
  in the kernel and in specc.
- **Localisable from the first version.** Every human-readable string in a prop (a label, an empty
  state's sentence, a confirm dialog's text, a note) has the type `text`: a string, or
  `{ "default": "...", "<language tag>": "..." }`. The interpreter picks the request language
  (D-6593), then the tenant's default language, then `default`.

## Versioning: schemas and the rendering contract

**The catalogue version** covers the schemas; `scripts/catalogue-compat.mjs` refuses a narrowing labelled
minor, deciding structurally and approximately.

**The rendering contract version** covers markup, the class contract, the page model, display-type
formats and fallbacks, and token meanings, with **golden outputs** (page model and serialised DOM per
example, and the pinned screenshot of each). Whether a change is major is decided by the gate:

- **one comparison**: pixels are compared **exactly** on the pinned image (same container digest, same
  Chromium, same fonts); a golden DOM diff that adds, removes or renames a class outside the allow-list
  of state classes, changes element structure, or an exact pixel change to an existing example, is a
  **major** change; any other golden diff is minor and listed;
- **tool bumps**: a stack that only bumps the pinned image, Chromium or the vendored fonts is classified
  apart: it may change pixels, not DOM; it is checked against a **calibration set** (a fixed list of
  examples covering every primitive) and passes when no calibration example moves more than a stated
  bound (a share of its pixels, in the gate's configuration, which is a baseline-class file); it then
  re-baselines in the same stack;
- **the previous major is gated too**, DOM and pixels;
- **security fixes**: a change that moves either major's goldens must be classified as above, except a
  security fix recorded as a Workgraph task named in a `Security-Fix:` trailer, applied to both majors,
  whose goldens it leaves **identical** on both (a fix that changes what is drawn is a major).

**Lifecycle (D-DS-22, owner decision): warn, then migrate, reversible.**

- The platform supports the current and the previous rendering major. The operator sets a major's
  end-of-support date and a **grace period** that follows it; during the grace period the web
  application still ships that major's renderers, so a tenant moved off it can move back.
- **Warnings**: every tenant still on the leaving major gets at least two warnings in the builder, at
  separate builder sessions, each with the date and the preview; the platform records when an owner or
  builder has opened one. An owner who has not opened any by the date is also notified by mail to every
  owner address. Inactive owners do not stop the move.
- **The preview** (`/design/preview?contract=<major>`) draws the tenant's real compositions through the
  new major's renderers, using the **production renderer path** (the same modules, selected by the
  contract version), as a member, as an anonymous visitor and as each group (the preview-as-roles of
  "Role-based views"), with the tenant's data as the builder may read it.
- **The move** is one recorded definition step: it re-checks every composition under the new major's
  catalogue (orphaning and reporting overlay entries as an upgrade does), re-runs the theme step (the
  derivation version's defaults for any tokens the new major adds, contrast checked), and records
  the before-state; the owner can reverse it until the end of the grace period.
- **Two majors behind** (possible only for a tenant moved back during a grace period that then ended):
  the move goes through each major's migration in turn within the one recorded step.
- Deprecated tokens and classes are aliases for one major.

**Corpus replay.** `tests/corpus/ui/` holds the showcase's, synthetic and adversarial compositions;
anonymised exports enter only after an exporter and the owner's privacy review.

## Version skew

- The **manifest** and **every part** name the rendering contract version and catalogue major they need;
  the **shell** names the ones it serves; a mismatch is never drawn and the page shows "This page has
  been updated. Reload to continue".
- **On a floor notice** (a role lost, a permission or policy change, D-FE-12) the page purges at once,
  before the new manifest arrives, every interface part it holds for the old class, and draws loading.
  It then reads the new manifest. **Any change of roles is treated as narrowing** (a role lost and
  another gained at once included): the page never decides "wider or narrower" itself.
- **A swap with no floor change** (a schema notice) is atomic: a new manifest is drawn only once every
  part the page needs and the view replies whose shape fingerprints match are held; past the contact
  bound of docs/frontend.md the page shows an error state with "Retry", never a mix.
- **Minimum client version.** Today nothing in `web/src` or `crates/bff` handles `426`, a
  `Strata-Reload` header or a forced reload; status handling covers 401, 403, 404 and 409, so an old
  shell answered `426` would show an error state, not reload. So: the handling of `426` with
  `Strata-Reload: 1`, and a minimum version carried by the bootstrapped page, **arrive with the first
  shell of this programme**; the server never sets a minimum above the oldest shell version known to
  handle `426` (it records that version and refuses a higher minimum); `index.html` stays uncached
  (`no-store`); and a recorded fixture of that first shell is tested against a minimum bump, reloading.
  Shells older than that first one get the error state, which is stated.

## International and accessible by rule

- **Logical properties only.** Tokens and layout classes use `inline` and `block` (`margin-inline-start`,
  `padding-block`, `inset-inline-end`); the stylesheet lint refuses physical properties (`left`,
  `margin-right`) outside an allow-list with a reason. The root element carries `dir` and `lang` from
  the request language (D-6593) or the tenant's default; icons that point (arrows, chevrons) flip in
  right-to-left.
- **Scripts and fonts.** The catalogue's `fonts` section lists vendored families with their licence,
  hash, and the scripts each covers; a theme's font pairing must cover the tenant's languages or name a
  system fallback per script. New families are added by platform review (a catalogue minor); tenant
  uploads of font files are not offered, because a font file is a parser input and `font-src 'self'`
  would serve it as the platform's own (an open question below).
- **Focus and change.** Every served element has a stable key (its id); the renderer patches by key.
  When a redraw removes the focused element, focus moves to the nearest surviving container's heading
  (made focusable with `tabindex="-1"`), and a polite live region says what changed; when it reorders,
  the moved element keeps focus. A builder watching an agent's step hears the step's label once in a
  polite live region; focus never moves because of someone else's step. A member's page redraws on a
  notice the same way, with no announcement unless the focused item changed.
- **Forced colours, targets, reflow, safe areas, touch**: see "Layer 2: primitives" and "Layer 4".

## The agent build loop

### What exists today, and what it does not do

The first draft of this section assumed more of the product paths than they do; the review checked
them and was right:

- `plan_upgrade` (`crates/substrate/src/product.rs:1930`) refuses an upgrade whose version is not
  strictly greater than the installed one, and `release_parts` (`product.rs:1488`) reads
  `1.0.0-stage.2` as `[1, 0, 0, 2]`, above `1.0.0`; so "undo by applying the previous version" is
  refused, a staged scheme with pre-release suffixes cannot end at its final version, and a retried
  upgrade at the same version is refused before the idempotency key is consulted (the backend plans,
  then runs the intent with the key: `crates/backend/src/products.rs:190-230`).
- `products::write` requires the owner and a fresh step-up on every write that is not a dry run
  (`products.rs:316-334`); no delegation can call it.
- The upgrade route is gated by `bundle-install` alone (`precheck.rs`); nothing classifies its diff;
  `propose-change` (CT-112) carries a change set, not a bundle; and `foreign_change`
  (`crates/kernel/src/products.rs:338`, INT-1582) refuses a change to a product's entities except
  through that product's upgrade, so a proposal cannot carry one either.
- `bundle_error` replies `{code, kind, message}` (`products.rs:59`); kernel refusals are strings; a
  stale `expected_version` is the generic `SchemaConflict` (`crates/kernel/src/transition.rs:2141`),
  tenant-wide, so any commit anywhere conflicts every agent step.
- Product segments are defined through the segment store around the schema change, not inside it
  (`products.rs`, WG-5166): a step is atomic for the schema, not for its segments.

The loop below is what the platform must offer; each gap is a deliverable of WG-5471, WG-5472 or
WG-5482.

### End to end

1. **Read.** The catalogue; the backend specification's product format; through the **agent surface**
   (WG-5483): installed products and versions, every screen body with ids, valid bindings with shapes and
   fingerprints, policies and invokers (a `ui-build` holder reads definitions), and the product's last
   installed source.
2. **Compose** the tenant app product.
3. **Validate**: `POST /api/products/compile` (bundle, provenance table, diagnostics); then a dry run (the
   projection, the classification, refusals mapped to pointers). A dry run's classification is shown to
   the agent and never trusted by the apply.
4. **Lease.** The product's **build lease** is **tenant kernel state** (the current fencing token and its
   expiry, stored with the tenant's schema state, so a restart does not reset the counter or reissue a
   token). Acquiring it raises the token and sets the expiry from the clock the kernel is given; the
   holder renews it before the expiry; the owner can break it. The check `token == current ∧ now <
   expiry` runs **inside the transaction that applies the step**, and a step that fails it is refused
   with `product.lease-lost` (`no_stale_step_after_acquire`, `no_step_after_expiry`). Held upgrades stay
   held when the lease changes hands; they are bound to their diff, not to the lease.
5. **Apply** into the draft, with the product base, the token, `"version": "next"`, the idempotency key
   (matched before any version check) and a label. The **classifier runs inside the applying
   transaction**, on the diff the transaction computes.
6. **Held upgrades.** An upgrade that needs approval is held with its bundle, its diff, its projection,
   its classification and a digest of them. On approval, the platform recomputes the diff against the
   schema as it is then; it applies only when the recomputed diff equals the held one, and refuses with
   `product.base-moved` otherwise (`approve_applies_held`, `approve_class_matches`,
   `approve_refused_when_moved`). The owner approves with a fresh step-up.
7. **Watch**: builders' connections draw each step; members hear nothing.
8. **Publish** (needs approval), with keep-or-purge per dormant type.
9. **Undo** is a new step at the next version carrying an earlier version's source.

When a step would orphan overlay entries of the tenant, it is held for approval and the tenant's builders
are notified (see "Tenant-arranged layouts").

### Classification and authority

| Class | What it holds | Who may apply it |
|---|---|---|
| interface into the draft | draft bodies of any interface element | an agent delegation with `ui-build` and `bundle-install` scoped to this product; no per-step step-up |
| dormant data | the kinds the "Drafts, published, and dormant entities" table creates dormant | the same |
| needs approval | everything else in that table, publishing, removals, migrations, steps that orphan overlay entries | held; applied only on the owner's approval with step-up, bound to its diff |

A kind not named is "needs approval". Scoping `bundle-install` to products is new (`tenant-agent` with a
`products` list). The owner's own path keeps `require_owner` and step-up.

### Diagnostics and convergence

```jsonc
{ "code": "upgrade.removal-in-use", "severity": "error",
  "message": "Removing the field showcase_community__message.channel is refused: it is still bound.",
  "path": "/objects/3/fields/channel",
  "related": [ { "path": "/screens/2/regions/main/0/props/columns/2/field", "entity": "showcase_community__messages" },
               { "tenantOverlay": "ui-layout:messages", "entry": "w-14" } ],
  "clears": [ { "sequence": "restructure", "steps": ["rebind-draft", "publish", "remove"] },
              { "op": "remove", "path": "/screens/2/regions/main/0/props/columns/2" } ] }
```

- `path` and `related`: a refusal that involves several entities (a reference cycle, a removal blocked by
  another product's or an overlay's use, a migration, the change cap) names each through `related`, with
  pointers from specc's provenance table or, for entities outside the product file (another product, an
  overlay), their ids.
- `clears` lists the closed set of edits that remove the refusal; an entry may be a **sequence** (the
  restructure below), which the agent carries out over several steps.
- **Convergence contract**: every code states its `clears` and has a negative fixture, including
  fixtures for refusals across entities (a cycle of three object types, a removal blocked by an overlay,
  a removal blocked by another product, a cap overflow); applying one `clears` entry (all of a sequence's
  steps) makes the fixture pass.
- **Loop budget**: the reference driver stops after a stated number of refused steps, and when the same
  code at the same path returns after its `clears` edit, except while it is inside a declared `clears`
  sequence; a legitimate restructure is never counted as a failure.

### Restructures

1. Add the new field or entity (dormant if on a dormant type; otherwise held).
2. Copy the data with **`CopyField`**, a typed schema change new to the kernel: `{ object, from, to,
   convert }`, where `convert` is one of a closed list of conversions between field types (identity,
   integer to text, text to integer when every value parses, time to date, choice renaming by a map). It
   copies in batches of at most 10,000 records (`MAX_SCAN_RECORDS`), each batch its own schema change,
   and **marks** each copied record with the record version it copied at, so "written since the copy" is
   "the record's version is above its mark". The kernel's existing migration (`plan_migration`,
   `transition.rs:1948`) moves and re-encodes fields in place and loads every record of the type; it is
   not used for this, and the unbounded load is a separate finding for WG-5464.
3. Rebind the draft's widgets to the new binding.
4. **Publish with a catch-up copy in the same schema change**: `PublishDraft` re-runs `CopyField` for the
   records whose version is above their mark; when more than 10,000 remain, publish is refused with
   `publish.catch-up-too-large` and the agent runs more batches first. The held publish's digest covers
   the **rule** (`object`, `from`, `to`, `convert`), never the count of records, so a changing count does
   not make the approval fail with `product.base-moved`.
5. Remove the old field or entity (held; the guard passes).

A type change goes through the same sequence with a new field; changing a field's type in place is
refused while any binding's fingerprint names it.

### Interleaving wireframe, data and styling steps

Three kinds of step act on one product file, in any order that keeps each step valid; the only
ordering rule is that a binding needs the data it names. Building the showcase's message centre:

| Step | Kind | What changes | Changes in the step | What the builder sees |
|---|---|---|---|---|
| 1 | wireframe | screens `/messages` (list) and `/messages/:id` (detail) as drafts with placeholders; navigation entry | about 6 | Two draft screens in the neutral theme: labelled regions, skeletons with sample labels |
| 2 | dormant data | objects message, comment, reaction; policies; queries and views | about 60 | Nothing changes on screen; the build log says the data model is in |
| 3 | dormant data | operations post, comment, react, hide; their invokers | about 10 | Nothing on screen |
| 4 | binding | the table bound to `messages`, the card to `message`, the thread to `comments` | 2 | Real (builder-created) messages in the table; the reaction placeholder still a skeleton |
| 5 | styling | presentations: person, datetime, status, rich text | 3 | Names instead of principals, "3 hours ago", status badges |
| 6 | binding | forms, row actions and gestures, prefill | about 8 | The post form, the react buttons on each row |
| 7 | styling | the tenant's theme seeds | 1 (configuration) | The tenant's colours and fonts |
| 8 | publish (needs approval) | `PublishDraft`: draft bodies to published, the overlay activated, keep-or-purge per dormant type | 1 (`PublishDraft` is counted as one change whatever it activates) | Members see the message centre |

(The change counts are estimates of entity counts for this example, not measurements.)

### The 200-change cap

A step carries only what changed (a changed entity re-emits its definition: one change); the dry run
reports the count. A first install over the cap is **staged** (D-DS-16):

- **Versions.** Product versions get their own comparator, semantic versioning with pre-release ordering
  (`1.0.0-stage.1` < `1.0.0-stage.2` < `1.0.0`). **Platform releases keep theirs**: `release_cmp` is also
  used by `PlatformRange::admits` (`product.rs:1519-1523`) against `PLATFORM_RELEASE = "0.2-draft"`
  (`crates/backend/src/products.rs:42`), and the showcase products declare `"min": "0.2"`; so only
  product versions move to the new comparator (a fixture: the showcase products install on
  `0.2-draft`). Because `from_json` accepts any string as a product version today, a fixture orders every
  version string present in the repository's products, corpus and specification examples (`1.0`, `1.0.0`,
  `1.1.0`, ...) and checks the old and new comparators agree on every pair that has no pre-release part;
  another checks that `from_json` accepts stage versions.
- **Publishing a large product** is not limited by the cap: `PublishDraft` counts as one change (see
  "Drafts, published, and dormant entities"), with a 224-entity fixture.
- `plan_install` (pure, to be written in `strata_substrate::product`) orders the strongly connected
  components of the bundle's reference graph topologically and packs them into stages of at most 200
  changes, no stage naming an entity of a later one; a single component over 200 is refused with
  `install.component-too-large`, naming its entities; the plan is recomputed from the bundle on each
  attempt and never persisted; stage records are internal and the visible history shows the final
  version.
- The kernel's cap and the all-or-none rule of one product change stay. Product segments are still
  defined outside the schema change (`products.rs`, WG-5166): after a crash between the change and the
  segment definitions, segments can be half-defined; this is open (see "Open questions").

### Prior art

Exegra's agent interface (`C:\dev\exegra\docs\agent-interface.md`): one envelope, `apply` with
`--expect-hash` and `--dry-run`, `admit` listing every failing reason, diagnostics with entity ids and
positions. The showcase's live driver (`tests/corpus/showcase/live.mjs`) shows every step can go
through public routes; the corpus loader's staged install is D-DS-16 by hand.

## Reserved entries and the enterprise work they wait for

The enterprise sweep (WG-5484) found needs the platform's backend does not meet yet. The design reserves
their interface shape now, so each becomes a catalogue **minor** change when its backend lands, and
states what it relies on. A **reserved** catalogue entry has its schema and examples from the first
catalogue version and the status `reserved`; the kernel refuses a published composition that uses one
(`ui.catalogue.reserved`, naming the backend task), and a draft may hold it only as a placeholder.

**Human tasks: the task inbox, the decision form and the `decide` action** (waits for WG-5496). Today a
tenant action can only invoke an operation (`UiAction`, `crates/model-core/src/types.rs:9700`), and
`approve-workflow-step` is a gateway route (`crates/bff/src/routes.rs`) the interpreter never calls.
Reserved:

- a platform source **`tasks`**, bound to WG-5496's pending-for-me contract;
- the **`task-inbox`** pattern over it, with claim, release and reassign as actions on each task;
- the **`decision-form`** pattern for one task: the task's subject as a reference, its declared outcomes
  as buttons, an optional comment;
- a new **action kind `decide`** in the `ui-action` union, beside today's operation action:
  `{ "kind": "decide", "task": "row.id", "outcome": "<one declared outcome>", "comment": "form.<field>" }`.

The shape assumes only what WG-5496's acceptance criteria name: a list of the viewer's open tasks with an
id, a title, a subject reference, whether the viewer is the assignee or a candidate, and an optional due
time; claim, release and reassign; and a decision with an outcome from the task's declared set and an
optional comment. It assumes nothing about escalation, deadlines enforced by the platform, delegation of
tasks or bulk decisions; those would be further minor additions. Who sees a task is the backend's
assignee expression, evaluated by the server for the pending-for-me read, so the inbox is a data
audience like any other and the admin rules apply unchanged: a `decide` action is not admin by itself,
and the task's operation's own effects are classified by its definition.

**Process timeline** (waits for WG-5507). Runs and steps live in the job store, not in tenant records,
so nothing can bind to them today. Reserved: the **`process-timeline`** pattern, bound to a list section
over WG-5507's run and step object types (state, actor, start and end times), ordered by step start.
Until the projection exists **nothing renders**: the entry is reserved, a published screen cannot use it,
and a builder may keep a placeholder for it in a draft. A run's status can be shown meanwhile only
through fields a product's own operations write to its records.

**Audience classes under principal attributes and time-bound grants** (WG-5500, WG-5499). Any condition
that names `principal` or `now` makes a data read's audience class individual (`audience.rs` `scan`), so
such reads are never shared. The interface stays shareable because of D-DS-24, and the rule is stated:

- **A class key holds only the screen's deciding groups and builder status.** Nothing that depends on a
  principal attribute, on the clock, on an individual grant, or on team membership may enter it.
- Principal attributes, time-bound grants and conditions naming `principal` or `now` act only in the
  **per-request narrowing** of a class's cached part by the viewer's data audiences, and in the data
  reads themselves; they never split the class cache.
- Declared audiences name roles and groups only. If audiences ever need attributes, they will be a new
  group kind evaluated per request in the narrowing step, not a class dimension.
- A time-bound grant that starts or ends must raise the access generation of the principals concerned
  at that moment (so browsers purge and read their new manifest, D-FE-12); the design relies on WG-5499
  doing that and does not poll clocks itself.

**Teams and invitations change the schema without a new version** (`put_team` and `put_invitation`
write `PutSchema`, `crates/kernel/src/transition.rs`). The design relies on the following, and on
nothing keyed by the whole schema's version or hash:

- the gateway's interface definition cache and class cache are keyed by a hash of the **interface-relevant
  parts** of the schema (interface elements, groups, and the object types, policies, queries, views and
  operations their bindings name), which excludes teams and invitations, so a membership change does not
  invalidate them;
- a viewer's roles, and so their groups and class, are computed **per request** with `roles_of`, which
  includes team roles, so a team change takes effect at the viewer's next request;
- a team membership or definition change raises the access generation of the principals concerned
  (D-FE-12, WG-5435), so their browsers purge and read the manifest again;
- the held-upgrade digest covers the product's diff, which never contains teams or invitations;
- if WG-5500 moves teams and invitations out of the schema document, none of this changes.

**Exact numbers and civil time** (WG-5494). The display types do not lock in a representation: `number`
and `money` format an exact decimal string (never a floating-point number), take currency, scale and
rounding from the field's type once it has them and from props until then, and `date` formats a civil
date as itself rather than through a UTC instant. Each display type's list of accepted field types is
additive, so WG-5494's decimal, money, date and duration types are a minor change to the catalogue.

**Field work offline: named gaps.** None of these falls out of the existing design, and each waits for
a decision:

- a **work pack** (a declared set of views downloaded for use offline beyond a session) waits for the
  realm opt-in of D-FE-11 to be extended to a declared pack element and its size bound;
- a **camera and photo queue** waits for a decision to queue file content at rest in the browser
  (encrypted, under a stated quota), which D-FE-5's queue of intents does not cover;
- a **large-file upload queue** waits for a resumable upload contract in the file store (B-14) and the
  same decision on queued file content.

**Acting as another person** (WG-5499): the shell's `header.actingAs` slot is reserved (see "Layer 4").

## Governance

- **The gallery** (`/design`, builder-only) draws every catalogue example in every state and every
  corpus composition.
- **One pinned environment for pixels**: a Linux container, the Chromium of the pinned Playwright
  version, vendored fonts, `system-ui` mapped to a vendored family in the test build; a smoke run per
  engine checks layout invariants with the real system fonts.
- **A sampled matrix** (the pair table: primitives in both themes, schemes and widths, compact density,
  forced colours, right-to-left, Japanese and 400% zoom; patterns and blocks in both themes, schemes and
  widths with right-to-left on desk; corpus compositions in both themes, light, both widths; generated
  stress themes on desk); the spec prints its count.
- **Accessibility**: axe at WCAG 2.2 A and AA on every gallery and corpus page; keyboard scripts.
- **Baselines, checked at the stack tip, after squashing.** The gate runs on the integration tip after a
  stack lands (Workgraph stack verification), so a squash or a red-then-baseline sequence is judged as the
  whole change. It runs the screenshot suite twice: the **new code against the old baselines**, where
  exactly the examples listed in the stack's `Baselines:` trailers must fail (and every other example
  pass), and the new code against the new baselines, where every example must pass; so regenerating all
  baselines cannot hide a change, because every example that moved must also have failed against its old
  baseline and been listed.
  - The screenshot configuration (thresholds, masks, the pinned image digest, the calibration bound) is
    a baseline-class file: changing it counts as changing every baseline it governs.
  - A change moving more than 10 examples or 1% of the matrix needs a `Baselines-Approved: <task>`
    trailer; the gate asks Workgraph (`workgraph --json task show <task>`) that the task exists, is
    accepted, and names this stack, and fails when Workgraph cannot be reached.
  - A tool bump is its own stack, classified by the calibration set (see "Versioning").
- **Contrast**: the catalogue's pairs on every preset at generation, at theme definition, and on every
  stored theme whenever a token or a pair is added.
- **Themes over time.** A tenant theme stores its seeds, derivation version, derived set and hash. Each
  derivation version states, for every token added in a later catalogue minor, how it is derived from the
  stored set (for example `control-border` from `border-strong`). A **contrast pair added later** is run
  over the stored-seed corpus before it ships (the gate refuses a non-text pair with no default that
  passes there), and over every stored theme at deployment; a stored theme that fails it is **not
  patched**: the failure is shown to the owner as a non-blocking warning in the builder until the owner's
  next theme step, which must pass it. A stored theme changes only by a definition step (the owner's or
  an approved agent step), which is a new derivation version with a record of the previous derived set
  and an undo; the stored hash is always the hash of the derived set in force, so no stored theme's hash
  is ever silently out of date.
- **Versioning**: `catalogue-compat.mjs` and the golden outputs in the gate, with majors decided by the
  gate.

## Acceptance target: the showcase re-expressed

The four showcase products are rewritten as compositions, composites, presentations and navigation,
with no JavaScript, CSS or HTML in `tests/corpus/showcase` beyond the drivers (`live.mjs` and the agent
build script), and built by the agent build script from wireframes to a published product through the
public API. It looks and behaves like a finished product when all of these pass:

1. **Screenshots** in the pinned environment: every showcase screen as the tenant admin, a member and,
   for public screens, an anonymous visitor, in the neutral and Fieldnotes themes, light and dark,
   phone and desk, against baselines the owner has accepted.
2. **Accessibility.** axe at WCAG 2.2 AA on every one of those pages, no violation.
3. **No raw values.** No text matches an epoch (`\b1\d{12}\b`), a principal (`\buser:`), a JSON object
   (`{"`), or a record id outside a `code` display; no form asks for a record number.
4. **Roles.** A member's navigation has no admin entry; a member at `/admin` gets the same page, status
   and text as an unknown address; an anonymous visitor is served only public screens; the moderator's
   message page has the hide action and the member's has none, in the served part and on the page; no
   byte served to a member names an admin-only element or error message.
5. **Live build.** The agent build script runs against a fresh stack; a screenshot after each step shows
   a valid draft; members are served nothing new, and notified of nothing, before the publish step.

"Looks finished" is the owner's judgement; the screenshots are its evidence and the checks its floor.
Showcase gaps that are backend work stay theirs (1, 3, 4, 7, 8, 11, 12, 14); the target needs gap 2's
platform admin role (WG-5474) and gap 6's reference coercion (in WG-5466).

## The specification part

`specifications/design-system.jsonc` is an area part (`AREA_PARTS` in `scripts/split-spec.mjs`: ids that
are `ds-` and a number, or start `d-ds-`; the behaviour-model ids `ds-save`, `ds-open` and so on are not
in it). It holds twenty-seven decisions: D-DS-1 to D-DS-6 and D-DS-20 to D-DS-22 are the owner's, decided
2026-11-22; the others are proposals, with a source saying so. It holds eighteen requirements, DS-001 to
DS-018, milestone 7, each stated of its Lean model and checked by the theorems that carry its `Verifies:`
line. Requirements checked by tests are added with those tests:

| Planned requirement | Its check | Added by |
|---|---|---|
| Every preset theme meets AA on every catalogue pair, in both schemes | generation check with a failing fixture | WG-5458 |
| A theme definition with a failing pair is refused; a later pair a stored theme fails is reported, never patched | substrate tests | WG-5460 |
| Every accepted catalogue example validates and every refused example is refused with its code | kernel and specc tests | WG-5459 |
| A composite with an unnamed control, a recursive reference or over 200 expanded nodes is refused | kernel test | WG-5479 |
| An admin element declared for members, the public or a default role is refused; `react_to_message` stays member-facing | kernel tests | WG-5465 |
| No non-builder evaluation is given a dormant entity; no active entity references one; dormant references never restrict an active delete | kernel tests, contract-enumerating gateway test | WG-5464 |
| `PublishDraft` publishes a 224-entity product as one change | kernel fixture | WG-5464 |
| A withheld marker appears only for required fields withheld by a read rule | backend test with a restricted subject | WG-5463 |
| The lease check runs in the applying transaction and survives a restart | backend test | WG-5471 |
| A golden output that changes a non-allowlisted class, structure or exact pixels is a major | gate check | WG-5478 |
| Baselines: the listed examples fail on old baselines and pass on new ones | gate check | WG-5473 |
| The showcase installs on platform release 0.2-draft; old and new comparators agree on every installed version string without a pre-release part | substrate fixtures | WG-5472 |
| No page of the showcase shows an epoch, a principal or a JSON object | Playwright | WG-5475 |

## What is proved

`lean/Strata/UI/Composition.lean`, `Contrast.lean`, `Visibility.lean` and `Loop.lean`, imported from
`lean/Strata.lean` and listed in `lean/Audit.lean`: 48 theorems, no `sorry`, `admit` or added axiom.

| Property | Theorem | Requirement |
|---|---|---|
| The check is exactly validity | `check_iff`, `checked_bindings_resolve` | DS-001 |
| Publishable: no placeholder; data widgets bound and accepted | `publishable_no_placeholder`, `publishable_bound` | DS-002 |
| Refinement valid at every step, ends publishable | `bindAt_valid`, `bindAt_placeholders`, `refine_valid`, `refine_publishable` | DS-003 |
| Incremental checking over a changing schema and catalogue | `incremental_valid` (counterexample `unguarded_removal_breaks`) | DS-004 |
| The derived on-colour meets AA; ramp search succeeds | `onColour_meets_aa`, `search_finds` | DS-005 |
| The theme check is exactly the pairs | `checkTheme_iff` | DS-006 |
| Served implies seen, inside seen containers | `render_sound` | DS-007 |
| A node not served leaves no trace | `hidden_leaves_no_trace`, `hidden_leaves_no_trace_in` | DS-008 |
| Action audiences match the invoker check | `audOf_sees` | DS-009 |
| Data audiences match grants, public included; content inside data | `dataAud_sees`, `public_grant_anon_sees`, `contentAud_sub` | DS-010 |
| Admin elements only to holders of a strict declared role other than public and the defaults | `admin_needs_role`, `admin_never_to_default_member` (`admin_action_for_members_refused`, `admin_default_role_refused`) | DS-011 |
| Containers served hold elements; those needing content hold counting ones | `served_has_element`, `served_needs_content` (`empty_region_pruned`, `static_does_not_keep_region`) | DS-012 |
| Orphaning per widget check keeps validity under a new catalogue and schema | `orphan_valid`, `orphan_removes_only_failing`, `orphan_keeps_valid` | DS-013 |
| Variants: the first that serves the viewer; shells: the first match | `selectServed_sound`, `selectServed_first`, `select_sees`, `select_first` (`variant_falls_through`) | DS-014 |
| The lease: current token, before expiry | `no_stale_step_after_acquire`, `no_step_after_expiry` | DS-015 |
| Approval applies exactly the held diff, or is refused | `approve_applies_held`, `approve_class_matches`, `approve_refused_when_moved` | DS-016 |
| Nothing reachable from the non-builder schema is dormant | `nonbuilder_reach_active`, `dormant_unreachable` (`unclosed_reaches_dormant`) | DS-017 |
| Accepted steps and publishing keep the closure invariant | `step_keeps_closed`, `publish_keeps_closed`, `publish_view` | DS-018 |
| What the content lint establishes | `checked_screen_has_content` | supporting (a lint) |

**Not proved, and what carries it.**

- The code against the models: the kernel's composition and admin checks, the gateway's filtering, the
  preflight, the construction of the active and builder schemas, the classifier's closure check: tests
  and differentials in WG-5464, WG-5465, WG-5466, WG-5471.
- **The classifier runs inside the applying transaction**: not modelled; `approve_class_matches` takes
  the classification as a hypothesis.
- The loop beyond its guards: version assignment, idempotency before version, the product base,
  `PublishDraft`'s expansion, `CopyField` and its marks, keep-or-purge, staged installs: tested in
  WG-5464, WG-5471 and WG-5472.
- Luminance and rounding; shape fingerprints; nested containers in the composition model; overlay
  operations other than dropping; the interface class's computation; that `scan` finds every role test;
  that the admin derivation by effect covers access decided by data.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5457 with testable acceptance criteria (Workgraph is the record).

1. (WG-5458) **Token layer and preset themes as catalogue data**, logical properties only, 3 to 1
   non-text pairs. No dependency. May start (the review's verdict, with its conditions folded in).
2. (WG-5459) **The catalogue format, generator and schema compatibility check.** Needs 1.
3. (WG-5460) **Theme seeds, stored derivation, contrast refusal, CSSOM delivery, governed fonts.** Needs 1.
4. (WG-5477) **Localisable strings, direction, the logical-property lint, RTL and CJK coverage.** Needs 2.
5. (WG-5461) **Primitives, sheet, gesture hosts, forced colours, target sizes, reflow.** Needs 1, 2.
6. (WG-5462) **Display types and presentation elements.** Needs 2.
7. (WG-5463) **Section shapes and fingerprints in parts, the people table, reference titles.** Needs 6.
8. (WG-5464) **Compositions: templates, regions, layout containers, binding kinds, placeholders and
   samples, drafts, dormant entities, publishing, discard.** Needs 2, 6.
9. (WG-5465) **Visibility: declared audiences and groups, the admin check, derived data audiences and the
   content lint, interface classes, shell and variant selection,
   pruning, class hashes, notice targeting, per-class errors and sections, cache rules.** Needs 8.
10. (WG-5466) **Advisory preflight, row arguments, prefill, reference coercion.** Needs 8, 9.
11. (WG-5467) **Interpreter and pattern renderers, stable keys and focus.** Needs 5, 6, 7, 8.
12. (WG-5478) **Rendering contract versions decided by the gate, the warn-then-migrate lifecycle and its
    preview route, golden outputs, corpus replay, version skew.** Needs 11.
13. (WG-5479) **Composite patterns and templates as data, platform blocks, the renderer registry,
    catalogue gaps.** Needs 8, 11.
14. (WG-5480) **Structured bindings: grouped, series and charts, positioned, record-defined forms,
    shared filters.** Needs 8, 11.
15. (WG-5481) **Gestures, wizards and pending state.** Needs 10, 11.
16. (WG-5468) **Shell variants, navigation and the notification bell.** Needs 9, 11.
17. (WG-5469) **Tenant layout overlays by id, orphaning and the layout editor.** Needs 9, 11.
18. (WG-5470) **Preview as a set of roles.** Needs 9.
19. (WG-5482) **Provenance and structured diagnostics with the convergence contract.** Needs 2.
20. (WG-5472) **A product-version comparator apart from platform releases, and staged installs by a pure
    planner.** No dependency.
21. (WG-5471) **Agent loop platform: compile, classification and held upgrades, scoped delegation,
    product base and lease, assigned versions, idempotency before version, labels and build log.**
    Needs 8, 9, 19, 20.
22. (WG-5483) **The agent surface: screens, bindings, shapes, policy read, page-model and screenshot
    tool.** Needs 9, 11, 21.
23. (WG-5473) **Gallery, pinned screenshot environment, sampled matrix, stress themes, axe, keyboard,
    baseline discipline.** Needs 5, 11, 12.
24. (WG-5474) **A platform admin role products may name (showcase gap 2, backend half).** No dependency.
25. (WG-5475) **The showcase re-expressed and built by an agent script: the acceptance target.** Needs
    10, 13, 14, 16, 17, 18, 21, 22, 23, 24.
26. (WG-5511) **Task inbox, decision form and the `decide` action.** Needs WG-5496, 11, 9.
27. (WG-5512) **Process timeline pattern.** Needs WG-5507, 11.

## Open questions

- Should a builder be able to preview as a named person, with that person's consent recorded?
- Should a field's existence be hideable from readers of its record type when its read rule is
  per-record?
- Should the platform serve map tiles of its own, so a geographic map can be a renderer extension?
- Should tenants upload fonts, with a server-side font sanitiser?
- Should product segments move inside the schema change, so a step is atomic for its segments too?
- Collaborative editing (a named gap): which shape, if any, should the platform offer?
- Field work offline: should a realm be able to declare a work pack, and should the browser queue file
  content at rest for photos and large uploads (see "Reserved entries")?

## Answers to the reviews

### Enterprise sweep (WG-5484)

| # | Finding | Answer |
|---|---|---|
| 1 | No task inbox or approval pattern | Confirmed (`UiAction` invokes operations only; `approve-workflow-step` is a gateway route the interpreter never calls). Reserved: the `tasks` source, `task-inbox`, `decision-form` and a `decide` action kind, assuming only WG-5496's named contracts |
| 2 | No process timeline | Confirmed (runs and steps are in the job store). Reserved `process-timeline` over WG-5507's projection; nothing renders until it exists |
| 3 | Audience classes under ABAC and time-bound grants | Confirmed that principal or clock conditions make reads individual. The interface stays shareable: class keys hold only deciding groups and builder status; attributes, time and individual grants act only in per-request narrowing; time-bound grant boundaries must raise the access generation (WG-5499) |
| 4 | Teams and invitations change the schema without a version | Confirmed (`put_team`, `put_invitation` write `PutSchema`). The interface caches are keyed by a hash of interface-relevant parts excluding teams; roles are computed per request; team changes raise the access generation; the held digest never contains teams |
| 5 | Money and number over scaled ints | Fixed: exact decimal strings, currency, scale and rounding from the type when it has them, civil dates formatted as themselves, additive accepted types |
| 6 | Field work offline | Named as gaps with the decision each waits for (work pack, photo queue, large-file upload queue) |
| 7 | Acting-as indicator | Reserved `header.actingAs`, filled by the platform, not hideable |

### Third review (four directions)

**Structural change: dormancy by an effective schema.** Confirmed: `schema.policies` is iterated in
more than ten places and `holds_any`/`effective_permissions` have 47 non-test call sites, so "one gate"
was not one place, and `dormant_unreachable` held only because its list was routed through the gate by
construction. Replaced by a draft overlay of the schema that no non-builder evaluation is given, with
the closure invariant enforced by the classifier, restrict references from dormant types refused, draft
quotas, the draft-data right, and the trigger note ("Drafts, published, and dormant entities"); the Lean
now models the structure (`nonbuilder_reach_active`, `dormant_unreachable`, `step_keeps_closed`,
`publish_keeps_closed`, `unclosed_reaches_dormant`; DS-017, DS-018).

| # | Finding | Answer |
|---|---|---|
| V1 | Variant selected on audience alone (compiled) | Confirmed; fixed: the first variant that serves the viewer something (`selectServed_sound`, `selectServed_first`, `variant_falls_through`; DS-014) |
| V2 | Groups, shells and variant order not draftable; admin check not re-run | Fixed: draftable and approval-needed; any group edit re-runs the admin check on every element naming the group, refusing a failure and showing who gains access |
| V3 | A role everyone holds counts as strict | Confirmed (`default_role`, `types.rs:3643`); fixed: `strict` excludes `public` and default roles; admin audiences naming a default role refused (`admin_default_role_refused`); whole-tenant roles warned; `admin_never_to_default_member` (DS-011) |
| V4 | `withheld` reveals optional withholding | Fixed: sent only for required fields |
| V5 | The 2^6 bound contradicts the class definition | Confirmed; fixed: classes are formed from deciding groups only, data audiences applied per viewer to the cached part (D-DS-24), cap stated for that reason |
| — | Plausible items | Adopted: admin by effect (write grants vs least viewer), only `Not` around a role test is negation with a pinned `react_to_message` test, `unknown` disabled for admin actions, nav label of the served variant, no audiences or group names in served parts, owner and manager-staff notes in preview |
| G1 | Publish over 200 changes | Fixed: `PublishDraft(product)` counted as one change, atomic as one schema change, bounded by draft quotas, with a 224-entity fixture; the worked example says so |
| G2 | Dormant isolation | The structural change above |
| G3 | Lease without expiry or durable counter | Fixed: lease in tenant kernel state with expiry, checked in the applying transaction; the model has a clock (`no_step_after_expiry`; DS-015) |
| G4 | Catch-up migration not implementable | Confirmed (`plan_migration` moves in place and loads every record; operations capped at 16 steps). Fixed: typed `CopyField` in batches of 10,000 with copy marks; publish re-runs it and refuses past 10,000 remaining; the digest covers the rule, not counts. The unbounded load in `plan_migration` is noted for WG-5464 |
| — | Plausible items | Adopted: the classifier-in-transaction claim marked unmodelled; comparator agreement and stage-version fixtures; an agent writes only as its person |
| R1 | 426 handling absent | Confirmed (no 426, `Strata-Reload` or reload handling in `web/src` or `crates/bff`). Fixed: arrives with the first shell of this programme; the minimum never exceeds the oldest shell that handles it; `index.html` uncached; a recorded old-shell fixture; older shells get the error state, stated |
| R2 | Pixels-make-major collides with tool bumps | Fixed: exact comparison on the pinned image; tool-bump stacks classified by a calibration set within a stated bound; previous major's DOM and pixels gated; security fixes need a recorded task and identical goldens on both majors |
| R3 | Reversal promise empty | Fixed: a grace period after end of support keeps the old renderer; two-behind path; orphaning and the theme step in the move; at least two warnings with an open record; mail to owners who saw none; inactive owners do not stop the move; preview as member, anonymous and each group |
| R4 | Holds withhold security fixes | Fixed: the audience falls back to the product's base screen and the owner is notified; upgrades are never held for an overlay |
| R5 | Patching stored themes | Fixed: a failing later pair is a non-blocking warning until the owner's next theme step; stored themes change only by a recorded definition step, a new derivation version with an undo; the hash is always of the set in force |
| R6 | Baselines tautology | Fixed: the gate runs new code against old baselines (exactly the listed examples must fail) and against new ones (all pass); `Baselines-Approved` checked against Workgraph; the gate runs on the integration tip after landing |
| — | Plausible items | Adopted: floor notices purge interface parts before the manifest arrives; any role change is narrowing; only overlay entries are orphaned and the model's `orphan` is stated against that; the preview uses the production renderer path |
| L1 | Composite-typed props, handle and focus | Fixed: the list of composite-typed host props; host-injected handle as first focusable, focus model inside items |
| L2 | Operators, plurals, derived fields, nested each | Fixed: open enums for node kinds and `when` operators reserving comparisons and `not`; a `plural` text node by `Intl.PluralRules`; computed conditions belong in derived fields; lanes are the host's, so nesting is lane-then-card |
| L3 | Lanes by reference | Fixed: a reference group field when the product declares lanes as a section read under the viewer's policies, with a catch-all lane; the privacy reason stated |
| L4 | Form builder | Fixed: typed answers per input type from the first version; a `reference` input; reserved `not equals`, `or`, repeating groups; sections as wizard steps; calculation and branching out of scope |
| L5 | Group cap | Fixed: only deciding groups count; a tenant setting under a measured platform maximum; the diagnostic suggests merges |
| L6 | Build-time items | Fixed: rank collision, rebalance and tiebreak; anonymous wizards in memory only; declined gaps' placeholders must be removed or replaced; registry `capabilities` is documentation until the verified-module manifest |
| — | Plausible items | Adopted: image node and icon vocabulary reserved; block interaction through filter state; `select`-first notes in preview; a `reschedule` gesture reserved |

### Second review (four directions, and the owner's decisions)

**Owner decisions recorded**: per-role shells and screen variants, first match wins (D-DS-20; "Layer 4";
`select_sees`, `select_first`, DS-014); audience groups with the cap on distinct groups and how it was
chosen (D-DS-21; "Declaring it"); the rendering-major lifecycle, warn then migrate, with the real-page
preview and the reversible automatic move (D-DS-22; "Versioning").

**The architecture smell review**: decided for explicit, owner-intended audiences with the derivation
as a lint (D-DS-23; "Who a screen is for: the architecture decision"). `ui.audience.admin-for-members`
refuses admin elements for members or the public by construction; own scope is derived from a principal
mention and no longer a field. Interpretation, stated: "any widget with a write action" is read as an
**administrative** action (invokers that name roles, role-testing or negating conditions, or changes to
roles, permissions, settings or policies); read literally it would forbid members from posting a message,
which the showcase needs.

| # | Finding | Answer |
|---|---|---|
| L1 | Composite body language undefined | Fixed: a static tree, decided now; node kinds, path-only slot references, `when` and `each` with bounds, acyclic references, 200 expanded nodes, derived states, no gesture hosts or landmarks, relative headings, accessible-structure refusals |
| L2 | Registry and gaps | Fixed: registry entries as data with module hash and capabilities; gap lifecycle with `resolvedBy` and `duplicateOf` |
| L3 | Gestures and wizards | Fixed: pure projections with examples; `move` with group and rank in one operation; a lexicographic rank scheme; inline-edit inputs per field type; wizard state home, lifetime, 64 KB bound, typed-text rule, step outputs carried forward; collaborative editing a named gap |
| L4 | Binding server contracts | Fixed: the contract table (where work happens, bounds, windows, time zone, week start, lane paging, closed groups only, plane images from file fields, chart table from the same buckets); the form-builder definition shape with sections, closed validation and conditional visibility |
| L5 | Sample vocabulary too closed | Fixed: tenant vocabulary, free text of at most 32 characters flagged as sample, personal-data refusal |
| L6 | Composite templates | Fixed: saved layouts with region constraints; the depth limit counts the expanded tree |
| R1 | Self-labelled majors | Fixed: majors decided by the gate (non-allowlisted classes, structure, pixels); previous major's goldens gated; security fixes to both; the preview route; lifecycle per D-DS-22 |
| R2 | Lean versus spec | Confirmed and fixed: orphan theorems carry `Verifies: DS-013`; orphaning now filters by the per-widget check under a new catalogue and schema (`orphan_valid` for any `cat'`, `has'`); DS-013 says other overlay operations are not modelled; the doc no longer overstates |
| R3 | Silent orphaning | Fixed: per-tenant orphan report in the result and dry run; owner and overlay author notified; an upgrade that takes a screen from an audience is held for that tenant; automatic drops recorded and restorable |
| R4 | Stored themes and later tokens | Fixed: per derivation version defaults for later tokens; new tokens and pairs checked over the corpus and every stored theme |
| R5 | Baseline trailer gameable | Fixed: checked at the stack tip; listed example ids must equal the changed set; configuration as baseline-class files; a size cap needing a recorded approval; tool bumps in their own stack |
| R6 | Atomic swap can keep revoked content | Confirmed and fixed: a narrower class switches at once with loading for unheld parts; a deadline to an error state; view replies by shape in the atomic unit; `426` with `Strata-Reload`; corpus contents stated honestly (no exports until an exporter and a privacy review); the shape fingerprint defined |
| V1 | Admin action passes | Confirmed; fixed by the architecture decision (`admin_action_for_members_refused`) |
| V2 | `ownScope` asserted | Confirmed; fixed: derived from a principal mention, incompatible with admin |
| V3 | Filters and read rules unscanned | Confirmed; no longer a safety question (derived audiences only narrow); the lint scans them |
| V4 | "Restricted" reveals a processing restriction | Confirmed (`subject_rights.rs::hide`); fixed: the server marks `withheld` fields; absence is never interpreted |
| V5 | Dormancy absent and incomplete | Confirmed (no dormancy in the kernel). Fixed in design: every `TenantSchema` kind named, dormant or needs approval; the choke points named; one gate; a contract-enumerating test; `dormant_unreachable` models the gate |
| — | Plausible items | Adopted: needs-content containers (`static_does_not_keep_region`); link-list entries by target audience; the owner implicitly in every group; preview without data reads shows structure; `ui-build` a capability of its own with no data reads; automation principals refused, agent principals as their person narrowed by delegation |
| G1 | `release_cmp` shared with platform ranges | Confirmed (`products.rs:42`, `product.rs:1519-1523`). Fixed: a separate product-version comparator; platform comparison unchanged; a fixture that the showcase installs on `0.2-draft` |
| G2 | Dormancy choke point | As V5 |
| G3 | Classifier and approval not bound | Confirmed (`plan_upgrade` diffs at run time). Fixed: held with diff, projection, classification and digest; recomputed at approval and refused if different; classifier inside the apply transaction; dry runs never trusted (`approve_applies_held`, DS-016) |
| G4 | Dormant steps not inert | Fixed: dormant operations may write only dormant types and send nothing; draft-created records marked; keep-or-purge at activation; every interface kind has a draft body; the approval shows activation's reach |
| G5 | Lease | Fixed: expiry, renewal, owner break, fencing token on every step (`no_stale_step_after_acquire`, DS-015); held upgrades bound to their diff survive a lease change; orphaning steps held and notified; overlays apply in draft preview. Segments outside the commit: still open |
| G6 | Multi-entity provenance | Fixed: `related`, `clears` sequences, cross-entity fixtures, restructures exempt from the loop budget |
| G7 | Restructure loses writes | Fixed: publish carries a catch-up migration in the same change; the projection shows the unmigrated count |
| Lean | Loop unmodelled | Modelled the cheap guards (lease, held approval, dormancy gate) in `Loop.lean`; the rest is listed under "not proved" |

### First review (four directions)

Each finding was checked against the code or the models before acting.

**A. Lock-in and extensibility**

| # | Finding | Answer |
|---|---|---|
| A1 | The only extension path is a platform change | Fixed: "Extending the catalogue": composite patterns and templates as product data with one generic renderer, platform blocks, a renderer registry for platform-reviewed extensions, catalogue gaps (`POST /api/ui/gaps`) with the placeholder kept; D-DS-7; WG-5479 |
| A2 | Bindings closed and CRUD-shaped | Fixed: `grouped`, `series`, `positioned`, `record-form` bindings; `board`, `chart`, `time-grid`, `plane`, `stream`, `dynamic-form`, `filter-bar`; shared filter state; `bottom-tab` shell and sheets for phone-first; D-DS-17; WG-5480. Open, stated: a geographic map (no tile source of the platform's own) and presence (no server feature) |
| A3 | No interaction vocabulary | Fixed: typed gestures with keyboard and non-drag paths, wizards with kernel step state, pending writes as the only optimistic state; "Interaction"; WG-5481 |
| A4 | One information architecture | Fixed: nestable layout containers, shell variants (`top-nav`, `rail`, `bottom-tab`, `full-bleed`, `none`), more templates and composite templates. Disputed in part: alternative templates per audience are not taken, with the reason in "Layer 4" |
| A5 | No i18n or RTL | Fixed: logical properties only (in WG-5458's criteria now), `dir` and `lang` on the root, a `text` type for every string prop from the first catalogue version, RTL and Japanese in the matrix; WG-5477 |
| A6 | Accessibility beyond contrast | Fixed: stable keys and focus restoration on redraw, announcements that never move focus, forced colours, the 24-pixel target floor at every density (44 in touch shells), reflow at 320 pixels, safe areas |
| A7 | Closed font list; unbounded classes | Fixed in part: fonts are a catalogue section grown by platform review, with script coverage; tenant font uploads are an open question (a font file is a parser input). Classes bounded: at most 32 roles named in audiences, class cache bounded by entries and bytes |

**B. Regressions on update**

| # | Finding | Answer |
|---|---|---|
| B1 | Rendering unversioned | Fixed: a rendering contract version per catalogue major with golden page models and DOM; compositions record the catalogue hash and contract version; restyles are majors that a tenant opts into; aliases for one major; `catalogue-compat.mjs` limits stated; WG-5478 |
| B2 | Derived themes recomputed silently | Fixed: derivation version, stored derived set and hash, gate re-derivation over a corpus of seeds; 3 to 1 non-text pairs were already listed and now include control borders (WG-5458, WG-5460) |
| B3 | The overlay guard is one-dimensional, DS-004 overclaims | Fixed: ids on widgets, containers and regions; shape fingerprints in bindings; overlays rechecked whenever their base changes; orphaning instead of refusal, proved (`orphan_valid`, then `orphan_removes_only_dangling`, renamed `orphan_removes_only_failing` in round 2, DS-013); precedence stated; `incremental_valid` now takes a changing catalogue; DS-004 reworded and its rationale no longer claims overlays cannot break |
| B4 | Weak governance | Fixed: pinned container, `system-ui` mapped for tests, the matrix as a stated pair table, generated stress themes, corpus compositions in the gallery, baseline commits separated and trailer-checked (review is stated as a repository rule a tool cannot enforce) |
| B5 | No corpus replay | Fixed: `tests/corpus/ui/` and page-model golden diffs gating interpreter, formatter and derivation changes |
| B6 | Version skew | Fixed: contract versions in manifest, shell and parts; mismatch reloads; atomic manifest swap (`acceptManifest` changed); minimum client version for reads |

**C. Visibility and roles**

| # | Finding | Answer |
|---|---|---|
| C1 | Empty region served (compiled counterexample) | Confirmed and fixed: containers and elements are separate in the model, empty containers are pruned, the content check recurses to counting leaves; `served_has_element` (DS-012), `hidden_leaves_no_trace_in`, and the counterexample is `empty_region_pruned` |
| C2 | Conditional grants defeat the content check | Confirmed and fixed: the check uses content audiences (unconditional grants, or any grant for an own-scoped widget); screens with write actions or admin-marked widgets must declare their audience; `conditional_grant_does_not_count` was the review's case. Superseded in round 2: the content check became a lint and the guarantee moved to the admin check (D-DS-23) |
| C3 | `dataAud` cannot express public (compiled) | Confirmed and fixed: every viewer, anonymous included, holds `public` (`rolesOf`); `public_grant_anon_sees` |
| C4 | Schema version and the global errors part leak | Confirmed (`Manifest.schema_version`, one `errors` part in `substrate/src/ui.rs`). Fixed: class content hash for non-builders, notices only on a class's manifest change, draft steps notify builders only, `errors` filtered and hashed per class |
| C5 | Class from declared audiences only | Fixed: the class is computed by `audience.rs` over grant, policy, `has_role` and invoker roles, the owner, declared roles and builder status, per screen |
| C6 | Individual classes unbounded | Fixed: parts are role-only; principal- or clock-dependent presence shows an empty state; individual classes never enter the part cache |
| C7 | `not has_role` breaks monotonicity | Agreed on the problem; fixed without restricting the policy language: any condition makes a grant conditional, conditional grants count only for own-scoped widgets, declared audiences have no negation, so every audience the check uses is monotone (`sees_mono`). Automation principals draw no interface and are not viewers |
| C8 | Preflight cannot equal invoke; bulk oracle | Agreed: the preflight is advisory, answers `unknown` for missing inputs, for conditions that read what the viewer may not read (evaluated under the viewer's read rights), and past a per-session budget; the invocation decides and a refusal refreshes the affordance; the differential claims only that `allowed` comes from checks the invocation runs |
| C9 | "—" gives away withheld required fields | Agreed: a required field's withheld value is drawn as a restricted mark and that disclosure is stated, or the tenant uses a role-only rule |
| — | Plausible items | Adopted: dormant entities until publish (operations, policies, object types); view sections filtered per class; preview intersected per row and field; `ui-build` reads policies and invokers; the class test before per-class work; the anonymous manifest revalidated |

**D. Agent build loop**

| # | Finding | Answer |
|---|---|---|
| D1 | Version rules contradict `plan_upgrade` and `release_parts` | Confirmed (`product.rs:1930`, `:1488`). Fixed: platform-assigned `next` versions, semantic-versioning pre-release ordering in `release_cmp`, idempotency key matched before the version check, undo as a new version carrying old content; WG-5471, WG-5472 |
| D2 | The risk gate does not exist on the agent's path | Confirmed (`precheck.rs`, CT-112's change set, `foreign_change`). Fixed: the upgrade classifies its own diff and holds what needs approval as a pending upgrade; the list is in D-DS-14; product-scoped delegation; WG-5471 |
| D3 | `products::write` requires owner and step-up | Confirmed (`products.rs:316-334`). Fixed: the authority table in "Classification and authority": a scoped delegation applies interface and dormant steps; everything else is a held upgrade the owner approves with step-up |
| D4 | Diagnostics are not as described | Confirmed (`bundle_error`, string refusals, generic conflict). Fixed: specc's provenance table, kernel codes mapped through it, the convergence contract (`clears` per code with negative fixtures), the loop budget and repeated-diagnostic stop; WG-5482 |
| D5 | Splitting at the cap | Fixed: `plan_install` specified as a pure function over the reference graph's strongly connected components, with the oversized-component refusal, no persistence, internal stage records; WG-5472 |
| D6 | Sessions are not atomic; restructures blocked | Fixed: dormant entities, `discard-draft`, "data that was activated is never rolled back", the restructure sequence, type changes through a new field. Segments outside the commit stated as an open question |
| D7 | Concurrency | Fixed: per-product base (installed version and draft revision) in place of the tenant-wide expected version, a build lease per product, drafts covered by the guard, tenants change product screens only through overlays |
| D8 | Watching and placeholders | Fixed: draft steps notify builders only; publish refuses any placeholder (the model's `Publishable`); sample labels from a closed vocabulary; binding kinds checked in the model (`accepts`). The protocol document's notice kinds are corrected by WG-5435, as docs/frontend.md already records |
| D9 | Missing agent surface | Fixed: screens, bindings, shapes, policy read, compile, source, gaps and a page-model and screenshot tool (the headless runner of D-FE-10) are deliverables of WG-5483 and WG-5471 |
| Lean | Models too narrow | Extended: binding kinds, static and placeholder, ids, publishability, a changing catalogue and orphaning (Composition); containers and pruning, the public role, conditional grants and content audiences (Visibility). Narrowed in wording where still not covered: "What is proved", "not proved" |

**What held up** is kept as it was: native-first primitives in the light DOM (now stated as a
deliberate one-way trade), visibility served from the server and the not-found equivalence,
`audOf_sees`, overlays over product screens, typed tokens through the CSSOM, fallbacks that never show
ids, semantic-versioning discipline, each step one atomic schema change, the expected-version check
failing safe, notices carrying no record data.
