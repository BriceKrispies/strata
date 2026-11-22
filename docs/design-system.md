# The design system and agentic composition

How a Strata tenant's screens are made, and made by agents: the design tokens and themes, the
primitives, the patterns, the app shell and templates, the contract that turns data into interface,
wireframes, who sees what (public and private, role by role), the layouts a tenant arranges, and the
loop in which an agent builds a tenant's screens while the tenant watches. This is a design. Its
requirements are in `specifications/design-system.jsonc` (DS-001 to DS-011, D-DS-1 to D-DS-16), the
properties that can be stated precisely are proved in Lean (`lean/Strata/UI/`), and the work is
listed at the end in dependency order, each item a Workgraph task under WG-5457.

It builds on the browser client design (`docs/frontend.md`, WG-5415) and changes none of its
decisions: the kernel stays pure, custom elements draw and the kernel decides (D-FE-4), no third-party
code reaches the browser (D-FE-9), the browser's copies keep their rules (D-FE-1, D-FE-2, D-FE-11).

## The recommendation in one page

Six decisions are the owner's (2026-11-22) and are recorded as decided: a neutral base with tenant
theming through tokens, Fieldnotes becoming one theme (D-DS-1); layouts the tenant arranges, as data
(D-DS-2); agentic first (D-DS-3); wireframing as a first-class concept (D-DS-4); public and private
for every element, with the rule that the interface never discloses what the data layer would refuse
(D-DS-5); and role-based views (D-DS-6). Every other D-DS-* is a proposal of this design.

- **One catalogue, read by everyone** (D-DS-7). A single machine-readable file,
  `specifications/ui-catalogue.jsonc`, lists the tokens, themes, primitives, patterns, templates and
  display types, each with a JSON Schema and examples (accepted and refused). specc generates the
  kernel's validator tables (Rust) and the browser's interpreter tables (JavaScript) from it; the
  gallery draws its examples; the gateway serves it to agents. There is no second description.
- **Tokens and themes** (D-DS-15). Components read semantic tokens only. A theme is token values:
  a preset gives them all (Fieldnotes), a tenant gives a few typed seeds and the rest is derived.
  Contrast is checked when a theme is defined, against the pairs the catalogue declares, and a theme
  that fails one is refused; the derived on-colour always passes (`onColour_meets_aa`). Values reach
  the page through the CSSOM, never as stylesheet text.
- **Primitives are mostly native elements** (D-DS-8). A button is a `<button>` with a class contract,
  a field is a `<label>` and an `<input>`. Custom elements exist only where a native element lacks the
  behaviour: dialog, menu, tabs, tooltip, toast region, record picker. Light DOM, no Shadow DOM.
- **Screens are compositions** (D-DS-10): a template, its regions, and widgets, each widget a pattern
  bound to a section of a view, a form, an action or a platform source. A widget with no binding is a
  wireframe placeholder drawn from a declared sample shape. The kernel checks every composition
  against the catalogue and the schema when it is defined (`check_iff`, `incremental_valid`).
- **Display types end raw values** (D-DS-9). A presentation element per object type names its title
  field and a display type per field (datetime, person, reference, status, money and so on). The
  gateway resolves people and reference titles in the view reply. No screen draws an id or an epoch.
- **Drafts and published** (D-DS-10). A screen holds a published body and, while it is being built,
  a draft body. Only builders see drafts; wireframes exist only in drafts; only a draft at full
  fidelity can be published.
- **Visibility is served, not hidden** (D-DS-11). Every element has a declared audience (public,
  signed-in, or roles) and, where it shows data or offers an action, a data audience derived from the
  policies and the operation's invokers. The server filters the manifest and the parts per audience
  class and serves nothing the viewer does not see; the browser filters nothing. A hidden element
  leaves no trace (`hidden_leaves_no_trace`). Only what the anonymous class is served may go to a
  shared cache.
- **Roles vary one composition** (D-DS-12). One layout, with audience conditions on its elements,
  never a copy per role. Actions are shown, disabled or absent from the kernel's own pre-flight of the
  operation (`audOf_sees`); a screen whose content a role cannot see cannot be declared for that role
  (`content_check_nonempty`). Builders preview a page as a set of roles.
- **Tenant layouts are an overlay** (D-DS-13) the tenant owns, over the screens products own.
- **The agent loop** (D-DS-14): an agent reads the catalogue and the backend specification, writes a
  tenant app product, and applies it in steps, each a dry run and then an upgrade into the draft with
  the expected schema version, at most 200 changes, with a one-line label. Refusals come back as
  structured diagnostics that point into the product file. The owner watches the draft fill in live
  and publishes it.
- **Large installs are staged** (D-DS-16), so the 200-change cap stays as it is.

## Where the framing should change

The request is the right direction. Taken literally, some of it would make the system worse.

1. **"Primitives as Web Components: button, field and inputs, form-associated with ElementInternals"
   is wrong for the inputs that have native elements.** A native `<input>`, `<select>` or
   `<textarea>` is already form-associated, labelled by `<label for>`, validated, autofilled and
   understood by every assistive technology. Wrapping it in a form-associated custom element
   re-implements all of that, and a light-DOM wrapper still has to forward labels and ARIA. The
   catalogue keeps every primitive the request names; most are implemented as a class contract on a
   native element, and a custom element is used only where behaviour has no native form (the list in
   "Primitives"). The catalogue entry is the same for an agent either way. This is D-FE-4 made precise.
2. **"A display-type catalogue that maps field types to renderings" cannot by itself end raw ids.**
   The field types are `str`, `int`, `bool`, `time`, `ref`, `list`, `file`, `language`
   (`crates/model-core/src/types.rs`, `FieldType`): a principal is a `str` (the showcase's `author`),
   money is an `int`, a status is a `str` driven by a workflow. The display type must be declared, and
   two of the renderings need data the browser does not have: a person's name, and a referenced
   record's title. So the contract has three parts: a declared display type per field (presentation
   elements), server-side resolution of people and reference titles in the view reply, and a fallback
   per field type that never draws the raw value.
3. **"Tenants can arrange layouts" and "products own their screens" conflict unless the arrangement is
   a layer of its own.** D-8250 gives a product ownership of its interface elements, so that a schema
   editor cannot change them behind the product's back and an upgrade replaces them. A tenant edit of
   a product's screen would be lost at the next upgrade or would block it. The tenant's arrangement is
   therefore an overlay element the tenant owns, keyed by the product's screen, and an upgrade that
   removes something the overlay names is refused with a diagnostic naming the overlay entry.
