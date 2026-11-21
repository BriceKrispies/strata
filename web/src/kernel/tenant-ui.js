// The interface definition of the tenant, read at run time (D-8250).
//
// A product installs its screens, forms, actions, live bindings and error messages into the tenant
// schema; the gateway serves them as a manifest (`GET /api/ui`, named by the hash of its list of
// parts) and parts (`GET /api/ui/parts/<hash>`, each named by the hash of its content). This module
// is the pure part of the kernel that works with them:
//
// - the state of the definition the page holds, and what to fetch when a manifest arrives;
// - the router of tenant screens, consulted when no literal route of a platform screen answers
//   an address (INT-3256);
// - the interpreter: the page model of a tenant screen, built from its part and the view model
//   with no code of the product (INT-3257), drawn by `views/tenant.js`;
// - the live bindings and the error messages of a tenant screen.
//
// What the interpreter draws is fixed: the layouts list, detail, editor and settings; the controls
// text, textarea, password, checkbox, select, radio, date and number; an action as a button whose
// arguments read form.<field> or route.<param>. Anything else a product file declares is refused by
// specc before it is installed (`bundle.ui-unsupported`), so a new kind is added here and to the
// element's type in the specification together.
// @ts-check

/**
 * @typedef {{ kind: string, id: string, hash: string, route?: string, view?: string, title?: string, layout?: string, auth?: string, query?: string[] }} ManifestEntry
 * @typedef {{ hash: string, schema_version: number, parts: ManifestEntry[] }} Manifest
 * @typedef {{ manifest: Manifest | null, parts: Record<string, any>, asked: Record<string, number>, held?: Record<string, any> }} TenantUi
 */

/** The definition a page holds before the manifest arrives. @returns {TenantUi} */
export function uiInit() {
  return { manifest: null, parts: {}, asked: {} };
}

/** @param {TenantUi | undefined} ui @returns {ManifestEntry[]} */
export function tenantScreens(ui) {
  return (ui?.manifest?.parts ?? []).filter((entry) => entry.kind === 'screen');
}

/**
 * Take a manifest in. The parts still listed are kept, since a hash names its content; the others
 * are dropped.
 * @param {TenantUi} ui @param {Manifest} manifest
 * @returns {TenantUi}
 */
export function acceptManifest(ui, manifest) {
  const listed = new Set((manifest?.parts ?? []).map((entry) => entry.hash));
  /** @type {Record<string, any>} */
  const parts = {};
  for (const [hash, part] of Object.entries(ui.parts)) if (listed.has(hash)) parts[hash] = part;
  /** @type {Record<string, number>} */
  const asked = {};
  for (const [hash, id] of Object.entries(ui.asked)) if (listed.has(hash)) asked[hash] = id;
  // A part the new manifest replaces stays in `held`, by the screen or errors id it was for, and is
  // used until its replacement arrives: the screen is never drawn empty during an upgrade.
  /** @type {Record<string, any>} */
  const held = {};
  const old = new Map((ui.manifest?.parts ?? []).map((entry) => [heldKey(entry), entry]));
  for (const entry of manifest?.parts ?? []) {
    if (entry.kind !== 'screen' && entry.kind !== 'errors') continue;
    if (entry.hash in parts) continue;
    const key = heldKey(entry);
    const before = old.get(key);
    const part = (before && ui.parts[before.hash]) ?? ui.held?.[key];
    if (part) held[key] = part;
  }
  return { manifest, parts, asked, held };
}

/** @param {ManifestEntry} entry */
function heldKey(entry) {
  return `${entry.kind}:${entry.id}`;
}

/**
 * Whether a manifest is older than the one the page holds: a lower schema version.
 * @param {TenantUi} ui @param {Manifest} manifest
 */
export function manifestIsOlder(ui, manifest) {
  const held = ui.manifest?.schema_version;
  return typeof held === 'number' && typeof manifest?.schema_version === 'number' && manifest.schema_version < held;
}

/**
 * The drafts a part no longer has a field for. A form the part dropped, or a field of a kept form
 * that was removed, with a value the person typed, is lost: the value is taken out of the form's
 * state, and the loss is named on the form (`notice`) when the form is still there.
 * @param {Record<string, TenantForm>} forms @param {any} part
 * @returns {{ forms: Record<string, TenantForm>, lost: string[] }}
 */
