# The design system and agentic composition

How a Strata tenant's screens are made, and made by agents: the design tokens and themes, the
primitives, the patterns and how products extend them, the app shell and templates, the contract
that turns data into interface, interaction, wireframes, who sees what (public and private, role by
role), the layouts a tenant arranges, how all of it is versioned, and the loop in which an agent
builds a tenant's screens while the tenant watches. This is a design. Its requirements are in
`specifications/design-system.jsonc` (DS-001 to DS-013, D-DS-1 to D-DS-19), the properties that can
be stated precisely are proved in Lean (`lean/Strata/UI/`), and the work is listed at the end in
dependency order, each item a Workgraph task under WG-5457. It was revised after one adversarial
review; the answers are in the last section, and everything above it is the current design.

It builds on the browser client design (`docs/frontend.md`, WG-5415) and changes none of its
decisions: the kernel stays pure, custom elements draw and the kernel decides (D-FE-4), no third-party
code reaches the browser (D-FE-9), the browser's copies keep their rules (D-FE-1, D-FE-2, D-FE-11).
It amends one copy class (`copy-browser-interface`, see "Serving and caching") and one behaviour of
`acceptManifest` (see "Version skew").

## The recommendation in one page

Six decisions are the owner's (2026-11-22) and are recorded as decided: a neutral base with tenant
theming through tokens, Fieldnotes becoming one theme (D-DS-1); layouts the tenant arranges, as data
(D-DS-2); agentic first (D-DS-3); wireframing as a first-class concept (D-DS-4); public and private
for every element, with the rule that the interface never discloses what the data layer would refuse
(D-DS-5); and role-based views (D-DS-6). Every other D-DS-* is a proposal of this design.

- **One catalogue, three tiers of extension** (D-DS-7). `specifications/ui-catalogue.jsonc` lists
  every token, primitive, layout container, pattern, template, shell variant, binding kind, gesture
  and display type with a JSON Schema and examples. A product extends it without code by
  **composite patterns** (typed slots wiring primitives and layout containers to a section shape);
  the platform extends it with **blocks** and, for kinds of drawing no composition can express, with
  **platform-reviewed renderer extensions**; an agent that finds nothing fitting files a **catalogue
  gap** and keeps a placeholder. No tenant or product JavaScript at any tier.
- **Tokens and themes** (D-DS-15). Components read semantic tokens only, written with logical
  properties. A theme is token values: a preset gives them all (Fieldnotes), a tenant gives typed seeds
  from which the rest is derived once, stored with its derivation version and hash, and checked for
  contrast (4.5 to 1 text, 3 to 1 non-text) when defined. Values reach the page through the CSSOM.
- **Primitives are mostly native elements** (D-DS-8), in the light DOM, by a deliberate one-way
  choice. Custom elements only where behaviour has no native form.
- **Screens are compositions** (D-DS-10): a template, regions, nestable layout containers, and
  widgets bound to sections, grouped sections, series, positioned sections, forms, record-defined
  forms, actions or platform sources (D-DS-17). The kernel checks every composition, including each
  binding's kind against its pattern (`check_iff`).
- **Interaction is typed** (D-DS-17): reorder, move between groups, inline edit and select-many are
  gestures bound to operations, each with a keyboard path; wizards keep their step in the kernel.
- **Display types end raw values** (D-DS-9), with people and reference titles resolved by the server.
- **Drafts and published** (D-DS-10). Builders alone see drafts and wireframes; publishing is refused
  while any placeholder remains (`publishable_no_placeholder`); what a draft step creates is dormant
  until published.
- **Visibility is served, not hidden** (D-DS-11). Declared audiences, data and content audiences
  derived from policies and invokers, filtering per class by the server, containers pruned when empty
  (`served_has_element`), nothing left as a trace (`hidden_leaves_no_trace`), no build activity
  visible to non-builders.
- **Roles vary one composition** (D-DS-12). The content check makes an admin panel undeclarable for
  members (`checked_screen_has_content`); row affordances come from a kernel preflight and are
  advisory.
- **Tenant layouts are an overlay** (D-DS-13), addressed by stable ids, orphaned rather than blocking
  an upgrade (`orphan_valid`).
- **The rendering contract is versioned** apart from the schemas, and version skew forces a reload
  (D-DS-19).
- **International and accessible by rule** (D-DS-18).
- **The agent loop** (D-DS-14): classified product steps into the draft, a per-product base version
  and build lease, interface-only steps for a scoped agent delegation, everything else held for the
  owner's approval, diagnostics that point into the product file and name the edit that clears them.
- **Large installs are staged** by a pure planner over the dependency graph (D-DS-16).

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

**Shell variants** are catalogue entries; the tenant chooses one, a product never does:

| Variant | Regions | For |
|---|---|---|
| `top-nav` | `header` (slots `brand`, `nav`, `search`, `actions`, `account`), `main`, `aside`, `footer` | the default |
| `rail` | a vertical icon rail for primary navigation, `header`, `main`, `aside` | tools with many screens |
| `bottom-tab` | `header` (compact), `main`, a bottom tab bar of at most five entries, sheets for secondary navigation; `env(safe-area-inset-*)` respected, 44-pixel targets | phone-first products |
| `full-bleed` | `main` only, with a floating account control | dashboards, kiosks, media |
| `none` | `main` only | embedded and print views |

Each variant states its responsive behaviour (under its container width a `top-nav` collapses into a
menu button; `aside` moves below `main`). Products contribute **navigation entries** (`navigation`
section: screen, label, group, order, icon); the tenant's overlay orders and hides them; the
`notification-bell` sits in `header.actions` (or the tab bar), bound to the platform inbox
(`/api/inbox/count` and `list`, CT-821 to CT-826).

**Templates** arrange a screen's main area: `list`, `detail`, `dashboard`, `settings`, `feed`,
`editor`, `board`, `calendar`, `chat` (a `stream` region with a pinned `dock` for the composer) and
`canvas` (one region, a grid of layout containers, for anything else). The catalogue says which
patterns each region allows and how many; the kernel enforces it.

**Layout containers** make a region a tree, not a list. A `layout` widget has `kind` (`stack`,
`cluster`, `grid`, `split`, `switcher`, `sidebar`), typed parameters (gap from the space scale,
columns, minimum item width, split ratio, collapse width) and children that are widgets or layout
containers, nested to a depth of four. A layout container binds nothing, has an id and an audience
like any widget, and is pruned when nothing in it is served (see "Visibility").