4. **"Possibly before the data model exists" needs a validity rule that allows it.** Today the kernel
   refuses a screen whose view does not exist (`crates/kernel/src/ui.rs`). A wireframe is valid
   because its unbound widgets name nothing: a placeholder is an explicit `bind: null` with a sample
   shape, not a dangling reference. Valid at every step means passing the same check at every step;
   the check allows placeholders and refuses dangling bindings (`check_iff`).
5. **"The tenant watches it being built live" should mean the owner, not the members.** An agent's
   work in progress is a draft. Showing members a page whose structure changes every few seconds,
   with placeholder shapes where data will be, is not a product; it also discloses plans. Drafts are
   served to builders only, and publishing is the owner's step.
6. **The 200-change cap is not a problem for the agent loop.** An upgrade carries only what changed
   (the showcase's second stage was 69 changes), and an agent's steps are small by design. The cap
   bites only on the first install of a large product (showcase gap 13); that is answered by staging
   the install, not by raising the cap (D-DS-16).
7. **"Hide what a role may not use" must not be a client rule.** A client that hides an admin panel
   with its own role check drifts from the server and discloses the panel's existence in its code and
   its manifest. Visibility is decided by the server from the same policy and invoker evaluation that
   decides reads and invocations, and the browser is never sent what it must not draw. This also
   changes one rule of D-8250: the manifest is no longer the same for every visitor
   (`crates/backend/src/ui.rs`: "the session, if any, plays no part").
8. **A theme with arbitrary values conflicts with CMS-005 as worded** ("a theme value that is not a
   value of the token schema SHALL be rejected") only if the schema is a list of names. The token
   schema becomes typed (a colour is an sRGB value, a font is one of the vendored families, a radius
   is one of a scale), and CMS-005 holds of that schema; fonts stay a closed list because D-FE-9 and
   `font-src 'self'` exclude fonts from elsewhere.

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
| Lines | `border`, `border-strong`, `focus` | colour |
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
`tone-danger-fg` (4.5); each tone's `fg` on its `bg` (4.5); `border-strong`, `focus` and `accent` on
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
exactly the pairs). An agent's styling step that picks an accent too light for links is refused with
the diagnostic, and the derived suggestion in the hint.

**Delivery.** The resolved token values come to the page with the bootstrap and the view model (as
the theme does today, `crates/bff/src/routes.rs:1507-1516`). The main thread sets each with
`document.documentElement.style.setProperty`, after checking the value against the token's type a
second time; no value is concatenated into a stylesheet, so no tenant value can open a rule or a
`url()`. CSSOM property writes are not inline markup and need no CSP change. Fonts are the vendored
families of WG-5439.

**Where a theme lives.** A theme is tenant configuration, changed through the settings intents as the
accent and paper are today; the theme of a tenant is not a product entity, so installing a product
never changes how the tenant looks. A product may ship a theme preset for the tenant to choose.

## Layer 2: primitives

No primitive knows anything about Strata data. Each is keyboard-complete and passes axe at WCAG 2.2
AA in every theme, in every state the catalogue lists.

| Primitive | Implementation | Keyboard and semantics |
|---|---|---|
| button | `<button class="ds-button">`, variants `primary`, `secondary`, `quiet`, `danger`; sizes | native |
| link | `<a class="ds-link">` | native |
| field | `<div class="ds-field">` with `<label>`, the control, hint and error joined by `aria-describedby`; `aria-invalid` | native |
| text, number, date, time inputs, textarea | native elements in a field | native; form-associated already |
| select | `<select>` in a field | native |
| checkbox, radio group | native inputs; a radio group is a `<fieldset>` with `<legend>` | native |
| switch | `<input type="checkbox" role="switch">` | native, announced as a switch |
| record picker | `<ds-record-picker>`, form-associated (ElementInternals), the ARIA combobox pattern | arrows, Enter, Escape, typeahead; the one input with no native element |
| dialog | `<ds-dialog>` around a native `<dialog>`, opened with `showModal()` when its `open` property is set | focus moves in, is trapped by the browser, returns to the opener; Escape closes |
| menu | `<ds-menu>`: the menu button pattern | roving tabindex, arrows, Home, End, typeahead, Escape returns focus |
| tabs | `<ds-tabs>`: the tabs pattern, manual activation | arrows move, Enter or Space selects |
| tooltip | `<ds-tooltip>`, on hover and focus, `aria-describedby` | Escape dismisses; never the only carrier of information |
| toast | `<ds-toast-region>`: a `status` live region, `alert` for errors | errors do not dismiss themselves; others pause on hover and focus |
| avatar | `<img alt>` or initials; decorative (`alt=""`) beside a visible name | none |
| badge, chip | `<span class="ds-badge">`; a removable chip is a button | native |
| progress | native `<progress>` or `<meter>` | native |
| skeleton | `<div class="ds-skeleton" aria-hidden="true">` inside a container with `aria-busy="true"` | none |
| layout | `ds-stack`, `ds-cluster`, `ds-grid`, `ds-sidebar`, `ds-switcher`, `ds-center`: classes with custom-property parameters (gap, threshold, minimum width), intrinsic and container-query based | none |

**Rules.**

- **Light DOM.** Labels, `aria-labelledby` and `aria-describedby` cannot cross a shadow root, the
  uiScenarios query by role and name, and the theme's tokens and the page's layers must reach every
  element. No primitive uses Shadow DOM. The only shadow root in the application remains the QR leaf
  named in D-FE-4, which has no cross-references.
- **What an element may hold.** A custom element holds no application state (D-FE-4). It may hold
  interaction state that no other part of the page depends on and that a redraw may lose: which menu
  item has focus, whether a tooltip is showing. Anything that changes what data is shown, or should
  survive a reload or be in the address (the selected tab of a page, a sort, a page of results), is
  kernel state, carried in the view model and reported by a `CustomEvent` that the main thread turns
  into a kernel message.
- **Properties, not markup.** Elements take properties through `dom.js`'s property channel (an
  attribute named `.items` is set as the property `items`) and build their children with DOM APIs.
  Nothing is parsed (D18), so Trusted Types need no new policy.
- **Registration.** Custom elements are defined on the main thread before the first draw; the kernel
  worker only names them in virtual nodes.

## Layer 3: patterns