export function lostDrafts(forms, part) {
  /** @type {Record<string, TenantForm>} */
  const next = {};
  /** @type {string[]} */
  const lost = [];
  const typed = (/** @type {any} */ v) => v === true || (typeof v === 'string' && v !== '');
  for (const [id, held] of Object.entries(forms)) {
    const form = part?.forms?.[id];
    const names = new Set((form?.fields ?? []).map((/** @type {any} */ f) => f.name));
    const gone = Object.entries(held.values).filter(([name, value]) => (!form || !names.has(name)) && typed(value)).map(([name]) => name);
    if (!form) {
      if (gone.length) lost.push(`The ${id} form was removed; what you had entered in ${gone.join(', ')} was not saved.`);
      continue;
    }
    if (!gone.length) {
      next[id] = held;
      continue;
    }
    const text = `The ${gone.join(', ')} field${gone.length > 1 ? 's were' : ' was'} removed in an update; what you had entered there was not saved.`;
    lost.push(text);
    const values = Object.fromEntries(Object.entries(held.values).filter(([name]) => names.has(name)));
    next[id] = { ...held, values, notice: text };
  }
  return { forms: next, lost };
}

/** @param {TenantUi} ui @param {string} hash @param {any} part @returns {TenantUi} */
export function acceptPart(ui, hash, part) {
  const { [hash]: _done, ...asked } = ui.asked;
  return { ...ui, parts: { ...ui.parts, [hash]: part }, asked };
}

/**
 * The hashes of the parts a screen needs that the page neither holds nor has asked for: the
 * screen's own part, and the part of the error messages.
 * @param {TenantUi} ui @param {string | null} screen
 * @returns {string[]}
 */
export function partsToFetch(ui, screen) {
  const entries = (ui.manifest?.parts ?? []).filter((entry) => entry.kind === 'errors' || (screen !== null && entry.kind === 'screen' && entry.id === screen));
  return entries.map((entry) => entry.hash).filter((hash) => !(hash in ui.parts) && !(hash in ui.asked));
}

/**
 * The tenant screen that answers an address, with its route and query parameters, or null. Of the
 * routes that answer the path, the one with the most literal segments wins, as for the platform's
 * own routes; only the screen's own query parameters are kept.
 * @param {TenantUi | undefined} ui @param {string} path
 * @param {Record<string, string>} [given]
 * @returns {{ screen: string, entry: ManifestEntry, params: Record<string, string>, query: Record<string, string> } | null}
 */