**Extending templates.** A template is a named set of regions with constraints. The platform adds
templates in a catalogue minor version. A product may declare a **composite template**: a named
`canvas` arrangement of regions as layout containers, checked like any composition, reusable by its
screens. Alternative templates per audience within one composition were weighed and not taken: one
screen serving different region sets to different roles needs a precedence rule and a content check
per variant; layout containers with audiences give the same result with one tree.

Today's four layouts are templates of the same names, and a screen that declares only `kind` (every
installed product today) is read as that template with one widget per view section in name order.

## Layer 5: the data-to-UI contract

### Display types

A display type maps a value to a rendering. The catalogue's `displayTypes` section lists, for each,
the field types it applies to, its props, and what the server must provide.

| Display type | Field types | Rendering | Server provides |
|---|---|---|---|
| `text` | str | the text; `multiline` keeps line breaks | |
| `rich-text` | str with `markdown` in the view section | prose blocks | `blocks` (exists) |
| `datetime` | time | `<time datetime>`, `relative` ("3 hours ago", absolute in the tooltip) or `absolute` ("22 Nov 2026, 14:05"), in the request language and the realm's time zone; the kernel relabels relative times on its heartbeat timer | |
| `date` | time | the day only | |
| `duration` | int (seconds) | "2 h 5 min" | |
| `count` | int | grouped digits, or compact ("1.2k") | |
| `number` | int | grouped digits with a declared scale (an int of hundredths shown as 12.50) | |
| `money` | int (minor units) | `Intl.NumberFormat` currency; the currency is a prop or names a sibling field | |
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
                             "actions": [ { "action": "showcase_community__hide", "whenRefused": "hide" } ] } } ],
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
  passes the content check (see "Role-based views").

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

1. **Platform patterns and templates** (above), each a renderer in the web application.
2. **Composite patterns, defined by a product as data.** A product's `composites` section declares a
   pattern: its name (in the product's namespace), the binding kind it takes (as `binds`), typed
   **slots** (a slot is a field of the bound shape with a required display type, or a nested
   composite, or an action), and a **body**: a tree of layout containers and primitives whose text and
   values come from slots. One generic renderer in the platform draws every composite from its body,
   with the same states as a platform pattern. The kernel checks a composite like a screen (every
   primitive and layout container is in the catalogue, every slot reference resolves, the depth limit
   holds) and checks each use against its slots. Example: a product's "listing card" (image slot,
   price slot as `money`, title slot, a "save" action) used in a `record-list` of listings.
3. **Platform blocks**: composites the platform ships in the catalogue (a pricing table, a hero, a
   media gallery, an FAQ), so a marketing or media tenant composes from tested parts.
4. **Platform-reviewed renderer extensions**, for a drawing no composition expresses (a new chart
   kind, a geographic map once there is a tile source of the platform's own). An extension is a
   renderer module in the web application's own repository, reviewed like any platform change,
   registered in the **renderer registry** under a catalogue entry, shipped in a platform release as a
   catalogue minor version. It follows every rule of primitives (light DOM, DOM APIs only, tokens only,
   keyboard-complete, forced colours, reflow) and is in the gallery and the screenshot matrix from its
   first release. The registry is a table in the web application, not a loading mechanism: there is
   no way to register a renderer at run time.

**Catalogue gaps.** When an agent needs what no tier offers, `POST /api/ui/gaps` records a gap: the
tenant, the requested behaviour in one paragraph, the binding kind and shape it would take, and the
screen and widget id where the agent placed a placeholder instead. The draft keeps the placeholder
(so the step is valid and the rest of the work goes on); the gap is visible to the tenant's builders
and, without the tenant's data, to the platform's operators, who decide whether it becomes a block or
an extension. `ui.catalogue.unknown` therefore never ends a session: its diagnostic hint names the
nearest entries and the gap route.

## Structured bindings

A binding names what a widget shows. The kinds, each with a shape the server computes and serves:

| Kind | Names | Shape | Patterns |
|---|---|---|---|
| `section` | a section of the screen's view | object type, fields with type and display type | most |
| `grouped` | a list section and a field of it with a closed set of values (a choice, a workflow state) | the section's shape plus the ordered groups | `board`, grouped `data-table` |
| `series` | an aggregate of a named query bucketed by a time field or grouped by a field | x (time bucket or group), y (a count or sum), optional series field | `chart`, `stat-tile` |
| `positioned` | a list section and its position fields (start and end times, or x and y numbers) | the section's shape plus the position fields | `time-grid`, `plane` |
| `form` | a form of the screen | fields, inputs, the operation | `form`, `wizard` steps |
| `record-form` | a single section whose records are form definitions, and a list section of their field records | a fixed definition shape the catalogue states (label, input type, required, choices, order) | `dynamic-form` |
| `action` | an action of the screen | the operation's inputs | buttons, gestures |
| `source` | a platform source: `inbox`, `navigation` | fixed | `notification-*`, `nav-menu` |
| `static` | nothing | none | `markdown-block`, `link-list`, layout containers |
| `placeholder` | nothing yet; a sample shape | the sample | any pattern that binds data, in drafts |

**Shared filter state.** A screen declares `filters`: named parameters with a type and an input
control. A `filter-bar` sets them; they are kernel state, carried in the address's query, and each
section that takes a query parameter of the same name (`ArgSource` of the view, as `query` on screens
today) is read again when one changes. So a dashboard's date range and team filter drive every chart
and table on it at once.

**Record-defined forms.** A `dynamic-form` draws the fields its definition records describe, from the
closed set of input types the catalogue states, and submits the answers to one operation of the
product whose inputs are the definition's id and a list of answers (strings in field order, each
validated by the operation against its field record). The form's structure is data the tenant edits
as records; the operation and its checks are the product's, so a form builder needs no schema change
per form.

## Interaction

**Gestures** are catalogue entries, each bound to an operation through typed arguments, each with a
keyboard path and a single-pointer, non-drag path (WCAG 2.5.7):