A pattern is a composition of primitives bound to a kind of data. It is what an agent places.

| Pattern | Binds to | Shows | Main props |
|---|---|---|---|
| `record-detail` | a single section | the fields of one record, by display type | `fields`, `layout` (`list`, `columns`) |
| `record-card` | a single section, or a row of a list | title, subtitle, meta, body, actions | `title`, `subtitle`, `meta`, `body`, `media`, `actions` |
| `record-list` | a list section | a card per row | as `record-card`, `density` |
| `data-table` | a list section | rows and columns, row actions, sorting among the query's declared orders, paging by cursor | `columns`, `rowActions`, `sort`, `pageSize`, `stackBelow` (container width under which rows become cards) |
| `form` | a form | fields, prefill, client checks from the operation's input types, server diagnostics by field | `prefill`, `layout`, `submit` |
| `feed` | a list section | actor (person), verb, object (reference), time | `actor`, `verb`, `object`, `time`, `body` |
| `comment-thread` | a list section and a form | comments with author and time, and the reply form | `author`, `body`, `time`, `form` |
| `reaction-bar` | an aggregate grouped by a field, and an action | counts per kind, the viewer's own reaction marked | `kinds`, `action` |
| `notification-bell` | the platform inbox | the unread count and a menu of recent notices | `limit` |
| `notification-list` | the platform inbox, or a list section | notices with read state and a mark-read action | `markRead` |
| `profile-header` | a single section | avatar, name, role line, facts, actions | `name`, `avatar`, `facts`, `actions` |
| `stat-tile` | an aggregate or a count | one number with its label and unit | `label`, `unit`, `display` |
| `aggregate-bars` | an aggregate grouped by a field | a ranked bar list (a leaderboard) | `label`, `limit` |
| `markdown-block` | nothing (static text in props), or a `markdown` field | prose blocks | `text` |
| `link-list` | nothing: screens of the tenant | links to screens by id | `links` |
| `nav-menu` | navigation entries | the primary navigation | `group` |

**States.** Every pattern that binds to data has four states, drawn by the pattern itself and listed
in the catalogue: `loading` (a skeleton of the pattern's shape with a fixed number of rows, so the
skeleton says nothing about how many records there are), `empty` (a sentence the pattern's props give,
with an optional action), `error` (what failed and a retry), and `ready`. A wireframe placeholder is
a fifth, `placeholder`, drawn only in drafts.

**Confirm flows.** An action's `confirm` prop (`title`, `body`, `confirmLabel`, `tone`) opens a
`ds-dialog` before the action is sent; a destructive tone puts focus on the cancel button. This
replaces today's second-press confirmation.

**Server diagnostics.** A refused form draws the refusal through the tenant's error messages, as
`explainRefusal` does today, with each failed check on its field and focus moved to the first one.

## Layer 4: the app shell, regions and templates

**The shell** is one element per tenant (tenant-owned, configured like the theme), with named
regions:

| Region | Contents | Responsive behaviour |
|---|---|---|
| `header` | slots `brand`, `nav`, `search`, `actions`, `account` | under the container width the catalogue sets, `nav` collapses into a menu button |
| `nav` | an optional side navigation (`nav-menu` of a group) | collapses into the header's menu |
| `main` | the screen's template | always present |
| `aside` | the screen's aside region, when its template has one | moves below `main` under a container width |
| `footer` | `link-list`, `markdown-block` | stacks |

Products contribute **navigation entries** (`navigation` section: screen, label, group `primary`,
`admin` or `account`, order); the tenant's overlay orders and hides them. The `notification-bell` is
placed in `header.actions` by the shell, bound to the platform inbox (`/api/inbox/count` and `list`,
CT-821 to CT-826), which closes showcase gap 5's header slot.

**Templates** arrange a screen's main area:

| Template | Regions | Rules |
|---|---|---|
| `list` | `header`, `filters`, `main`, `aside` | `main` holds at least one list pattern |
| `detail` | `header`, `main`, `aside`, `below` | `header` holds one `profile-header` or `record-card`, bound to a single section |
| `dashboard` | `header`, `tiles`, `main`, `aside` | `tiles` holds `stat-tile`s; `main` a grid of patterns |
| `settings` | `header`, `main` | `main` holds forms and `record-detail`s |
| `feed` | `header`, `main`, `aside` | `main` holds one `feed` or `record-list` |
| `editor` | `header`, `main`, `aside` | `main` holds one `form`; `aside` its context |

The catalogue says which patterns each region allows and how many; the kernel enforces it. A region
that holds nothing the viewer is served is not drawn at all: no heading, no empty box (see
"Visibility").

Today's four layouts are templates of the same names, and a screen that declares only `kind` (every
installed product today) is read as that template with one widget per view section in name order,
drawn as `record-detail` or `record-list`: so every installed product keeps working, and looks better,
before it is rewritten.

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

- A **binding** is `{ "section": <name of a section of the screen's view> }`,
  `{ "form": <form id> }`, `{ "action": <action id> }`, `{ "source": "inbox" | "navigation" }`, or
  `null` (a placeholder).
- A **placeholder** carries `"sample"`: the shape it will show (`{ "single": false, "rows": 5,
  "fields": [ { "label": "Title", "display": "text" }, { "label": "Posted", "display": "datetime" } ] }`),
  drawn with skeleton shapes and its labels, and the widget's `"note"` (one line saying what it will
  show). A sample holds no data: labels and display types only.
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
  action on the screen, or a platform source; field names in props exist on the section's object;
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

## Wireframes and fidelity

A wireframe is the same composition with placeholders. It is drawn at once in the neutral base theme
or the tenant's: labelled regions, the pattern's skeleton with the sample's labels and display
shapes, and each widget's note.

**Valid at each step, mechanically.** A composition is **valid** when every widget is allowed where
it is placed and every binding it has names something the schema defines; a placeholder names
nothing and is valid (`Composition.Valid`). It is at **full fidelity** when every widget whose
pattern needs data is bound (`Composition.Full`). The kernel runs the check on every change and
refuses an invalid result, so every state the tenant can see passed it. Proved:

- the check is exactly validity (`check_iff`), so a checked composition never references a missing
  binding (`checked_bindings_resolve`), and at full fidelity every data widget is bound to something
  that exists (`full_valid_bound`);