export function matchTenantRoute(ui, path, given = {}) {
  const parts = path === '/' ? [] : path.replace(/^\//, '').split('/');
  /** @type {{ entry: ManifestEntry, params: Record<string, string>, literals: number } | null} */
  let best = null;
  for (const entry of tenantScreens(ui)) {
    const route = entry.route ?? '';
    const segments = route === '/' ? [] : route.replace(/^\//, '').split('/');
    if (segments.length !== parts.length) continue;
    /** @type {Record<string, string>} */
    const params = {};
    let literals = 0;
    let ok = true;
    for (let i = 0; i < parts.length && ok; i += 1) {
      const pattern = segments[i];
      if (pattern.startsWith(':')) {
        try {
          params[pattern.slice(1)] = decodeURIComponent(parts[i]);
        } catch {
          ok = false;
        }
        if (parts[i] === '') ok = false;
      } else if (pattern === parts[i]) {
        literals += 1;
      } else {
        ok = false;
      }
    }
    if (ok && (!best || literals > best.literals)) best = { entry, params, literals };
  }
  if (!best) return null;
  /** @type {Record<string, string>} */
  const query = {};
  for (const name of best.entry.query ?? []) {
    if (typeof given[name] === 'string') query[name] = given[name];
  }
  return { screen: best.entry.id, entry: best.entry, params: best.params, query };
}

/**
 * The address of a tenant screen, with these parameters; null when the tenant has no such screen.
 * @param {TenantUi | undefined} ui @param {string} screen @param {Record<string, unknown>} [params]
 * @returns {string | null}
 */
export function tenantHref(ui, screen, params = {}) {
  const entry = tenantScreens(ui).find((e) => e.id === screen);
  if (!entry?.route) return null;
  const segments = entry.route === '/' ? [] : entry.route.replace(/^\//, '').split('/');
  return `/${segments.map((s) => (s.startsWith(':') ? encodeURIComponent(String(params[s.slice(1)] ?? '')) : s)).join('/')}`;
}

/**
 * The tenant screen that shows a view, or null.
 * @param {TenantUi | undefined} ui @param {string | null | undefined} view
 * @returns {ManifestEntry | null}
 */
export function tenantScreenOfView(ui, view) {
  if (!view) return null;
  return tenantScreens(ui).find((entry) => entry.view === view) ?? null;
}

/** The part a manifest entry names, when the page holds it. @param {TenantUi | undefined} ui @param {ManifestEntry | null} entry */
export function partOf(ui, entry) {
  return entry ? ui?.parts[entry.hash] ?? ui?.held?.[heldKey(entry)] ?? null : null;
}

/** The part of the error messages, when the tenant has one and the page holds it. @param {TenantUi | undefined} ui */
export function errorsPart(ui) {
  const entry = (ui?.manifest?.parts ?? []).find((e) => e.kind === 'errors');
  return entry ? ui?.parts[entry.hash] ?? ui?.held?.[heldKey(entry)] ?? null : null;
}

/** @param {any} value @param {string} path */
function valueAt(value, path) {
  let at = value;
  for (const key of path.split('.')) {
    if (at === null || typeof at !== 'object') return '';
    at = at[key];
  }
  return typeof at === 'string' || typeof at === 'number' ? String(at) : '';
}

/**
 * A title as a screen states it: the text, with each `${a.b.c}` read from the view model.
 * @param {string | undefined} template @param {any} model
 */
export function tenantTitle(template, model) {
  return (template ?? '').replace(/\$\{([^}]*)\}/g, (_, path) => valueAt(model, path));
}

/** The value of a field as a page shows it. @param {any} value */
function shown(value) {
  if (value === null || value === undefined) return '';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** A projected record as the interpreter draws it: each field by name, in name order. @param {any} row */
function recordOf(row) {
  const fields = Object.keys(row?.fields ?? {}).sort().map((name) => ({ name, value: shown(row.fields[name]) }));
  return { id: row?.id ?? null, fields };
}

/**
 * The sections of a view model, as the interpreter draws them: a list section as its rows, a
 * single section as one record. The aggregate of a section is drawn as its value.
 * @param {any} model
 */
function sectionsOf(model) {
  const data = model?.data ?? {};
  return Object.keys(data).sort().map((name) => {
    const value = data[name];
    if (Array.isArray(value)) return { name, single: false, rows: value.map(recordOf), more: typeof model.next?.[name] === 'string' };
    if (value && typeof value === 'object' && 'fields' in value) return { name, single: true, record: recordOf(value) };
    return { name, single: true, record: { id: null, fields: [{ name, value: shown(value) }] } };
  });
}

/**
 * The state of a form of a tenant screen the page keeps: its values, whether it is waiting for a
 * reply, and the refusal it shows.
 * @typedef {{ values: Record<string, string | boolean>, busy: boolean, error: string, notice?: string, errors: Record<string, string> }} TenantForm
 */

/** The values a form opens with. @param {any} form @returns {TenantForm} */
export function formInit(form) {
  /** @type {Record<string, string | boolean>} */
  const values = {};
  for (const field of form?.fields ?? []) values[field.name] = field.control === 'checkbox' ? false : '';
  return { values, busy: false, error: '', errors: {} };
}

/**
 * The inputs of the operation a form invokes, from its values: each field fills its input, trimmed
 * when it says so; a number field that holds a number is sent as one.
 * @param {any} form @param {Record<string, string | boolean>} values
 */
export function formInputs(form, values) {
  /** @type {Record<string, any>} */
  const inputs = {};
  for (const field of form?.fields ?? []) {
    /** @type {any} */
    let value = values[field.name] ?? (field.control === 'checkbox' ? false : '');
    if (typeof value === 'string' && field.trim) value = value.trim();
    if (field.control === 'number' && typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) value = Number(value);
    inputs[field.input] = value;
  }
  return inputs;
}

/**
 * The inputs of the operation an action invokes: each argument read from form.<field> of the form
 * it names, or route.<param>.
 * @param {any} action @param {Record<string, string | boolean>} formValues @param {Record<string, string>} route
 */
export function actionInputs(action, formValues, route) {
  /** @type {Record<string, any>} */
  const inputs = {};
  for (const [input, source] of Object.entries(action?.arguments ?? {})) {
    const [scope, name] = String(source).split('.', 2);
    inputs[input] = scope === 'form' ? formValues[name] ?? '' : scope === 'route' ? route[name] ?? '' : '';
  }
  return inputs;
}

/**
 * Whether a live binding of the screen refreshes it for this event: a notice whose kind it names,
 * or one about an object type the view depends on when it says so; or the end of the session.
 * @param {any} part @param {'notice' | 'session-ended'} event
 * @param {{ kind?: string, object?: string }} notice @param {any} model
 */
export function liveRefreshes(part, event, notice, model) {
  const dependsOn = Array.isArray(model?.depends_on) ? model.depends_on : [];
  return Object.values(part?.live ?? {}).some((live) => {
    if (live.event !== event) return false;
    if (event === 'session-ended') return true;
    if ((live.kinds ?? []).includes(notice.kind)) return true;
    return live.objects === true && typeof notice.object === 'string' && dependsOn.includes(notice.object);
  });
}

/** The kind of a failed check (`required` of `required:post.title`) and the field it names. @param {{ rule?: string }} check */
function checkParts(check) {
  const rule = String(check?.rule ?? '');
  const [kind, rest = ''] = rule.split(':', 2);
  const field = rest.includes('.') ? rest.slice(rest.lastIndexOf('.') + 1) : rest;
  return { kind, field };
}

/**
 * The message of the first error message that words this refusal: those that apply to the owner
 * first, then those for all; `kinds` empty words every kind. Text is filled in with
 * ${check.detail}, ${check.statement} or ${error.message}.
 * @param {any} errors The part of the error messages. @param {string} owner A form or an action.
 * @param {'check' | 'error'} on @param {string} kind @param {Record<string, string>} fill
 */
function wording(errors, owner, on, kind, fill) {
  const messages = Object.values(errors?.messages ?? {}).filter((m) => m.on === on && ((m.kinds ?? []).length === 0 || m.kinds.includes(kind)));
  const own = messages.filter((m) => (m.applies_to ?? []).includes(owner));
  const all = messages.filter((m) => (m.applies_to ?? []).length === 0);
  const chosen = own[0] ?? all[0];
  if (!chosen) return null;
  const text = String(chosen.text).replace(/\$\{([^}]*)\}/g, (_, name) => fill[name] ?? '');
  return { text: text || chosen.fallback || '', field: chosen.field ?? null };
}

/**
 * What a refusal of a form or an action says, in the tenant's wording: a message for the form as a
 * whole and, for each failed check that names a field of the form, a message on that field.
 * @param {any} errors @param {string} owner @param {any} form The form, when the owner is one.
 * @param {{ code?: string, message?: string, failed?: { rule?: string, detail?: string, passed?: boolean }[] }} error
 */
export function explainRefusal(errors, owner, form, error) {
  /** @type {Record<string, string>} */
  const fields = {};
  const names = new Set((form?.fields ?? []).map((/** @type {any} */ f) => f.name));
  for (const check of (error?.failed ?? []).filter((c) => c.passed !== true)) {
    const { kind, field } = checkParts(check);
    const detail = String(check.detail ?? '');
    const said = wording(errors, owner, 'check', kind, { 'check.detail': detail, 'check.statement': detail.replace(/\s*\([^)]*\)\s*$/, '') });
    const target = said?.field === 'prefix' ? [...names].find((n) => field.startsWith(n)) : field;
    if (target && names.has(target) && !(target in fields)) fields[target] = said?.text || detail;
  }
  const general = wording(errors, owner, 'error', String(error?.code ?? ''), { 'error.message': String(error?.message ?? '') });
  const message = general?.text || String(error?.message ?? 'That did not work.');
  return { message, fields };
}

/**
 * The page model of a tenant screen: what `views/tenant.js` draws. Built from the screen's part and
 * the view model alone (INT-3257).
 * @param {ManifestEntry} entry @param {any} part @param {any} model
 * @param {{ forms: Record<string, TenantForm>, confirm: string | null, signedIn: boolean }} context
 */
export function tenantPage(entry, part, model, context) {
  const forms = Object.entries(part?.forms ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([id, form]) => {
    const held = context.forms[id] ?? formInit(form);
    return {
      id,
      submitLabel: form.submit_label,
      busy: held.busy,
      error: held.error,
      notice: held.notice ?? '',
      fields: (form.fields ?? []).map((/** @type {any} */ field) => ({
        name: field.name,
        label: field.label,
        control: field.control,
        options: field.options ?? [],
        value: held.values[field.name] ?? (field.control === 'checkbox' ? false : ''),
        error: held.errors[field.name] ?? '',
      })),
    };
  });
  const actions = Object.entries(part?.actions ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([id, action]) => ({
    id,
    label: action.label,
    confirm: action.confirm ?? null,
    armed: context.confirm === id,
  }));
  const layout = entry.layout ?? part?.screen?.kind ?? 'list';
  const title = tenantTitle(entry.title, model) || model?.title || '';
  return {
    kind: 'tenant',
    screen: entry.id,
    layout,
    pageTitle: title,
    heading: title,
    formsFirst: layout === 'editor' || layout === 'settings',
    sections: sectionsOf(model),
    forms,
    actions,
  };
}