| Gesture | Arguments it supplies | Keyboard path |
|---|---|---|
| `reorder` | `row.id`, `target.before` or `target.after` (the neighbour's id) | Space to pick up, arrows to move, Space to drop, Escape to cancel; or "Move up/down" in the row's menu |
| `move` (between groups) | `row.id`, `target.group` (the group value) | the same, across lanes; or "Move to…" with a list of groups |
| `inline-edit` | `row.id`, `field.<name>` (the new value) | Enter to edit, Enter to save, Escape to cancel |
| `select-many` | `rows.ids` (a list) to a bulk action | Space toggles, Shift+arrows extend, Ctrl+A all on the page |

The kernel checks that the operation takes each argument with a compatible type (a `ref` to the
section's object for `row.id`, the group field's type for `target.group`). A gesture whose operation
the viewer may not invoke is absent; the row affordance (see "Role-based views") disables it per row.

**Wizards.** A `wizard` holds its steps (each a form or a region), validates a step's fields on
"Next" with the client checks, keeps the current step and the values in the kernel (so a reload or a
second tab resumes it, and the address names the step), and submits once, to one operation, at the
end; a step that needs a server decision is its own operation and its own form.

**Optimistic and live state.** The only home of optimistic state is the kernel's pending writes
(docs/frontend.md, D-FE-5): a gesture's effect is drawn from the pending write over the view result,
marked as pending, until the receipt or the refusal arrives; a refusal restores the view result and
shows why. Live state is the view result plus the notices that make it be read again (live bindings);
a `stream` appends rows from `upsert` notices by reading the section again from the last row it holds.
Nothing else in the browser holds state that is not the server's or the person's own typed input.

## Wireframes and fidelity

A wireframe is the same composition with placeholders, drawn at once in the neutral base theme or the
tenant's: labelled regions, the pattern's skeleton, the sample's labels and display shapes, and each
widget's note.

**Samples hold no data.** A sample is a single-or-list flag, a row count from 1 to 12, and fields,
each a display type and a label. Labels come from a closed vocabulary the catalogue lists ("Title",
"Name", "Date", "Amount", "Status", "Author", "Description", ... in each supported language) or are
the label of a field of the schema; free text is refused (`ui.sample.label`), so an agent cannot put
a realistic name, address or message into a draft. Placeholders are drawn only to builders.

**Valid at each step, mechanically.** A composition is **valid** when widget ids are distinct, every
widget is allowed where it is placed, a static pattern is bound `static`, and a data pattern is a
placeholder or is bound to a binding its pattern accepts (the binding's kind against the pattern's
`binds`, the field names its props read against the shape) and the schema defines
(`Composition.Valid`). It is **publishable** when it is valid and has no placeholder at all, whatever
the pattern (`Composition.Publishable`). The kernel runs the check on every change and refuses an
invalid result, so every state a builder can see passed it. Proved:

- the check is exactly validity, and a checked composition binds only to bindings that exist and its
  patterns accept (`check_iff`, `checked_bindings_resolve`); a publishable composition has no
  placeholder and every data widget bound (`publishable_no_placeholder`, `publishable_bound`);
- binding a placeholder keeps validity and leaves one placeholder fewer (`bindAt_valid`,
  `bindAt_placeholders`); refining one widget at a time is valid at every step and ends publishable
  (`refine_valid`, `refine_publishable`);
- a change of schema *and catalogue* that removes no binding an unchanged screen names and alters no
  catalogue answer it relies on, with every changed screen checked against the new schema and
  catalogue, leaves every screen valid (`incremental_valid`); without the removal guard a valid screen
  breaks (`unguarded_removal_breaks`).

**Shapes are part of a binding.** In the implementation a binding is the name of what it binds
together with a **shape fingerprint**: for a section, the object type and the type and display type of
each field the widget reads; for a form, the operation's input types. A field whose type changes, a
section whose query moves to another object type, an operation whose input changes type: each is a
binding that stops existing and another that starts, so the removal guard and the incremental rule
cover shape changes as well as removals. The models abstract a binding (`B`); the fingerprint is how
the implementation instantiates it.

**What the models do not cover**: the content of props beyond what `accepts` decides, nested layout
containers (the composition model's widgets are a flat list per screen; the visibility model has the
tree), and the meaning of a catalogue change beyond the answers a screen relies on.

## Visibility: public and private

### Declaring it

Every element declares an **audience**: `"public"` (anyone, signed in or not), `"members"` (every
signed-in person of the tenant), or `{ "roles": [...] }` (the holders of any of the roles). An absent
audience means the parent's.

| Level | Where declared | Default |
|---|---|---|
| app (the shell) | the shell element | `members` |
| navigation entry | its entry | its screen's audience |
| screen | the screen (replaces `auth`: `anyone` is `public`, `session` is `members`) | none: required when the screen has a write action or an admin-marked widget; otherwise the app's |
| region | `regionAudience` of the screen or overlay | the screen's |
| layout container | the container | its parent's |
| widget (a pattern instance) | the widget | its parent's |
| field rendering, column | the widget's props | the widget's |
| action, gesture | its placement in props | the widget's |
| placeholder | as a widget; served only in drafts | |

**Composition.** A viewer is served an element only when the viewer is in its declared audience, in
the declared audience of every container above it, and in its **data audience**; a container is served
only when at least one element below it is served. The data audience is never written; the kernel
derives it:

- for a widget bound to a section, grouped section, series or positioned section: the roles holding a
  Read policy (with or without a condition) or a Read permission on the object type, and the owner.
  Every principal holds the role `public` (`roles_of`), so a Read grant to `public` makes the widget's
  data audience everyone, anonymous visitors included (`public_grant_anon_sees`);
- for a column or field rendering: the widget's, narrowed by a field read rule that depends on roles
  only; a rule that reads the record, the principal or the clock is decided per row (below);
- for an action or gesture: the invokers of its operation (`audOf_sees`);
- for a navigation entry: its screen's effective audience; for the platform inbox: `members`.

Each element bound to data also has a **content audience** (used by the content check, below): the
roles with an **unconditional** Read grant, or, when the widget is marked `ownScope` (it shows each
person their own rows: "my notifications"), every role with a grant (`contentAud_sub`: content is
inside data).

A child may not declare an audience wider than its parent's (`ui.audience.wider-than-parent`).

### The rule: nothing the data layer would refuse

For a viewer outside an element's effective audience:

- the element is not in the manifest, the part, or the view reply the viewer is served: neither its
  label, its placeholder, its skeleton, its props, its section's rows nor its existence reaches the
  browser (the gateway leaves out of a screen's view reply every section no served widget binds);
- a container left with nothing (a region, a layout container, a navigation group) is not served at
  all: no heading, no box, no "nothing here" (`served_has_element`); the empty-region counterexample
  of the first review is pruned (`empty_region_pruned`);
- no count says how many elements, rows or fields were left out; a skeleton's row count is fixed by
  the pattern;
- a screen the viewer is not served answers its address with the same not-found page, status and
  text as an address nothing answers;
- a column whose field the viewer's roles can never read is not in the viewer's shape;
- a value a per-record read rule withholds is drawn as an absent value ("—") when the field is
  optional, so withheld and empty cannot be told apart; when the field is required (so "—" could only
  mean withheld), it is drawn as a **restricted mark** ("Restricted"), and that disclosure (that a
  rule withheld it) is accepted and stated; a tenant that must not disclose it uses a role-only rule,
  which removes the column;
- aggregates count only records the reader may read (D-5061);
- an action the viewer may not invoke is absent.

Proved of the model in which the server serves elements the viewer sees inside containers the viewer
sees, and prunes empty containers: every label served comes from such a node (`render_sound`), every
container served holds a served element (`served_has_element`), and a node the viewer is not served
changes nothing the viewer is served, wherever it is among its siblings, including the container
around it, which is served or pruned exactly as if the node were absent (`hidden_leaves_no_trace`,
`hidden_leaves_no_trace_in`).

**What is disclosed, stated.** A field's existence is disclosed to readers of its record type when its
read rule is per-record; a required field's restriction is disclosed by the restricted mark; the
structure of a screen a viewer is served is disclosed to that viewer.

### Drafts, published, and dormant entities

A screen holds a **published** body and, while it is built or changed, a **draft** body. The draft's
audience is the **builders**: the holders of the capability `ui-build` (the owner holds it; a tenant
admin role may be granted it; an agent acting for a builder carries it in its delegation). Holding
`ui-build` also lets a builder read the tenant schema's policies and invokers, which composing needs.

- A member or the public is never served a draft, its placeholders, its notes, or the fact that one
  exists, and is not notified of a draft step (see "Serving and caching").
- Builders see the draft in **preview mode** and can switch to the published body or preview as roles.
- Publishing replaces the published bodies of a product's drafts in one schema change, and is refused
  unless each draft is valid with no placeholder and passes the content check.
- **Dormant entities.** An entity a draft step creates (an object type, a query, a view, an operation,
  a policy) is created **dormant**: an operation is invocable by builders only, a policy grants only to
  builders, an object type's records are readable only by builders, whatever the definitions say,
  until the publication that activates them (in the same schema change as the screens). So a draft
  step adds no entry point for API callers. A change to an existing, active entity cannot be dormant;
  it is a data step classed as needing approval (see "The agent build loop").
- **Abandoning.** `discard-draft` removes a product's draft bodies and its dormant entities in one
  schema change; dormant object types with records are kept and listed, and are removed only with the
  owner's confirmation. Drafts untouched past a tenant setting are listed to the owner for discarding,
  never discarded unasked. Data that was activated is never rolled back by discarding a draft.
- **The removal guard covers drafts**: a draft body is a screen to the incremental rule like a
  published one.

### Relation to today's auth and to policies

`auth: anyone | session` becomes `public | members`; the role variant is what showcase gap 2 asked for,
generalised. Audiences narrow what the interface offers; they never grant data. A widget a viewer is
served still shows only the rows and fields the kernel's reads allow; an action is still decided when
invoked.

### Serving and caching

- **The interface class.** A viewer's class for a screen is computed by the classifier of
  `crates/kernel/src/audience.rs` over everything the screen's elements read and offer: the roles that
  hold a Read grant or policy on a carried object type, the roles a condition tests with `has_role`,
  the roles of the invokers of its actions, the owner, the roles any audience of the screen declares,
  and whether the viewer is a builder. So an editor holding a Read grant and a plain member get
  different classes when the screen shows them different elements, and the same class when it does not.
  The membership test runs before any per-class work, and its cost does not depend on the class.
- **Parts are role-only.** What a part holds depends only on the class. An element whose presence
  would depend on the principal or the clock (a condition that mentions either) is present for every
  member of its role class and shows its empty state where the reads return nothing; an individual
  class never enters the part cache (`AudienceClass::key` is `None` for it, and parts are keyed the
  same way).
- **Per class, by the server.** The gateway computes the manifest and parts per class
  (`strata_substrate::ui`, pure), keeps them in memory keyed by the schema hash and the class, and
  names each part by the hash of its filtered content.
- **What non-builders see of change.** The manifest a non-builder class is served carries a **class
  content hash** in place of the schema version (`Manifest.schema_version` in `substrate/src/ui.rs`
  becomes builder-only), so a draft step, a dormant entity or a change to another class's screens
  changes nothing such a viewer can observe. Schema notices go to a connection only when the manifest
  hash of its class changed; a draft step notifies builders only.
- **Error messages per class.** The `errors` part is filtered per class to the messages that apply to
  forms and actions the class is served, plus the messages for all, and hashed per class, so an
  admin-only operation's wording does not reach members.
- **A part is served only to a class whose manifest lists it**; any other hash is answered exactly as
  an unknown hash.
- **Anonymous visitors.** The anonymous class's parts are public and immutable (`Cache-Control:
  public, immutable`), may be cached by the edge and kept in the shared Cache API; its manifest is
  `public, max-age=0, must-revalidate` with the class content hash as its entity tag.
- **Everyone else.** Manifests and parts of any other class are `no-store`, kept only in the
  principal's encrypted partition (D-FE-2, WG-5442; in the kernel worker's memory until it exists).
  `copy-browser-interface` is amended: the shared copy holds only the anonymous class's parts.
- **Live changes.** A membership, role or permission change raises the access generation and a policy
  change the policy generation (D-FE-12, WG-5435); the browser purges below the floor, interface parts
  included, and reads its new class's manifest. A screen no longer served becomes the not-found page.

**Bounds.** A tenant's interface may name at most 32 distinct roles in its declared audiences
(`ui.audience.too-many-roles`), so an agent cannot multiply classes; the classes actually computed are
at most the distinct role sets the members hold, and the gateway's class cache is bounded by entries
and bytes, evicting by last use (a miss recomputes, never fails).

## Role-based views

One composition, with audiences on its elements, serves every role; a person with two roles is served
the union, because every element is in or out by the same test.

**Layout per role** varies by declared, data and content audiences. There is no layout per role.

**Fields per role.** A role-only read rule removes the column from the shapes of classes that cannot
satisfy it; a per-record rule leaves the column and the server withholds the value per row (drawn as
above).

**Actions per role: show, disable, hide.**

1. **Invokers** (stage 1 of `decide-tenant-operation`): an action whose invokers do not name the
   viewer is absent from the viewer's part (`audOf_sees`).
2. **Per row**, a kernel `preflight` answers `allowed`, `refused` (with the condition's code) or
   `unknown`. It runs stages 1, 3 and 4 of the invocation with the inputs the row supplies:
   - `unknown` when an input the operation needs is not supplied by the row (a value the person types);
   - `unknown` when a condition reads a record or a field the viewer may not read (the preflight is
     evaluated under the viewer's read rights, unlike the invocation, which runs under the operation's
     powers), so a preflight is never an oracle on data the viewer cannot read;
   - `unknown` beyond a per-session budget of preflights (rate-limited like reads);
   - and the result is **advisory**: the time (`ctx.now`), the state and the inputs at the click can
     differ, so the invocation decides, and a refusal at the click refreshes the row's affordance.
   `decide-tenant-operation` calls the same stage functions, so the two share code; they are not
   claimed equal, only that `allowed` never comes from a check the invocation does not run.
   A refused row action is drawn disabled with its error message's wording (`whenRefused: "disable"`)
   or not drawn (`"hide"`); `unknown` is drawn enabled.
3. **Forms** decide on submit.

A reply whose affordances read conditions that mention the principal or the clock is of an individual
class and is not shared.

**The admin panel, by construction.** The **content check**, run by the kernel on every screen: for
each least viewer of the screen's declared audience (the anonymous visitor for `public`; a signed-in
person holding only `public` for `members`; a holder of exactly one named role, and `public`, for each
role named), some element that **counts** (binds data or offers an action; static blocks do not count)
must be reachable through containers that viewer sees, with the viewer in its declared and **content**
audiences. Content audiences count conditional grants only for own-scoped widgets, so:

- the showcase's `/admin` (declared for every signed-in person, every widget readable only by the admin
  role) is refused with `ui.audience.empty-for`;
- the second review's case (members read their own messages under a condition; a moderation queue
  screen declared for members) is refused too (`conditional_grant_does_not_count`), unless the author
  marks the queue widget own-scoped, which states that each member is meant to see their own rows;
- a screen with a write action or an admin-marked widget (`"admin": true`, which the catalogue requires
  of widgets bound to operations that change roles, policies or other people's records) must declare
  its audience explicitly; inheriting `members` is refused.

With the check, a screen served to any viewer in its audience holds an element that counts whose
content audience holds that viewer (`checked_screen_has_content`). The check is sound because every
audience in it is monotone in the viewer's roles (`sees_mono`): declared audiences have no negation,
and a grant with any condition, `not has_role(banned)` included, is conditional and so never counts
for a widget that is not own-scoped. The check is a guarantee about this derivation, not about the
rows: an own-scoped widget can still show an empty state.

A single admin across products needs a role every product may name (showcase gap 2), WG-5474.

**Preview as a role.** A builder may ask for the manifest, the parts and the view reads **as a set of
roles** (`preview-roles`, or "anonymous"). The gateway serves that class, for a principal holding
exactly those roles and `public`, with no identity. Data in a preview is intersected **per row and per
field** with what the builder may read, so a preview never shows a builder more than they may read.
Writes are refused; preview replies are `no-store`. Previewing as a named person is not offered.

## Tenant-arranged layouts

The tenant's arrangement is an **overlay** element (`ui-layout`), one per screen it changes, owned by
the tenant. It addresses widgets, layout containers and regions by **id**, never by position, so an
upgrade that adds or reorders the product's widgets does not misdirect it. It may reorder, move a
widget to a region the template allows, hide, change props within the pattern's schema, add widgets
bound to the screen's own view sections, forms and actions, and narrow audiences, or widen a screen's
declared audience within its content audiences (the content check runs on the result).

**Precedence.** The catalogue constrains everything; the product's base screen is applied first; the
overlay is applied on top of it; the result is checked as one screen.

**When the product changes.** The removal guard of a product's own screens stays (an upgrade that
removes a binding its own published screen names is refused, since the product can fix itself). For
overlays the rule is different, because a security fix must not be blocked by one tenant's
arrangement:

- every overlay whose base screen changed in an upgrade is checked again, whatever changed;
- an overlay entry that names a widget id the base no longer has, or adds a widget whose binding no
  longer exists or changed shape, is **orphaned**: kept as data, left out of the effective screen, and
  listed to the owner with "drop" and "rebind" (and an option, per tenant, to drop orphans
  automatically);
- the upgrade proceeds; the effective screen is valid (`orphan_valid`), and orphaning removes only
  entries whose binding stopped existing (`orphan_removes_only_dangling`);
- if orphaning leaves a screen failing the content check for some audience, the screen is not served
  to that audience until the owner acts, which is the same as the screen not existing for it.

An owner edits overlays on the **layout editor** (a builder-only platform screen), or an agent writes
them; both go through `define-ui-element`. Record-driven pages stay content (`page-canvas` draws block
records whose `kind` chooses among content patterns); structure that binds data stays in overlays.

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

Two things change independently and are versioned independently.

**The catalogue version** (`major.minor`) covers the **schemas**: entries, props, bindings, codes.
Adding is minor; removing, renaming or narrowing is major. `scripts/catalogue-compat.mjs` compares
with the previous version and refuses a narrowing labelled minor. Its limits, stated: it decides
structural narrowing (a prop removed, made required, an enumeration shortened, a type changed, a
pattern dropped from a region) and treats any change to a pattern string, a numeric bound or a
`oneOf` as narrowing unless labelled otherwise, because deciding whether one JSON Schema accepts less
than another is not decidable in general.

**The rendering contract version** (`major.minor`, one line of versions per catalogue major) covers
what the page looks like and does: renderer markup and the class contract, the interpreter's page
model, display-type formats and fallbacks, and token meanings. It has **golden outputs**: for every
catalogue example, the interpreter's page model (JSON) and the renderer's DOM (serialised, attributes
sorted), committed under `web/test/golden/`. A change that moves any golden output is either:

- a **minor** change, when its commit labels it a fix and lists the examples it moves, or
- a **major** change of the rendering contract, when it restyles or re-lays-out an existing pattern.

The web application ships the current major of the rendering contract and the previous one. A
tenant's interface records, with every composition, the **catalogue hash** and the **rendering
contract version** it was made against; a tenant moves to a new rendering major when its owner (or
its agent, with the owner's approval) chooses to, after previewing it; nobody is restyled without
opting in. Deprecated tokens and classes stay for one major as aliases of their replacements; the
gate fails a stylesheet that defines an alias past its major.

**Corpus replay.** `tests/corpus/ui/` holds compositions: the showcase's, exports from real tenants
anonymised (labels and data replaced, structure kept), and adversarial ones (the deepest nesting, every
pattern in every region, long text, right-to-left, every state). Any change to the interpreter, the
display-type formatters or the theme derivation is gated on the page-model golden diff over the whole
corpus: no diff, or a diff its commit labels and lists.

## Version skew

The shell (the built web application), the kernel code, the manifest and the parts come from
different moments.

- The **manifest** and **every part** name the rendering contract version and the catalogue major
  they need; the **shell** names the ones it serves.
- A part or manifest that needs a version the shell does not serve is never drawn: the kernel shows
  "This page has been updated. Reload to continue", keeps drawing what it holds from before, and the
  service worker installs the new shell (docs/frontend.md, "Service worker and the Cache API").
- **Atomic manifest swap.** A new manifest is drawn only when every part the current page needs from
  it is held; until then the page keeps drawing from the previous manifest and its held parts.
  `acceptManifest` (`web/src/kernel/tenant-ui.js`) changes accordingly: today it holds the old part of
  a screen until its replacement arrives, but the errors part and a screen part can come from
  different manifests; after the change, the page's whole part set switches at once.
- The server's **minimum client version** (docs/frontend.md follow-up 9) covers reads as well as
  queued writes: a client older than it is answered "reload" on its next read.

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

1. **Read.** The catalogue (`GET /api/ui/catalogue`); the backend specification's product format;
   the tenant's state through the **agent surface** (WG-5483): installed products and versions,
   the current screens (`GET /api/ui/screens`, builders: every body, draft and published, with widget
   ids), the valid bindings of a screen (`GET /api/ui/bindings?screen=`: each view section with its
   shape and fingerprint, each form and action, and the patterns that accept each), the policies and
   invokers (a builder reads the schema), and the last installed source of the product
   (`GET /api/products/<id>/source`).
2. **Compose.** The agent edits a **tenant app product**, a product file in the tenant's namespace.
3. **Validate.** `POST /api/products/compile` runs specc's layer rules and the catalogue checks and
   returns the bundle, specc's **provenance table** (each entity id to the JSON Pointer of the product
   file that declared it), and diagnostics. Then a dry run of the upgrade returns the projection, the
   **classification** of its diff (below) and the kernel's refusals mapped to pointers through the
   provenance table.
4. **Lease and base.** The agent holds a **build lease** on the product (one session at a time; a
   lease expires unless renewed and can be broken by the owner), and every step names the product's
   **base**: its installed version and draft revision as the agent last read them. A step is refused
   with `product.base-moved` when the product or its draft changed since (another agent, the owner,
   an approved held upgrade), naming what changed; commits elsewhere in the tenant do not conflict.
   This replaces the tenant-wide `expected_version` for product steps.
5. **Apply.** An upgrade with `"into": "draft"`, `"version": "next"` (the platform assigns the next
   version, so two steps never collide and the agent never computes one), the idempotency key
   (matched **before** any version check, so a retry replays the stored result), and a `label`.
6. **Watch.** Builders' connections get the schema notice and draw the new draft (atomic swap, see
   "Version skew"); the step's label is announced once; widgets the step changed are outlined for a
   moment (`dur-settle`; none under reduced motion). Members' connections hear nothing.
7. **Publish.** The owner publishes (or approves the agent's publication); dormant entities activate
   in the same schema change; members' classes get a new manifest hash and a notice.
8. **Undo.** Undo is a new step, at the next version, carrying the old content (the source of an
   earlier version, from the product's history); it is classified like any step.

### Classification and authority

The upgrade classifies its own diff (the projection already lists every entity created, changed and
removed, and the records a migration touches):

| Class | What it holds | Who may apply it |
|---|---|---|
| interface into the draft | screens' draft bodies, composites, templates, presentations, navigation, forms, actions, error messages, filters, samples | an agent delegation carrying `ui-build` and `bundle-install` **scoped to this product**; no step-up per step |
| dormant data | new object types, queries, views, operations and policies created dormant by a draft step | the same, since nothing dormant is reachable by a non-builder |
| needs approval | any change to an active operation, its invokers, an active policy or role permission; any removal; any migration of records; any segment, trigger or automation; publishing | held as a **pending upgrade**, with its projection and classification, until the owner approves it with a fresh step-up; then applied as one product change |

The list of what needs approval is part of the specification (D-DS-14) and of the classifier's code;
a change of kind not in the table is classed "needs approval". Scoping `bundle-install` to products is
new: a delegation names the product ids it may change (`tenant-agent` with a `products` list); an
upgrade of another product by that token is refused. The owner's own path keeps `require_owner` and
step-up as today.

### Diagnostics and convergence

Every refusal has one shape:

```jsonc
{ "code": "ui.binding.unknown-field", "severity": "error",
  "message": "The column \"author_name\" names no field of showcase_community__message.",
  "path": "/screens/2/regions/main/0/props/columns/1/field",
  "entity": "showcase_community__messages", "catalogue": "data-table@1.0",
  "clears": [ { "op": "replace", "path": "/screens/2/regions/main/0/props/columns/1/field",
                "oneOf": ["title", "body", "author", "channel", "status", "created_at"] },
              { "op": "remove", "path": "/screens/2/regions/main/0/props/columns/1" } ] }
```

- `path` comes from specc for specc's codes and from the provenance table for the kernel's (the
  kernel names the entity id; the gateway maps it).
- **Convergence contract**: every code in the catalogue's `codes` section states its `clears`, the
  closed set of edits that remove it (a replacement from a listed set, a removal, an addition of a
  named entity), and every code has a negative fixture in which applying one of its `clears` edits
  makes the fixture pass.
- **Loop budget**: the reference agent driver stops a session after a stated number of refused steps,
  and at once when the same code at the same path comes back after its `clears` edit was applied, and
  files the session's diagnostics with the build log; a repeated diagnostic is a platform bug, not an
  agent's.

### Restructures

A rename or a type change is a removal and an addition, and the removal guard protects published
screens. The **restructure** sequence, each step valid:

1. add the new field or entity (dormant if new to a draft, or a held data step);
2. migrate the data (a held step: a migration or an operation over the records);
3. rebind the draft's widgets to the new binding (interface step);
4. publish (the published bodies no longer name the old binding);
5. remove the old field or entity (a held step; the guard now passes).

A type change of a field goes through the same sequence with a new field; changing a field's type in
place is refused while any screen's binding fingerprint names it.

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
| 8 | publish (needs approval) | draft bodies to published, dormant entities activated | 1 | Members see the message centre |

(The change counts are estimates of entity counts for this example, not measurements.)

### The 200-change cap

A step carries only what changed (a changed entity re-emits its whole definition, which is one
change), so the loop works under the cap; the dry run reports the count. A first install over the cap
(showcase gap 13) is **staged** (D-DS-16) by `plan_install`, a pure function to be written in
`strata_substrate::product`:

- it builds the reference graph of the bundle's entities (a policy names its object type, a view its
  queries, a query its object type, a screen its view, an operation the types it writes, an object
  type the types its references target) and takes its strongly connected components;
- it orders the components topologically and packs them into stages of at most 200 changes, so no
  stage names an entity of a later stage;
- a single component over 200 changes (a cycle of mutually referencing object types with their
  fields, say) is refused with `install.component-too-large`, naming its entities: that product must
  be restructured, since no order installs it in pieces;
- stages are installed as versions `<version>-stage.<n>`, with `release_cmp` given semantic-versioning
  pre-release ordering (a pre-release sorts below its release, stages in numeric order), so the final
  version installs after its stages; this fixes `release_parts` for every product, not only staged
  ones;
- the plan is computed again from the bundle on each attempt, never persisted; an interrupted install
  resumes from the installed stage; the product's visible version history shows only the final
  version (stage records are marked internal);
- the kernel's cap and the all-or-none rule of one product change stay as they are; raising the cap
  was weighed and not taken.

### Prior art

Exegra's agent interface (`C:\dev\exegra\docs\agent-interface.md`): one envelope, `apply` with
`--expect-hash` and `--dry-run`, `admit` listing every failing reason, diagnostics with entity ids and
positions. The showcase's live driver (`tests/corpus/showcase/live.mjs`) shows every step can go
through public routes; the corpus loader's staged install is D-DS-16 by hand.

## Governance

- **The gallery** (`/design`, builder-only) draws every catalogue example in every state, from the
  catalogue; and every composition of the UI corpus (`tests/corpus/ui/`), so real compositions are
  covered, not only canned examples.
- **One pinned environment for pixels.** Screenshots run only in one container image: Linux, the
  Chromium build pinned by the Playwright version in `package.json` (1.65.1 today), the vendored font
  files, and `system-ui` mapped in the test build to one vendored family, so the neutral theme renders
  the same on every machine. Pixels taken anywhere else are not compared. The neutral theme's real
  system fonts are checked by a smoke run on each supported engine that compares nothing but layout
  invariants (no overflow, no overlap of regions).
- **A sampled matrix, stated.** Not every combination is shot; the pairs are:

  | Subject | Themes | Schemes | Widths | Other |
  |---|---|---|---|---|
  | every primitive example, every state | neutral, fieldnotes | light, dark | desk, phone | compact density; forced colours; right-to-left (Arabic); a non-Latin script (Japanese); 400% zoom |
  | every pattern and block example, every state | neutral, fieldnotes | light, dark | desk, phone | right-to-left on desk |
  | every corpus composition | neutral, fieldnotes | light | desk, phone | |
  | stress themes (generated: accents at the luminance extremes, the narrowest and widest type scales, compact density) | the stress set | light, dark | desk | every pattern example once |

  The count of screenshots is the sum of each row's product and is printed by the spec; it is a
  measured number of the suite, not a target.
- **Accessibility:** axe at the WCAG 2.2 A and AA tags on every gallery page and corpus page, no
  violation; keyboard scripts per behavioural primitive and per gesture.
- **Baselines:** a commit that changes a baseline changes nothing else and carries the trailer
  `Baselines: <reason>`; the gate refuses a commit that mixes baseline changes with other files, and a
  baseline change without the trailer. The owner (or a reviewer the owner names) reviews such commits;
  that is a rule of the repository, not something a tool can enforce, and it is stated as such.
- **Contrast:** the catalogue's pairs on every preset at generation, on every stored theme at a
  derivation change (below), and at theme definition.
- **Theme derivation is versioned.** A tenant theme stores its seeds, the **derivation version**, the
  derived token set and its hash, computed at definition; reads use the stored set. A change to the
  derivation is a new derivation version; the gate re-derives a corpus of stored seeds (the presets,
  the stress set, anonymised tenant seeds) and fails if any output moves, unless the change is labelled
  a derivation change; a stored theme moves to a new derivation only through a definition step, which
  runs the contrast check again.
- **Versioning:** `scripts/catalogue-compat.mjs` (schemas) and the golden outputs (rendering) in the
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

`specifications/design-system.jsonc` is an area part (`AREA_PARTS` in `scripts/split-spec.mjs`: ids
that are `ds-` and a number, or start `d-ds-`; the existing behaviour-model ids `ds-save`, `ds-open`
and so on are not in it). It holds nineteen decisions: D-DS-1 to D-DS-6 are the owner's, decided
2026-11-22; D-DS-7 to D-DS-19 are proposals, with a source saying so. It holds thirteen requirements,
DS-001 to DS-013, milestone 7, each stated of its Lean model and checked by the theorems that carry its
`Verifies:` line. A requirement may not name a check that does not exist yet (the gate's evidence
check refuses it), so requirements checked by tests are added with those tests:

| Planned requirement | Its check | Added by |
|---|---|---|
| Every preset theme meets AA on every catalogue pair, in both schemes | generation check with a failing fixture | WG-5458 |
| A theme definition with a failing pair is refused naming the pair; stored themes do not move on a derivation change unless labelled | substrate tests, gate re-derivation | WG-5460 |
| Every accepted catalogue example validates and every refused example is refused with its code | kernel and specc tests | WG-5459 |
| A composition with an unknown pattern, field, binding kind or placeholder at publish is refused with its code and path | kernel test | WG-5464 |
| A member is never served a builder's draft, a draft notice, or a schema version | gateway tests | WG-5464, WG-5465 |
| A part, a section of a view reply and an error message are served only to a class that is served the element | gateway tests | WG-5465 |
| A preflight's `allowed` comes only from checks the invocation runs, and reads nothing the viewer cannot read | kernel differential | WG-5466 |
| Every code's `clears` edit clears it on its negative fixture | specc and kernel tests | WG-5482 |
| A golden output moves only in a labelled commit | gate check | WG-5478 |
| No page of the showcase shows an epoch, a principal or a JSON object | Playwright | WG-5475 |
| axe finds no violation on any gallery or corpus page | Playwright | WG-5473, WG-5475 |

## What is proved

`lean/Strata/UI/Composition.lean`, `Contrast.lean` and `Visibility.lean`, imported from
`lean/Strata.lean` and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom.

| Property | Theorem | Requirement |
|---|---|---|
| The check is exactly validity (distinct ids, placement, binding kind and existence) | `check_iff`, `checked_bindings_resolve` | DS-001 |
| Publishable: no placeholder at all; every data widget bound and accepted | `publishable_no_placeholder`, `publishable_bound` | DS-002 |
| Refinement keeps validity, one placeholder fewer per binding, ends publishable | `bindAt_valid`, `bindAt_placeholders`, `refine_valid`, `refine_publishable` | DS-003 |
| Incremental checking over a changing schema and catalogue, with the removal guard; without it a screen breaks | `incremental_valid`, `unguarded_removal_breaks` | DS-004 |
| The derived on-colour meets AA; a ramp search ending at it succeeds | `onColour_meets_aa`, `search_finds` | DS-005 |
| The theme check is exactly the declared pairs | `checkTheme_iff` | DS-006 |
| Everything served is visible to the viewer, inside visible containers | `render_sound` | DS-007 |
| A node not served leaves no trace among its siblings or in its container | `hidden_leaves_no_trace`, `hidden_leaves_no_trace_in` | DS-008 |
| An action's audience is exactly the kernel's invoker admission | `audOf_sees` | DS-009 |
| Data audiences are the roles with a Read grant, the public role included; content inside data | `dataAud_sees`, `public_grant_anon_sees`, `contentAud_sub` | DS-010 |
| A checked screen serves every viewer in its audience an element that counts, in its content audience | `checked_screen_has_content` (with `sees_mono`, `minimal_le`, `reach_served`) | DS-011 |
| Every container served holds a served element | `served_has_element` | DS-012 |
| Orphaning dangling widgets keeps a valid screen and removes nothing else | `orphan_valid`, `orphan_removes_only_dangling` | DS-013 |
| The first review's two counterexamples, now refused or pruned | `empty_region_pruned`, `conditional_grant_does_not_count` | supporting |

**Not proved, and what carries it.**

- The Rust and JavaScript against the models (the kernel's check, the gateway's filtering, the
  preflight's relation to the invocation): differential tests in WG-5464, WG-5465 and WG-5466.
- Luminance and its rounding: fixed point in the model; the implementation rounds the lighter colour
  down and the darker up (WG-5460).
- Shape fingerprints: the models abstract a binding; that a binding includes its fingerprint, and so
  that a shape change is a removal, is the implementation's (WG-5464).
- The composition model's widgets are a flat list per screen; layout containers and their nesting are
  in the visibility model only. Overlay operations other than dropping an orphan are not modelled.
- Data audiences model Read grants with a conditional flag, an upper bound of who gets rows; that a
  grant's condition is detected as a condition (any condition at all) is the implementation's.
- The interface class's computation from `audience.rs`, the class content hash, notice targeting and
  per-class error filtering are tested (WG-5465), not modelled.
- Contrast is necessary, not sufficient, for an accessible theme.

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
9. (WG-5465) **Visibility: audiences, data and content audiences, the content check, interface classes,
   pruning, class hashes, notice targeting, per-class errors and sections, cache rules.** Needs 8.
10. (WG-5466) **Advisory preflight, row arguments, prefill, reference coercion.** Needs 8, 9.
11. (WG-5467) **Interpreter and pattern renderers, stable keys and focus.** Needs 5, 6, 7, 8.
12. (WG-5478) **Rendering contract versions, golden outputs, corpus replay, version skew and the
    atomic manifest swap.** Needs 11.
13. (WG-5479) **Composite patterns and templates as data, platform blocks, the renderer registry,
    catalogue gaps.** Needs 8, 11.
14. (WG-5480) **Structured bindings: grouped, series and charts, positioned, record-defined forms,
    shared filters.** Needs 8, 11.
15. (WG-5481) **Gestures, wizards and pending state.** Needs 10, 11.
16. (WG-5468) **Shell variants, navigation and the notification bell.** Needs 9, 11.
17. (WG-5469) **Tenant layout overlays by id, orphaning and the layout editor.** Needs 9, 11.
18. (WG-5470) **Preview as a set of roles.** Needs 9.
19. (WG-5482) **Provenance and structured diagnostics with the convergence contract.** Needs 2.
20. (WG-5472) **Pre-release ordering and staged installs by a pure planner.** No dependency.
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

## Open questions

- Should a builder be able to preview as a named person, with that person's consent recorded?
- Should a field's existence be hideable from readers of its record type when its read rule is
  per-record, at the cost of a per-viewer shape on every read?
- Should the platform serve map tiles of its own, so that a geographic map can be a renderer extension?
- Should tenants upload fonts, with a font sanitiser on the server, so that full brand typography is
  possible under `font-src 'self'`?
- Should product segments move inside the schema change, so a step is atomic for its segments too?

## Answers to the reviews

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
| B3 | The overlay guard is one-dimensional, DS-004 overclaims | Fixed: ids on widgets, containers and regions; shape fingerprints in bindings; overlays rechecked whenever their base changes; orphaning instead of refusal, proved (`orphan_valid`, `orphan_removes_only_dangling`, DS-013); precedence stated; `incremental_valid` now takes a changing catalogue; DS-004 reworded and its rationale no longer claims overlays cannot break |
| B4 | Weak governance | Fixed: pinned container, `system-ui` mapped for tests, the matrix as a stated pair table, generated stress themes, corpus compositions in the gallery, baseline commits separated and trailer-checked (review is stated as a repository rule a tool cannot enforce) |
| B5 | No corpus replay | Fixed: `tests/corpus/ui/` and page-model golden diffs gating interpreter, formatter and derivation changes |
| B6 | Version skew | Fixed: contract versions in manifest, shell and parts; mismatch reloads; atomic manifest swap (`acceptManifest` changed); minimum client version for reads |

**C. Visibility and roles**

| # | Finding | Answer |
|---|---|---|
| C1 | Empty region served (compiled counterexample) | Confirmed and fixed: containers and elements are separate in the model, empty containers are pruned, the content check recurses to counting leaves; `served_has_element` (DS-012), `hidden_leaves_no_trace_in`, and the counterexample is `empty_region_pruned` |
| C2 | Conditional grants defeat the content check | Confirmed and fixed: the check uses content audiences (unconditional grants, or any grant for an own-scoped widget); screens with write actions or admin-marked widgets must declare their audience; `conditional_grant_does_not_count` is the review's case. The check is stated as a guarantee about the derivation, not about rows |
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