- binding a placeholder to something that exists keeps the composition valid and leaves one
  placeholder fewer (`bindAt_valid`, `bindAt_unbound`); refining one widget at a time is valid at
  every step and ends at full fidelity (`refine_valid`, `refine_full`);
- a schema change that removes nothing an unchanged screen names, with every changed screen checked
  against the new schema, leaves every screen valid (`incremental_valid`), so the kernel need only
  check the screens a change touches plus the removal guard; without the guard a valid screen breaks
  (`unguarded_removal_breaks`).

**Publishing** a draft is refused unless it is at full fidelity: a placeholder is never served to a
member or the public.

**Fidelity is per widget, not per screen.** An agent can bind the message list while the reactions
widget is still a placeholder; the draft shows real messages beside a labelled reaction-bar skeleton.

## Visibility: public and private

### Declaring it

Every element declares an **audience**: `"public"` (anyone, signed in or not), `"members"` (every
signed-in person of the tenant), or `{ "roles": [...] }` (the holders of any of the roles). An
absent audience means the parent's. The levels are:

| Level | Where declared | Default |
|---|---|---|
| app (the shell) | the shell element | `members` |
| navigation entry | its entry | its screen's audience |
| screen | the screen (replaces `auth`: `anyone` is `public`, `session` is `members`) | the app's |
| region | `regionAudience` of the screen or overlay | the screen's |
| widget (a pattern instance) | the widget | its region's |
| field rendering, column | the widget's props (`columns[i].audience`, `fields.<f>.audience`) | the widget's |
| action | the action placement in props | the widget's |
| placeholder | as a widget; served only in drafts (below) | |

**Composition.** A viewer is served an element only when the viewer is in its declared audience, in
the declared audience of every ancestor, and in its **data audience**. The data audience is never
written; it is derived by the kernel:

- for a widget bound to a section: the roles holding a Read policy (with or without a condition) or a
  Read permission on the section's object type, and the owner; when `public` is among them (the role
  `roles_of` gives every anonymous and signed-in principal), anyone (`dataAud_sees`);
- for a column or field rendering: the widget's data audience, narrowed by the field's read rule when
  the rule depends only on roles (`has_role`); a rule that reads the record, the principal or the
  clock cannot be decided per screen, and is decided per row by the server (below);
- for an action: the invokers of its operation (`audOf`: every signed-in person, or the named roles),
  exactly the audience the kernel's invoke check admits (`audOf_sees`);
- for a navigation entry: its screen's effective audience;
- for the platform inbox: `members`.

A child may not declare an audience wider than its parent's: such a composition is refused
(`ui.audience.wider-than-parent`), because an author who writes `public` on a widget inside a
members-only screen believes something false. So "a public page containing a private widget" is a
public screen with a widget declaring `{ "roles": ["editor"] }`: the public is served the page
without the widget; an editor is served it with the widget. "A private region inside a public
template" is the same at region level: a viewer outside the region's audience is served the screen
without the region, its heading or its box, and the template's layout reflows as it does for an empty
region.

### The rule: nothing the data layer would refuse

The interface never discloses what the data layer would refuse. Concretely, for a viewer outside an
element's effective audience:

- the element is not in the manifest or the part the viewer is served, so neither its label, its
  placeholder, its skeleton, its props nor its existence reaches the browser;
- a region left with nothing is not drawn: no heading, no "nothing here" sentence;
- no count says how many elements, rows or fields were left out, and a skeleton's row count is fixed
  by the pattern, never by the data;
- a screen the viewer is not served answers its address with the same not-found page as an address
  nothing answers, with the same status and the same text;
- a column whose field the viewer's roles can never read is not in the viewer's shape; a value a
  per-record read rule withholds is drawn exactly as an absent value ("—"), so withheld and empty
  cannot be told apart;
- aggregates count only records the reader may read (D-5061), so a `stat-tile` discloses nothing the
  reader could not count;
- an action the viewer may not invoke is absent; one refused for this row by a condition is drawn
  disabled with the condition's own wording, which an attempt to invoke it would show anyway.

This is proved of the model in which the server serves only the nodes the viewer sees under both
audiences, below nodes the viewer sees: every label served comes from such a node (`render_sound`),
and a node the viewer does not see changes nothing in what the viewer is served, wherever it is
(`hidden_leaves_no_trace`).

**What is disclosed, stated.** A field's existence is disclosed to readers of its record type when its
read rule is per-record (the column header is in their shape); a tenant that must hide a field's
existence from a role uses a rule that depends on roles only. The structure of a screen a viewer is
served is disclosed to that viewer, by design.

### Drafts and published

A screen holds a **published** body and, while it is being built or changed, a **draft** body. The
draft's audience is the **builders**: the holders of the capability `ui-build` (the owner holds it;
the tenant admin role may be granted it; an agent acting for a builder carries it in its delegation).

- A member or the public is never served a draft, its placeholders, its notes, or the fact that one
  exists. A screen with no published body is not in their manifest and its address is not found.
- Builders see the draft in **preview mode**, with a bar saying so, and can switch to the published
  body.
- Publishing replaces the published body with the draft, in one schema change, and is refused unless
  the draft is valid at full fidelity and passes the content check.
- **Limit, stated.** Drafts cover interface elements only. A data-model step (a new object type,
  operation or policy) is live for API callers as soon as it commits, whatever the screens say. That
  is why data steps are classed as risky changes the owner approves (see "The agent build loop"), and
  why an agent declares a new operation's invokers before any screen offers it.

### Relation to today's auth and to policies

`auth: anyone | session` (`UiAuth`) becomes the screen's audience `public | members`; the role
variant is what showcase gap 2 asked for, generalised to every level. Audiences narrow what the
interface offers; they never grant data. A widget a viewer is served still shows only the rows and
fields the view reply carries, which the kernel has filtered by the same policies; an action a viewer
is served is still decided by the kernel when invoked. The audience classes of
`crates/kernel/src/audience.rs` are reused: the interface's audience class of a viewer is the set of
the viewer's roles that any audience of the tenant's interface names, plus whether the viewer is a
builder.

### Serving and caching

- **Per class, by the server.** The gateway computes the manifest and the parts for the viewer's
  interface audience class (`strata_substrate::ui`, from the schema and the class, pure). Each part is
  named by the hash of its filtered content, so two classes that are served the same screen share its
  hash. The number of classes is bounded by the distinct sets of the roles the tenant's audiences
  name; the gateway keeps the definition of a class in memory keyed by the schema hash and the class,
  as the aggregates copies are keyed (`crates/backend/src/aggregates.rs`).
- **A part is served only to a class whose manifest lists it.** A request for any other hash is
  answered not-found, the same answer as a hash no definition holds, so a hash learned elsewhere
  fetches nothing.
- **Anonymous visitors.** The manifest and parts of the anonymous class are public: they may be
  cached by the edge, sent `Cache-Control: public, immutable` (a part) and kept by the service worker
  in the shared Cache API (D-FE-2's interface copy).
- **Everyone else.** Manifests and parts of any other class are sent `Cache-Control: no-store`, so no
  shared HTTP cache in the browser profile keeps them, and are kept only in the principal's encrypted
  partition in IndexedDB, under the rules of view results (stamped, purged below the floor and at a
  new tenant epoch, gone at sign-out). Until the encrypted partition exists (WG-5442) they are kept
  in the kernel worker's memory only. `copy-browser-interface` is amended to say the shared copy holds
  only the anonymous class's parts.
- **Live changes.** A schema notice makes the browser read the manifest again (today); a membership,
  role or permission change raises the access generation and a policy change the policy generation
  (D-FE-12, WG-5435), the browser purges what it holds below the floor, interface parts included, and
  reads the manifest of its new class. A screen that is no longer served becomes the not-found page;
  a screen newly served appears in the navigation without a reload.

## Role-based views

One composition, with audiences on its elements, serves every role. A person with the moderator role
and a member open `/messages/12`; both are served the same template; the moderator's part also holds
the hide action in the record card and the moderation region; the member's does not. A person with two
roles is served the union, with no precedence rule to resolve, because every element is in or out by
the same test.

**Layout per role.** Widgets, regions and actions vary by their declared audiences and their data
audiences, as above. There is no layout per role: copies drift, a person holding two roles would need a
precedence rule, and the content check would have to run on every copy.

**Fields per role.** Read rules hide fields. A rule that depends on roles only removes the column from
the shapes served to roles that cannot satisfy it; a per-record rule leaves the column and the server
leaves the value out of the rows it withholds (`FieldDef.read`, D-6586, evaluated on every read path).

**Actions per role: show, disable, hide.** Decided by the server from the kernel's own evaluation:

1. **Invokers** (stage 1 of `decide-tenant-operation`): an action whose operation's invokers do not
   name the viewer is absent from the viewer's part (`audOf_sees`).
2. **Per row** (stages 3 and 4: the subject is readable, every condition holds): the view reply
   carries, for each row action, `allowed` or `refused` with the condition's code, computed by a
   kernel function `preflight` that runs exactly those stages and that `decide-tenant-operation`
   itself calls before its steps, so the two cannot drift. A refused row action is drawn disabled with
   the wording of its condition's error message (`whenRefused: "disable"`, the default) or not drawn
   (`whenRefused: "hide"`).
3. **Forms** keep deciding on submit: a condition that reads what the person types cannot be decided
   before they type it.

A reply whose affordances read conditions that mention the principal or the clock is of an individual
audience class and is not shared (the aggregates classifier's rule, `audience.rs`). The cost is
bounded: a widget has at most five row actions (a catalogue limit), so a page of at most 200 rows
(`MAX_PAGE_SIZE`) needs at most 1,000 pre-flights, each of which reads only the row and what its
conditions read.

**Role-specific navigation, and the admin panel.** A navigation entry is served when its screen is
served. An admin control panel is a screen declared `{ "roles": ["<admin role>"] }`, so a member is
never served it, its entry or its address. That it cannot be declared for members by mistake is the
**content check**, run by the kernel on every screen: for each least viewer of the screen's declared
audience (the anonymous visitor for `public`, a signed-in person holding only `public` for
`members`, a holder of exactly one named role and `public` for each role named), at least one widget
that binds data or offers an action must be in that viewer's effective audience. Static widgets
(`markdown-block`, `link-list`) do not count, so an introduction paragraph cannot make an admin panel
pass; a screen with no data or action widget at all (a content page) is not subject to the check. The showcase's `/admin` (declared for every signed-in person, every widget readable only by
the admin role) is refused with `ui.audience.empty-for` naming the least viewer and the widgets. With
it, a screen served to anyone shows at least one widget (`content_check_nonempty`,
`checked_screen_renders_content`). A widget whose data audience includes a role only through a
conditional policy (a member's own notifications) may still show its empty state to that role; an
empty list discloses nothing an empty read does not.

A single admin across products needs a role every product may name (showcase gap 2's platform role);
that is backend work, WG-5474 below.

**Tenant layouts and roles.** The overlay is one layout with audience conditions, as products are. An
overlay may narrow a product element's audience, and may widen a product screen's declared audience
only within the union of its content's data audiences (the content check runs on the result), so a
tenant can open the moderation queue to a new role that already reads the messages, and cannot make a
screen public whose data no anonymous visitor may read.

**Preview as a role.** A builder may ask for the manifest, the parts and the view reads **as a set of
roles** (`preview-roles` on the request, or "anonymous"). The gateway serves the interface class of a
principal holding exactly those roles and `public`, with no identity: conditions that mention the
principal evaluate as for a principal with no records of its own. Data reads in a preview return only
what both the builder and that role set may read, so a preview never shows a builder more than they
may already read. Writes are refused in preview. Preview replies are never cached shared. Previewing
as a named person is not offered (it would be impersonation); see "Open questions".

**Role changes take effect live**, through the access floor: see "Serving and caching".

## Tenant-arranged layouts

The tenant's arrangement is an **overlay** element (`ui-layout`), one per screen it changes, owned by
the tenant, never by a product. It may: reorder widgets, move a widget to another region the template
allows, hide a widget, change a widget's props within its pattern's schema, add widgets from the
catalogue bound to the same screen's view sections, forms and actions, and set audiences as above. It
may not bind to data the screen's view does not read; new data needs a product step.

- The kernel checks the screen as the overlay leaves it, with the same check as a product's screen.
- An upgrade that removes or renames something an overlay names is refused, naming each overlay
  entry; the fix is to remove or rebind the entry first (an agent can do both in one plan). This is
  the removal guard of `incremental_valid`, with overlays among the unchanged screens.
- An owner edits overlays on a platform screen, the **layout editor** (a builder-only screen of the
  platform, drawn with the same patterns), or an agent writes them; both go through `define-ui-element`.
- Record-driven pages (the showcase's pages of widget records) stay content: a `page-canvas` pattern
  draws a list section of block records whose `kind` field chooses among a fixed set of content
  patterns (`markdown-block`, `media`, `link-list`); structure that binds data stays in overlays,
  where the kernel checks it.

## The catalogue

`specifications/ui-catalogue.jsonc`, read by specc (as `deploy.jsonc` is), has the sections
`version`, `tokens`, `contrastPairs`, `themes`, `primitives`, `patterns`, `templates`, `shell`,
`displayTypes`. Each entry of the last five holds:

```jsonc
{ "id": "data-table", "since": "1.0", "summary": "Rows and columns of a list section, with row actions, sorting and paging.",
  "binds": ["section:list"], "states": ["loading", "empty", "error", "ready", "placeholder"],
  "props": { /* JSON Schema of the props */ },
  "regions": ["main", "below"],
  "examples": [ { "name": "people", "props": { ... }, "data": { ... } } ],
  "refused": [ { "name": "column of a missing field", "props": { ... }, "code": "ui.binding.unknown-field" } ] }
```

- **Generated from it:** the kernel's validator tables (Rust, through specc into the model crates),
  the interpreter's tables (`web/src/generated/catalogue.js`), the token stylesheet of each preset,
  and the gallery's fixtures.
- **Served:** `GET /api/ui/catalogue` (public, named by its hash, cacheable) and the agent gateway's
  catalogue read (`catalogue-read`), with the JSON Schemas of the product file sections that hold
  interface (`screens`, `presentations`, `navigation`, `forms`, `actions`).
- **Checked:** every accepted example validates and every refused example is refused with its code,
  in the kernel and in specc; a refusal code is never added without a refused example (the same
  practice as `crates/spec-compiler/tests/fixtures/products`).
- **Versioned:** `version` is `major.minor`. Adding an optional prop, a pattern, a display type or a
  template is a minor change; removing, renaming or narrowing anything is a major one. A script
  (`scripts/catalogue-compat.mjs`) classifies a change by comparing with the previous version and
  refuses a narrowing labelled minor. Every composition records the major it was checked against;
  the web application draws the current major and the previous one, through a pure migration in the
  kernel tested on every example of the previous major; the kernel refuses a composition at a major it
  does not serve. Installed products keep working across a minor change with no action, and across a
  major one by the migration, until the next major after it.

## The agent build loop

### End to end

1. **Read.** The agent reads the catalogue (`GET /api/ui/catalogue`), the backend specification's
   product format (the types of each product section, as specc checks them), and the tenant's state:
   installed products and versions, the schema version, the current interface definition, and the
   source of the tenant's app product (the agent keeps it; `GET /api/products/<id>/source` returns
   the last one installed).
2. **Compose.** The agent writes or edits a **tenant app product**: a product file in the tenant's
   namespace, with objects, policies, queries, views, operations, presentations, screens, forms,
   actions and navigation. A tenant may have several (one per feature), each evolved independently.
3. **Validate before applying.** `POST /api/products/compile` (new) runs specc's layer rules and the
   catalogue checks on the product file and returns the bundle or the diagnostics. Then
   `upgrade-bundle` (CT-835) with `dry-run: true` and `expected-version` runs the kernel's checks and
   returns the projection: the changes and their number, entities created, changed and removed,
   records migrated, and the kernel's refusals.
4. **Apply.** `upgrade-bundle` (`install-bundle` the first time) with an idempotency key, the
   expected schema version, `"into": "draft"` (interface elements go to the draft bodies), and a
   `label`: one line saying what the step does ("Bind the message list to the latest messages"). The
   upgrade is one product change, one schema change, one commit: all or none.
5. **Watch.** The commit raises the schema version; the gateway sends the schema notice; every
   builder's browser reads the manifest of its class, fetches the parts whose hashes changed and
   redraws, keeping held parts until their replacements arrive (`acceptManifest`, so nothing is drawn
   empty). The builder's page shows the step's label in a build log (read from the product change
   history by version) and outlines the widgets the step changed for a moment (`dur-settle`; none
   under reduced motion).
6. **Refusals.** A refused compile, dry run or upgrade returns diagnostics the agent acts on (below).
   A stale `expected-version` (someone else changed the schema) returns `version-conflict` with the
   current version; the agent reads the state again and re-plans its step.
7. **Publish.** When the draft is at full fidelity the owner publishes it (or the agent proposes the
   publication and the owner approves it); members see the new screens at once, through the same
   schema notice.

**Authority.** The agent acts for a builder through the agent gateway with a `tenant-agent`
delegation that carries `bundle-install` limited to the products it created and `ui-build` (today's
delegation does not carry `bundle-install`; adding it is part of WG-5471). The owner's step-up is
taken when the delegation is granted, not at each step. Steps that change data in a way tenant policy
classes as risky (removing an object type or a field with records, a migration, a new operation or a
policy change) go through `propose-change` (CT-112) and wait for the owner's approval in the live
view; interface steps into the draft and additive data steps are not risky by default. Every step is recorded
with the agent and the person it acts for (G-25), and an interface step can be undone by applying
the previous version of the product as an upgrade.

### Diagnostics

Every refusal, from specc, the catalogue checks or the kernel, has one shape:

```jsonc
{ "code": "ui.binding.unknown-field", "severity": "error",
  "message": "The column \"author_name\" names no field of showcase_community__message.",
  "path": "/screens/2/regions/main/0/props/columns/1/field",
  "entity": "showcase_community__messages", "catalogue": "data-table@1.0",
  "hint": "Fields of showcase_community__message: title, body, author, channel, status, created_at." }
```

`path` is a JSON Pointer into the product file the agent sent (specc keeps the positions), so the
agent edits the exact place. The codes are a closed list in the catalogue with what to do about each:
`layer.*` (specc), `bundle.ui-*`, `ui.catalogue.*` (unknown entry, props against schema),
`ui.binding.*` (unknown section, field, form or action; type mismatch of a row argument),
`ui.audience.*` (wider than parent, empty for a least viewer), `ui.publish.*` (placeholder left),
`theme.contrast` (pair, ratio, minimum, suggestion), `schema.*` (the kernel's, today strings, given
codes), `install.too-many-changes` (the count and the cap), `version-conflict`, `upgrade.overlay-names`
(the overlay entries a removal would break).

### Interleaving wireframe, data and styling steps

Three kinds of step act on one product file, in any order that keeps each step valid; the only
ordering rule is that a binding needs the data it names. Building the showcase's message centre:

| Step | Kind | What changes | Changes in the step | What the owner sees |
|---|---|---|---|---|
| 1 | wireframe | screens `/messages` (list) and `/messages/:id` (detail) as drafts with placeholder widgets and samples; navigation entry | about 6 | Two draft screens in the neutral theme: labelled regions, a table skeleton with columns Title, Author, Posted, a comment thread skeleton |
| 2 | data | objects message, comment, reaction; policies; queries and views | about 60 | Nothing changes on screen; the build log says the data model is in |
| 3 | data | operations post, comment, react, hide; their invokers | about 10 | Nothing on screen; owner approves (new operations) |
| 4 | binding | the table bound to `messages`, the card to `message`, the thread to `comments` | 2 | Real messages in the table; the reaction placeholder still a skeleton |
| 5 | styling | presentations: person, datetime, status, rich text | 3 | Names instead of principals, "3 hours ago", status badges |
| 6 | binding | forms and row actions, prefill | about 8 | The post form, the react buttons on each row |
| 7 | styling | the tenant's theme seeds | 1 (configuration) | The tenant's colours and fonts |
| 8 | publish | draft bodies to published | 1 | Members see the message centre |

(The change counts are estimates of entity counts for this example, not measurements.)

### The 200-change cap

A step carries only what changed, so the loop works under the cap by construction: the dry run reports
the count, and a planner that finds a step over 200 splits it by dependency (object types, then
policies, queries and views, then operations, then presentations, then screens, then navigation), each
part a valid state. The first install of a large product (showcase gap 13) is answered by **staged
installs** (D-DS-16): `plan_install` splits a product over the cap into versions
`<version>-stage.1`, `-stage.2`, ... of the same product, each installed as an upgrade of the one
before, each at most 200 changes and each a complete installed version with its ownership record, so
every intermediate state is a valid installed product, an interrupted install resumes as an upgrade,
and the kernel's cap and the all-or-none rule of one product change stay as they are. Raising the cap
was weighed and not taken: it bounds the work and the journal entry of one commit, and nothing in the
agent loop needs it raised.

### Prior art

Exegra's agent interface (`C:\dev\exegra\docs\agent-interface.md`) is the model: one JSON envelope,
`apply` with `--expect-hash` and `--dry-run`, `admit` with a diff and every failing reason listed, and
diagnostics with entity ids and positions. The showcase's live driver (`tests/corpus/showcase/live.mjs`)
shows that every step can go through public routes as a browser would; the corpus loader's staged
install (`crates/corpus`) is D-DS-16 done by hand.

## Governance

- **The gallery** (`/design`, a builder-only platform screen) draws every catalogue example of every
  primitive and pattern, in every state, in every preset theme, in light and dark, at regular and
  compact density, from the catalogue's examples: the gallery is the catalogue, so a new entry is in
  it by construction.
- **Screenshot regression:** a Playwright spec takes one screenshot per example, theme, scheme and
  width (phone and desk) and compares with committed baselines (`toHaveScreenshot`, a stated pixel
  threshold); a baseline changes only in a commit that says why.
- **Accessibility:** axe (`@axe-core/playwright`, already a dependency) at the WCAG 2.2 A and AA tags
  on every gallery page, no violation allowed; keyboard scripts per behavioural primitive (menu,
  tabs, dialog, record picker, toast) check every key the table above lists.
- **Contrast:** the catalogue's pairs on every preset, at generation (`npm run generate` refuses a
  failing preset) and at theme definition (refused).
- **Versioning:** `scripts/catalogue-compat.mjs` in the gate; the migration of the previous major
  tested on all its examples.

## Acceptance target: the showcase re-expressed

The four showcase products (`identity`, `community`, `pages`, `admin`) are rewritten as compositions,
presentations and navigation, with no JavaScript, CSS or HTML in `tests/corpus/showcase` beyond the
drivers (`live.mjs` and the agent build script), and built by the agent build script from wireframes
to full fidelity through the public API. It looks and behaves like a finished product when all of
these pass:

1. **Screenshots.** `tests/e2e/showcase.spec.js` visits every showcase screen as the tenant admin, a
   member and, for public screens, an anonymous visitor, in the neutral base and the Fieldnotes theme,
   light and dark, at phone and desk widths, and compares with baselines the owner has accepted.
2. **Accessibility.** axe at WCAG 2.2 AA on every one of those pages, no violation.
3. **No raw values.** On every page, no text matches an epoch (`\b1\d{12}\b`), a principal
   (`\buser:`), a JSON object (`{"`), or a record id outside a `code` display; no form has a field that
   asks for a record number.
4. **Roles.** A member's navigation has no admin entry; a member at `/admin` gets the same page, status
   and text as at an address nothing answers; an anonymous visitor is served only public screens; the
   moderator's message page has the hide action and the member's has none, in the served part and on
   the page.
5. **Live build.** The agent build script runs against a fresh stack; a screenshot after each step
   shows a valid page; members see nothing until the publish step.

"Looks finished" is the owner's judgement; the screenshots are the evidence for it, and the checks
above are the floor beneath it: each screen has one `h1` and a heading per region, no unlabelled
value, spacing from tokens only, worded empty states, and person, time and status drawn by display
type.

Showcase gaps that are backend work stay theirs (1, 3, 4, 7, 8, 11, 12, 14); the acceptance target
needs gap 2's platform admin role (WG-5474) and gap 6's reference coercion (in WG-5466).

## The specification part

`specifications/design-system.jsonc` is an area part (`AREA_PARTS` in `scripts/split-spec.mjs`: ids
that are `ds-` and a number, or start `d-ds-`; the existing behaviour-model ids `ds-save`,
`ds-open` and so on are not in it). It holds sixteen decisions: D-DS-1 to D-DS-6 are the owner's, decided
2026-11-22; D-DS-7 to D-DS-16 are proposals of this design, with a source saying so. It holds eleven
requirements, DS-001 to DS-011, milestone 7 (the agent plane, where the tenant interface definition's
INT-3256 and INT-3257 are), each stated of its Lean model and checked by the theorems that carry its
`Verifies:` line. Requirements whose checks are tests yet to be written are added with those tests,
as `docs/frontend.md` does, by the tasks below; a requirement may not name a check that does not exist,
because the gate's evidence check refuses a `checkedBy` that names no test (`gate-evidence.mjs`).

| Planned requirement | Its check | Added by |
|---|---|---|
| Every preset theme meets AA on every catalogue pair, in both schemes | generation check with a failing fixture | WG-5458 |
| A theme definition with a failing pair is refused naming the pair | substrate test | WG-5460 |
| Every accepted catalogue example validates and every refused example is refused with its code | kernel and specc tests | WG-5459 |
| A composition with an unknown pattern, field or binding is refused with its code and path | kernel test | WG-5464 |
| A member is never served a builder's draft, and a screen without a published body is not found | gateway test | WG-5464, WG-5465 |
| A part is served only to a class whose manifest lists it | gateway test | WG-5465 |
| Non-anonymous parts are sent `no-store` and never written to the shared Cache API | gateway test, Playwright | WG-5465 |
| A row action's affordance equals the outcome of invoking it, for every row of the fixture | kernel differential of `preflight` and `decide` | WG-5466 |
| No page of the showcase shows an epoch, a principal or a JSON object | Playwright | WG-5475 |
| axe finds no violation on any gallery example or showcase page | Playwright | WG-5473, WG-5475 |

## What is proved

`lean/Strata/UI/Composition.lean`, `Contrast.lean` and `Visibility.lean`, imported from
`lean/Strata.lean` and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom.

| Property | Theorem | Requirement |
|---|---|---|
| The check is exactly validity; a checked composition names only bindings that exist | `check_iff`, `checked_bindings_resolve` | DS-001 |
| Valid at full fidelity: every data widget bound to something that exists | `full_valid_bound` | DS-002 |
| Refinement keeps validity, one placeholder fewer per binding, valid at every step, ends at full fidelity | `bindAt_valid`, `bindAt_unbound`, `refine_valid`, `refine_full` | DS-003 |
| Incremental checking with the removal guard keeps every screen valid; without the guard it fails | `incremental_valid`, `unguarded_removal_breaks` | DS-004 |
| The derived on-colour meets AA on any surface; a ramp search ending at it always succeeds | `onColour_meets_aa`, `search_finds` | DS-005 |
| The theme check is exactly the declared pairs | `checkTheme_iff` | DS-006 |
| Everything served comes from a node the viewer sees, under visible ancestors | `render_sound` | DS-007 |
| A hidden node leaves no trace | `hidden_leaves_no_trace` | DS-008 |
| An action's audience is exactly the kernel's invoke admission | `audOf_sees` | DS-009 |
| A data audience is exactly the roles with a Read grant | `dataAud_sees` | DS-010 |
| A checked screen is never served empty | `content_check_nonempty`, `checked_screen_renders_content` | DS-011 |

**Not proved, and what carries it.**

- The Rust and JavaScript against the models: the kernel's check against `Composition.check`, the
  gateway's filtering against `Visibility.render`, `preflight` against `decide`. Differential tests
  in WG-5464, WG-5465 and WG-5466.
- The luminance computation and its rounding: the model is in fixed point; the implementation must
  round the lighter colour down and the darker up (WG-5460, with boundary cases).
- `audOf_sees` is the invoker stage only; per-row conditions are the `preflight` differential.
- `dataAud_sees` models Read grants without conditions (an upper bound): a conditional policy puts a
  role in the data audience even when its condition will exclude every row, which is why a widget may
  show its empty state; the content check reasons on the same upper bound.
- The model's audiences are flat (a viewer's roles include its teams' roles and run-as roles, as
  `roles_of` gives them); the owner role's read-everything is the implementation's.
- The content check of the model runs over all of a node's children; the kernel runs it over the
  screen's data and action widgets only (static widgets removed), so the theorem gives a data or
  action widget, and a screen with none is outside the check.
- The per-class filtering is modelled as one tree; drafts are an audience (the builders) on the draft
  body, and are covered by `render_sound` as any other audience is.
- Contrast is necessary, not sufficient, for an accessible theme: focus visibility, target size and
  motion are checked by axe and the keyboard scripts, not proved.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5457 with testable acceptance criteria (Workgraph is the record).

1. (WG-5458) **Token layer and preset themes as catalogue data.** No dependency.
2. (WG-5459) **The catalogue format, generator and compatibility check.** Needs 1.
3. (WG-5460) **Theme seeds, derivation, contrast refusal and CSSOM delivery.** Needs 1.
4. (WG-5461) **Primitives: class contracts and behavioural custom elements.** Needs 1, 2.
5. (WG-5462) **Display types and presentation elements.** Needs 2.
6. (WG-5463) **Section shapes in parts, the people table and reference titles in view replies.** Needs 5.
7. (WG-5464) **Compositions: templates, regions, widgets, bindings, placeholders, drafts and
   publishing, checked by the kernel.** Needs 2, 5.
8. (WG-5465) **Visibility: audiences at every level, derived data audiences, the content check, per-class
   manifests and parts, the part guard, cache rules and live refetch.** Needs 7.
9. (WG-5466) **Action affordances from a shared `preflight`, row arguments, prefill, reference
   coercion.** Needs 7, 8.
10. (WG-5467) **Interpreter and pattern renderers.** Needs 4, 5, 6, 7.
11. (WG-5468) **App shell, navigation and the notification bell.** Needs 8, 10.
12. (WG-5469) **Tenant layout overlays and the layout editor.** Needs 8, 10.
13. (WG-5470) **Preview as a set of roles.** Needs 8.
14. (WG-5471) **Agent loop platform: compile endpoint, structured diagnostics, draft upgrades, step
    labels and build log, agent delegation.** Needs 7, 8.
15. (WG-5472) **Staged installs over the change cap.** No dependency.
16. (WG-5473) **Gallery, screenshot regression, axe and keyboard checks.** Needs 4, 10.
17. (WG-5474) **A platform admin role products may name (showcase gap 2, backend half).** No
    dependency.
18. (WG-5475) **The showcase re-expressed and built by an agent script: the acceptance target.**
    Needs 9, 11, 12, 13, 14, 15, 16, 17.

## Open questions

- Should a builder be able to preview as a named person, with that person's consent recorded, to check
  conditional policies (`record.recipient == principal`) that a role preview cannot show?
- Should a field's existence be hideable from readers of its record type when its read rule is
  per-record, at the cost of computing a per-viewer shape on every read?
- Should a draft be able to hold data-model changes that stay invisible to API callers until publish
  (a schema branch), so that drafts cover data as well as interface?
