// The frontend kernel's transition function. Pure: no fetch, no socket, no clock, no DOM.
// @ts-check
//
// What the specification states about the pages is generated (web/src/generated): the routes,
// the requests and replies of the contracts, the forms, the actions, the live bindings and the
// client machines. This file holds the kernel state and wires those modules to the messages the
// worker delivers and the effects it performs. The lab keeps its own hand-written logic (lab.js).

import * as actions from '../generated/actions.js';
import * as contracts from '../generated/contracts.js';
import * as refusals from '../generated/errors.js';
import * as forms from '../generated/forms.js';
import * as live from '../generated/live.js';
import { matchRoute, route as routeOf } from '../generated/routes.js';
import { enterSections, rowAction as rowButton, rowActionAsked as rowButtonAsked, screenOfView } from '../generated/screens.js';
import { factorsEnter, factorsInit, factorsInput, factorsRequireStepUp, factorsUpdate, factorsWaiting, isStepUp } from './factors.js';
import { invitesInit, invitesInput, invitesOpened, invitesUpdate } from './invites.js';
import { labEnter, labInit, labUpdate } from './lab.js';
import { linksInit, linksInput, linksLand, linksOpened, linksUpdate } from './links.js';
import { oidcEnter, oidcInit, oidcLand, oidcOpened, oidcUpdate } from './oidc.js';
import { passkeysEnter, passkeysInit, passkeysUpdate } from './passkeys.js';
import { checkRecord, disagreement, draftOf } from './record-rules.js';
import { sessionsEnter, sessionsInit, sessionsUpdate } from './sessions.js';
import {
  acceptManifest,
  acceptPart,
  actionInputs,
  errorsPart,
  explainRefusal,
  formInit,
  formInputs,
  liveRefreshes,
  matchTenantRoute,
  partOf,
  partsToFetch,
  tenantHref,
  tenantScreenOfView,
  uiInit,
} from './tenant-ui.js';
import {
  busy,
  liveInit,
  liveStep,
  liveTimer,
  sessionInit,
  sessionRead,
  sessionStep,
  submitInit,
  submitStep,
} from './machines.js';

/** @typedef {import('./types.js').Message} Message */
/** @typedef {import('./types.js').Effect} Effect */
/** @typedef {{ state: any, effects: Effect[] }} Result */

const THEME_MODES = ['auto', 'light', 'dark'];
const LAB_SCREEN = 'screen-lab';
const SIGN_IN = 'form-sign-in';
const SETTINGS = 'form-settings';
// The object type the editor writes: what the server tells the page of it rides in the editor's view.
const EDITOR_OBJECT = 'post';

// Reads whose reply is applied only when it answers the latest request of its kind, as `view`
// is: an older reply that arrives late must not overwrite a newer one (or a newer sign-in).
const LATEST_ONLY = ['session', 'sessions.list', 'passkeys.list', 'factors.summary', 'lab.overview', 'ui.manifest'];
// Requests made on behalf of the person signed in. A reply to one that was made before the
// user-scoped state was cleared (sign-out, session end, account switch) belongs to someone else.
const USER_KIND_PREFIXES = ['sessions.', 'lab.', 'editor.', 'post.', 'settings.', 'tenant.'];
const USER_KINDS = [
  'passkeys.list', 'passkeys.register_start', 'passkeys.register_finish', 'passkeys.remove',
  'factors.summary', 'factors.enrol_start', 'factors.enrol_finish', 'factors.remove_totp', 'factors.new_codes', 'factors.stepup',
];
/** Refusals of the socket upgrade (the server reachable, the connection refused) after which live updates stop. */
const WS_REFUSAL_LIMIT = 5;

/** @param {string} kind */
function isUserKind(kind) {
  return USER_KINDS.includes(kind) || USER_KIND_PREFIXES.some((prefix) => kind.startsWith(prefix));
}

export function initialState() {
  return {
    ready: false,
    route: { path: '/', query: {} },
    /** @type {import('./types.js').Session | null} */
    session: null,
    sessionFlow: sessionInit(),
    site: { name: '', tagline: '' },
    theme: { mode: 'auto', systemDark: false, accent: 'moss', paper: 'linen' },
    // `next` holds, for each list section with more rows, the cursor of its next page; `more` is the
    // id of the request that loads one (0 when none is).
    view: { status: 'loading', model: null, error: null, request: 0, next: {}, more: 0 },
    ws: liveInit(),
    forms: {
      login: { values: forms.initialValues(SIGN_IN, {}), busy: false, error: '', flow: submitInit() },
      editor: null,
      settings: null,
    },
    optimistic: {},
    confirmDelete: null,
    // The interface definition of the tenant (D-8250): its manifest and the parts the page holds.
    ui: uiInit(),
    // The forms of the tenant screen on show, by id, and the action whose confirmation is armed.
    tenant: { forms: /** @type {Record<string, import('./tenant-ui.js').TenantForm>} */ ({}), confirm: /** @type {string | null} */ (null) },
    lab: labInit(),
    sessions: sessionsInit(),
    passkeys: passkeysInit(),
    factors: factorsInit(),
    links: linksInit(),
    invites: invitesInit(),
    oidc: oidcInit(),
    /** @type {{ id: number, kind: string, text: string }[]} */
    toasts: [],
    announce: '',
    menuOpen: false,
    nextId: 1,
    pending: {},
    /** The id of the latest request of each kind, for the kinds that apply only their latest reply. */
    latest: /** @type {Record<string, number>} */ ({}),
    /** Counts the clearings of user-scoped state; a request remembers the one it was made in. */
    generation: 0,
    /** Refusals of the socket upgrade counted since the socket last opened, and whether live updates gave up. */
    wsRefusals: 0,
    wsStopped: false,
    focus: { target: '', token: 0 },
  };
}

/** @param {string} href */
export function parseHref(href) {
  const [rawPath, rawQuery = ''] = href.split('#')[0].split('?');
  /** @type {Record<string, string>} */
  const query = {};
  for (const pair of rawQuery.split('&')) {
    if (!pair) continue;
    const [name, value = ''] = pair.split('=');
    try {
      query[decodeURIComponent(name)] = decodeURIComponent(value.replace(/\+/g, ' '));
    } catch {
      // A malformed escape is dropped rather than guessed at.
    }
  }
  const path = rawPath.length > 1 ? rawPath.replace(/\/+$/, '') : '/';
  return { path: path || '/', query };
}

/** @param {{ path: string, query: Record<string, string> }} route */
export function hrefOf(route) {
  const pairs = Object.entries(route.query).map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`);
  return pairs.length ? `${route.path}?${pairs.join('&')}` : route.path;
}

/** Turn a title into a slug: lower-case letters and digits joined by single hyphens. */
export const slugify = forms.slug;

/**
 * Register a request and return the state and its effect.
 * @param {any} state @param {string} kind
 * @param {{ method?: 'GET' | 'POST', path: string, body?: any, write?: boolean, idempotencyKey?: string, meta?: any }} spec
 * @param {Message} msg
 * @returns {[any, Effect]}
 */
export function request(state, kind, spec, msg) {
  const id = state.nextId;
  const correlation = `c-${id}-${Math.floor(msg.rand * 0xffffff).toString(16).padStart(6, '0')}`;
  const next = {
    ...state,
    nextId: id + 1,
    pending: { ...state.pending, [id]: { kind, correlation, generation: state.generation ?? 0, ...(spec.meta ?? {}) } },
    latest: { ...(state.latest ?? {}), [kind]: id },
  };
  return [
    next,
    {
      type: 'http',
      id,
      method: spec.method ?? 'GET',
      path: spec.path,
      body: spec.body,
      write: spec.write ?? false,
      csrf: spec.write ? state.session?.csrf_token : undefined,
      idempotencyKey: spec.idempotencyKey,
      correlation,
    },
  ];
}

/**
 * A key for one user action that creates something, made once and sent with every attempt of
 * that action, so that a retry after a lost reply creates one record (D-6595). Time and chance
 * arrive on the message, as everywhere in the kernel.
 * @param {any} state @param {Message} msg
 */
function newKey(state, msg) {
  return `k-${state.nextId}-${Math.floor(msg.rand * 0xffffffff).toString(16).padStart(8, '0')}-${Math.floor(msg.at).toString(36)}`;
}

/**
 * Register the call of a contract, as the generated request builder describes it.
 * @param {any} state @param {string} kind @param {string} contract
 * @param {Record<string, any>} inputs @param {Message} msg @param {any} [meta]
 * @returns {[any, Effect]}
 */
function call(state, kind, contract, inputs, msg, meta = {}) {
  const req = contracts.request(contract, inputs, state.route.path);
  // A create carries a key. A retry of the same create (`meta.retryKey`, the key of an attempt
  // whose outcome was not learned) reuses it when the request is the same; any other request has
  // another key, since a key with other input is refused.
  const body = req.write && req.body?.op === 'create' ? JSON.stringify(req.body) : null;
  const key = body === null ? undefined : meta.retryKey?.body === body ? meta.retryKey.key : newKey(state, msg);
  const sent = key === undefined ? {} : { sent: { key, body } };
  return request(state, kind, { ...req, idempotencyKey: key, meta: { contract, inputs, ...meta, ...sent } }, msg);
}

function toast(state, kind, text) {
  const id = state.nextId;
  return {
    ...state,
    nextId: id + 1,
    toasts: [...state.toasts.slice(-2), { id, kind, text }],
    announce: text,
  };
}

/**
 * The screen the current address opens: a platform screen whose route answers it literally, else
 * a screen of the tenant's interface definition (INT-3256), else the platform's catch-all.
 * `tenant` is the manifest entry of a tenant screen, null for a platform one.
 * @returns {{ screen: string, params: Record<string, string>, query: Record<string, string>, tenant: import('./tenant-ui.js').ManifestEntry | null } | null}
 */
function screenOf(state) {
  const platform = matchRoute(state.route.path, state.route.query);
  if (platform && !routeOf(platform.screen)?.catchAll) return { ...platform, tenant: null };
  const tenant = matchTenantRoute(state.ui, state.route.path, state.route.query);
  if (tenant) return { screen: tenant.screen, params: tenant.params, query: tenant.query, tenant: tenant.entry };
  return platform ? { ...platform, tenant: null } : null;
}

/** The tenant screen whose view is on show, with its part when the page holds it. */
function tenantOnShow(state) {
  const entry = tenantScreenOfView(state.ui, state.view.model?.view);
  return { entry, part: partOf(state.ui, entry) };
}

/**
 * Ask for the parts of the interface definition a tenant screen needs that the page neither holds
 * nor has asked for. Each part is fetched by its hash, once.
 * @param {any} state @param {Message} msg @param {string | null} screen
 * @returns {Result}
 */
function fetchParts(state, msg, screen) {
  /** @type {Effect[]} */
  const effects = [];
  let next = state;
  for (const hash of partsToFetch(state.ui, screen)) {
    const [asked, effect] = request(next, 'ui.part', { path: `/api/ui/parts/${hash}`, meta: { hash } }, msg);
    next = { ...asked, ui: { ...asked.ui, asked: { ...asked.ui.asked, [hash]: effect.type === 'http' ? effect.id : 0 } } };
    effects.push(effect);
  }
  return { state: next, effects };
}

/**
 * Whether the page should read the manifest of the tenant's interface definition now: it holds
 * none, has not asked, and the address is one no literal route of a platform screen answers. A
 * page of the platform's own screens never needs it.
 */
function wantsManifest(state) {
  if (state.ui.manifest || Object.values(state.pending).some((p) => p.kind === 'ui.manifest')) return false;
  const platform = matchRoute(state.route.path, state.route.query);
  return !platform || routeOf(platform.screen)?.catchAll === true;
}

/** Ask for the manifest of the tenant's interface definition (INT-3258). @returns {Result} */
function fetchManifest(state, msg) {
  const [next, effect] = request(state, 'ui.manifest', { path: '/api/ui' }, msg);
  return { state: next, effects: [effect] };
}

/** The principal as the generated policy checks read it. */
export function principalOf(session) {
  if (!session?.authenticated) return null;
  const roles = session.roles ?? [];
  return { id: session.principal, tenant: session.tenant, kind: 'member', roles };
}

/** The scope the generated expressions read on the current screen. */
function scopeOf(state, extra = {}) {
  const screen = screenOf(state);
  return {
    route: { ...(screen?.params ?? {}), ...(screen?.query ?? {}) },
    query: screen?.query ?? {},
    view: state.view.model,
    local: {},
    principal: principalOf(state.session),
    ...extra,
  };
}

/** Ask for the view of the current route. `quiet` keeps the current page on screen meanwhile. */
/** @returns {Result} */
function fetchView(state, msg, quiet) {
  const route = state.route;
  const screen = screenOf(state);
  const viewModel = screen ? routeOf(screen.screen)?.viewModel : null;
  // A tenant screen's view is the tenant view of its route, read with the screen's query parameters.
  const tenantQuery = screen?.tenant ? Object.entries(screen.query).map(([name, value]) => `&${encodeURIComponent(name)}=${encodeURIComponent(value)}`).join('') : '';
  const [next, asked] = viewModel
    ? call(state, 'view', viewModel, { ...screen?.params, ...screen?.query }, msg, { href: hrefOf(route) })
    : request(state, 'view', { path: `/api/view?path=${encodeURIComponent(route.path)}${tenantQuery}`, meta: { href: hrefOf(route) } }, msg);
  // The editor asks what the kernel will check on an article, so it can refuse a draft first.
  const effect = screen && routeOf(screen.screen)?.kind === 'desk.editor' && asked.type === 'http' ? { ...asked, path: `${asked.path}&rules=${EDITOR_OBJECT}` } : asked;
  const view = quiet
    ? { ...state.view, request: effect.type === 'http' ? effect.id : 0 }
    : { status: 'loading', model: state.view.model, error: null, request: effect.type === 'http' ? effect.id : 0 };
  const parts = fetchParts({ ...next, view }, msg, screen?.tenant ? screen.screen : null);
  const manifest = wantsManifest(parts.state) ? fetchManifest(parts.state, msg) : { state: parts.state, effects: [] };
  return { state: manifest.state, effects: [effect, ...parts.effects, ...manifest.effects] };
}

/** Ask for the session again. */
function readSession(state, msg, meta = {}) {
  return call(state, 'session', 'read-session-view', {}, msg, meta);
}

/** True when the screen the address opens needs a signed-in person. */
function guardedScreen(state) {
  const screen = screenOf(state);
  if (screen?.tenant) return screen.tenant.auth === 'session';
  return routeOf(screen?.screen ?? '')?.auth === 'session';
}

/**
 * Forget everything that belongs to the person who was signed in: recovery codes, the enrolment
 * secret, the lists of sessions and passkeys, the Lab's overview and results, the editor and
 * settings forms with their drafts, and (on a guarded page) that person's view model. Replies to
 * requests made before this are dropped when they arrive. The caller reads the view again, since
 * a guarded page's model is gone.
 * @param {any} state
 */
function clearUserState(state) {
  const latest = { ...(state.latest ?? {}) };
  for (const kind of LATEST_ONLY) latest[kind] = 0;
  const login = state.forms.login;
  return {
    ...state,
    generation: (state.generation ?? 0) + 1,
    latest,
    sessions: sessionsInit(),
    passkeys: { ...passkeysInit(), supported: state.passkeys.supported },
    factors: factorsInit(),
    lab: { ...labInit(), timer: state.lab.timer + 1 },
    forms: { ...state.forms, login: { ...login, values: { ...login.values, password: '' } }, editor: null, settings: null },
    optimistic: {},
    confirmDelete: null,
    view: guardedScreen(state) ? { status: 'loading', model: null, error: null, request: 0 } : state.view,
  };
}

/** The form whose fields the editor shows: the new article's, or an existing one's. */
function editorForm(editor) {
  return editor.id == null ? 'form-new-article-save' : 'form-edit-article-save-draft';
}

/** The editor form for a view model, keeping unsaved edits to the same article. */
function editorFor(state, model) {
  const current = state.forms.editor;
  if (model.view === 'desk_new') {
    if (current?.key === 'new') return current;
    return {
      key: 'new',
      id: null,
      version: null,
      status: 'draft',
      values: forms.initialValues('form-new-article-save', scopeOf(state)),
      edited: [], dirty: false, busy: false, flow: submitInit(), errors: {}, formError: '',
      tab: 'write', preview: null, remoteChanged: false, rev: state.nextId,
    };
  }
  const post = model.data.post;
  if (!post) return null;
  if (current?.id === post.id && current.dirty) {
    // Someone else saved while we were typing: keep our text, remember theirs exists.
    return current.version === post.version ? current : { ...current, remoteChanged: true };
  }
  return {
    key: post.fields.slug,
    id: post.id,
    version: post.version,
    status: post.fields.status,
    values: forms.initialValues('form-edit-article-save-draft', scopeOf(state)),
    edited: [], dirty: false, busy: false, flow: submitInit(), errors: {}, formError: '',
    tab: current?.id === post.id ? current.tab : 'write',
    preview: current?.id === post.id ? current.preview : null,
    remoteChanged: false, rev: state.nextId,
  };
}

function settingsFor(state, model) {
  const site = model.data.site;
  const current = state.forms.settings;
  if (!site) return null;
  if (current?.dirty && current.id === site.id) return current;
  return {
    id: site.id,
    values: forms.initialValues(SETTINGS, scopeOf(state)),
    dirty: false, busy: false, flow: submitInit(), errors: {}, formError: '', rev: state.nextId,
  };
}

/** The view model as its contract types it; the lab's and an unknown page's are kept whole. */
function decodeView(state, body) {
  const screen = screenOf(state);
  const entry = screen ? routeOf(screen.screen) : null;
  if (!entry?.viewModel || entry.catchAll || entry.screen === LAB_SCREEN) return body;
  return contracts.decode(entry.viewModel, body);
}

/** Take a view model in: page data, site chrome, theme, and the forms that edit it. */
function acceptView(state, model, cursors = {}) {
  const site = model.data.site?.fields;
  let next = {
    ...state,
    ready: true,
    view: { status: 'ready', model, error: null, request: 0, next: cursors ?? {}, more: 0 },
    // A confirmation belongs to the view it was asked on; a view that replaces it disarms it.
    confirmDelete: null,
    site: site ? { name: site.name ?? '', tagline: site.tagline ?? '' } : state.site,
    tenant: model.view === state.view.model?.view ? state.tenant : { forms: {}, confirm: null },
    theme: { ...state.theme, accent: model.theme?.accent ?? 'moss', paper: model.theme?.paper ?? 'linen' },
    optimistic: {},
  };
  const formsState = { ...next.forms };
  formsState.editor = model.view === 'desk_new' || model.view === 'desk_edit' ? editorFor(next, model) : null;
  formsState.settings = model.view === 'desk_settings' ? settingsFor(next, model) : null;
  // A form kept with unsaved changes holds the view-model values its saves set.
  const kept = formsState.editor?.dirty ? editorForm(formsState.editor) : formsState.settings?.dirty ? SETTINGS : null;
  if (kept) next = { ...next, view: { ...next.view, model: { ...model, ...forms.held(kept, state.view.model) } } };
  return { ...next, forms: formsState };
}

/** The names of a form's fields, in the order it shows them. */
function fieldNames(form) {
  return forms.FORMS[form].fields.map((f) => f.name);
}

/**
 * The form a press of the editor's buttons submits: save (no status), publish or unpublish.
 * @param {any} editor @param {string | null} status
 */
function editorSubmission(editor, status) {
  if (editor.id == null) return status === 'published' ? 'form-new-article-publish' : 'form-new-article-save';
  if (status === 'published') return 'form-edit-article-publish';
  if (status === 'draft') return 'form-edit-article-unpublish';
  return editor.status === 'published' ? 'form-edit-article-save-changes' : 'form-edit-article-save-draft';
}

/**
 * What the page can tell of the article it is about to send, from what the server said the kernel
 * will check (the view's `record_rules`): the kernel's failed checks, and the ones it left to the
 * server. Null when the view carried nothing, which leaves everything to the server.
 * @param {any} state @param {any} editor @param {string} contract @param {Record<string, any>} inputs @param {number} now
 */
function localVerdict(state, editor, contract, inputs, now) {
  const rules = state.view.model?.record_rules?.[EDITOR_OBJECT];
  if (!rules) return null;
  const body = contracts.request(contract, inputs, state.route.path).body;
  const stored = editor.id == null ? null : (state.view.model.data.post?.fields ?? null);
  const session = state.session;
  return checkRecord(rules, draftOf(body, stored), {
    old: stored,
    principal: session?.authenticated ? (session.principal ?? null) : null,
    roles: session?.roles ?? [],
    now,
  });
}

/** Save the editor's article, optionally moving it to a new status in the same request. */
/** @returns {Result} */
function saveEditor(state, msg, status) {
  const editor = state.forms.editor;
  if (!editor) return { state, effects: [] };
  const form = editorSubmission(editor, status);
  const contract = forms.FORMS[form].contract;
  const inputs = forms.inputs(form, editor.values, scopeOf(state));
  const verdict = localVerdict(state, editor, contract, inputs, msg.at);
  const refused = verdict?.failed.length ? verdict.failed : null;
  const stepped = submitStep(editor.flow, 'ui-form-submitted', { complete: refused ? 0 : 1 }, msg.at);
  if (!stepped) return { state, effects: [] };
  if (refused) {
    // The page refuses what the kernel would refuse, in the kernel's words, before any request;
    // the fields show it as they show the server's refusal, and the first one takes focus.
    const { errors, formError, announce } = refusals.refusal(form, { failed: refused });
    const first = fieldNames(form).find((name) => errors[name]);
    return {
      state: {
        ...state,
        announce,
        forms: { ...state.forms, editor: { ...editor, flow: stepped.flow, busy: busy(stepped.flow), errors, formError } },
        focus: first ? { target: `editor-${first}`, token: (state.focus.token | 0) + 1 } : state.focus,
      },
      effects: [],
    };
  }
  if (!stepped.sends) return { state, effects: [] };
  const [next, effect] = call(state, 'editor.save', contract, inputs, msg, { form, status, retryKey: editor.createKey, page: verdict });
  return {
    state: { ...next, forms: { ...next.forms, editor: { ...editor, flow: stepped.flow, busy: true, errors: {}, formError: '' } } },
    effects: [effect],
  };
}

/** Where a form or an action leads after a success, and the message it shows. */
/** @returns {Result} */
function succeed(state, outcome, msg) {
  const toasted = outcome.toast ? toast(state, 'ok', outcome.toast) : state;
  // What the success says the view model now holds, until it is read again.
  const done = outcome.view && toasted.view.model ? { ...toasted, view: { ...toasted.view, model: { ...toasted.view.model, ...outcome.view } } } : toasted;
  if (!outcome.navigate) return fetchView(done, msg, true);
  const target = parseHref(outcome.navigate);
  if (hrefOf(state.route) === hrefOf(target)) return fetchView(done, msg, true);
  const from = screenOf(state);
  const to = matchRoute(target.path, target.query);
  // Between two screens drawn by the same renderer (a new article becoming the article saved)
  // the address is replaced and the page refreshed in place; elsewhere it is a navigation.
  const sameKind = from && to && routeOf(from.screen)?.kind === routeOf(to.screen)?.kind;
  if (!sameKind) return navigate(done, outcome.navigate, false, msg);
  const refreshed = fetchView({ ...done, route: target }, msg, true);
  return { state: refreshed.state, effects: [{ type: 'push_url', href: hrefOf(target), replace: true }, ...refreshed.effects] };
}

/** The live connection told that the identity it was opened with has ended. */
function resetLive(state, msg) {
  return liveStep(state, 'ui-live-reset', {}, msg) ?? { state, effects: [] };
}

/** A view refresh after a step of the live connection. @returns {Result} */
function refreshAfter(stepped, msg) {
  const refreshed = fetchView(stepped.state, msg, true);
  return { state: refreshed.state, effects: [...stepped.effects, ...refreshed.effects] };
}

/** Handle one frame from the server. */
/** @returns {Result} */
function onSocket(state, frame, msg) {
  switch (frame.type) {
    case 'welcome': {
      // With nothing seen on this machine the subscription starts at the server's head, so what
      // changed since the page was read is never replayed: read the view again.
      const first = state.ws.lastSeq === null;
      const stepped = liveStep(state, 'ui-live-welcome', { head: frame.head, 'heartbeat-ms': frame.heartbeat_ms }, msg);
      if (!stepped) return { state, effects: [] };
      return first ? refreshAfter(stepped, msg) : stepped;
    }
    case 'event': {
      const stepped = liveStep(state, 'ui-live-frame', { n: frame.n, seq: frame.seq }, msg);
      if (!stepped) return { state, effects: [] };
      // A frame went missing on this connection: the machine starts over rather than carry on stale.
      // What it missed is not replayed, so the view is read again now and once more when the
      // connection comes back from scratch.
      if (stepped.effects.some((e) => e.type === 'ws_close')) return refreshAfter(stepped, msg);
      const notice = frame.notice;
      // Whether the view is read again is the generated live binding's decision, for every kind
      // of notice; a change of membership also changes what the session may do, so it is read too.
      const shown = tenantOnShow(state);
      const relevant = state.view.status === 'ready'
        && (shown.entry
          ? liveRefreshes(shown.part, 'notice', { kind: notice.kind, object: notice.object }, state.view.model)
          : live.refreshes(screenOf(state)?.screen ?? '', 'ui-notice-received', { kind: notice.kind, object: notice.object }, state.view.model));
      let current = { state: stepped.state, effects: stepped.effects };
      // A change of the schema may change the tenant's interface: its manifest is read again (INT-3258).
      if (notice.kind === 'schema' && (current.state.ui.manifest || wantsManifest(current.state))) {
        const read = fetchManifest(current.state, msg);
        current = { state: read.state, effects: [...current.effects, ...read.effects] };
      }
      if (notice.kind === 'membership') {
        const [withRequest, effect] = readSession(stepped.state, msg);
        current = { state: withRequest, effects: [...stepped.effects, effect] };
      }
      return relevant ? refreshAfter(current, msg) : current;
    }
    case 'resync':
      // Too far behind to replay: carry on from the server's head, with the view read again.
      return refreshAfter(liveStep(state, 'ui-live-resync', { head: frame.head }, msg) ?? { state, effects: [] }, msg);
    case 'pong': {
      // A head beyond what this connection delivered means changes arrived that no frame told of.
      const behind = frame.head > (state.ws.lastSeq ?? 0);
      const stepped = liveStep(state, 'ui-live-pong', { head: frame.head }, msg);
      if (!stepped) return { state, effects: [] };
      return behind ? refreshAfter(stepped, msg) : stepped;
    }
    case 'session': {
      if (frame.state === 'active') return { state, effects: [] };
      // Our own sign-out ends the session too; its reply will say so, once.
      if (Object.values(state.pending).some((p) => p.kind === 'logout')) return { state, effects: [] };
      const ended = sessionStep(state, 'ui-session-ended', {}, msg.at);
      if (!ended) return { state, effects: [] };
      const text = frame.state === 'expired' ? 'Your session has expired. Sign in again to keep writing.' : 'You have been signed out.';
      let next = clearUserState(toast(ended.state, 'warn', text));
      /** @type {Effect[]} */
      const effects = [];
      if (ended.readSession) {
        const [withRequest, effect] = readSession(next, msg);
        next = withRequest;
        effects.push(effect);
      }
      next = { ...next, session: { ...state.session, authenticated: false, csrf_token: undefined, roles: ['public'], may: {}, may_manage_schema: false } };
      // A guarded page's model was cleared with the person's state, so it is always read again.
      const shown = tenantOnShow(state);
      const refresh = guardedScreen(state)
        || (shown.entry ? liveRefreshes(shown.part, 'session-ended', {}, state.view.model) : live.refreshes(screenOf(state)?.screen ?? '', 'ui-session-ended', {}, state.view.model));
      return refresh ? refreshAfter({ state: next, effects }, msg) : { state: next, effects };
    }
    case 'run':
      return labUpdate(state, { ...msg, type: 'lab_run', frame }, { request, toast }) ?? { state, effects: [] };
    default:
      return { state, effects: [] };
  }
}

/**
 * The hooks a screen's `onEnter` names: what each section asks for when its screen is shown.
 * `afterSession` hooks also run when the session reply arrives after the page.
 * @type {Map<string, { enter: (state: any, msg: any, io: { request: typeof request }) => Result, afterSession: boolean }>}
 */
const ENTER_HOOKS = new Map([
  ['sessions', { enter: sessionsEnter, afterSession: false }],
  ['passkeys', { enter: passkeysEnter, afterSession: false }],
  ['factors', { enter: factorsEnter, afterSession: false }],
  ['lab', { enter: labEnter, afterSession: true }],
]);

/**
 * Register the hook of a section name a screen's `onEnter` may use, so a product asks for what its
 * screen shows without editing this file.
 * @param {string} name
 * @param {(state: any, msg: any, io: { request: typeof request }) => Result} enter
 * @param {{ afterSession?: boolean }} [options]
 */
export function registerEnterHook(name, enter, options = {}) {
  ENTER_HOOKS.set(name, { enter, afterSession: options.afterSession === true });
}

/** The names of the registered entry hooks, sorted: for a test that every `onEnter` name has one. */
export function enterHookNames() {
  return [...ENTER_HOOKS.keys()].sort();
}

/**
 * What a view's screen asks for when it is shown: its `onEnter` sections, in order, each hook
 * seeing the state the one before left.
 * @param {any} state @param {any} msg @param {string | null | undefined} view
 * @param {{ afterSession?: boolean }} [options]
 * @returns {Result}
 */
function enterScreen(state, msg, view, options = {}) {
  /** @type {Effect[]} */
  const effects = [];
  let next = state;
  for (const name of enterSections(view)) {
    const hook = ENTER_HOOKS.get(name);
    if (!hook || (options.afterSession && !hook.afterSession)) continue;
    const entered = hook.enter(next, msg, { request });
    next = entered.state;
    effects.push(...entered.effects);
  }
  return { state: next, effects };
}

/** A passkey was accepted: the same as a password being accepted, from the session on. @returns {Result} */
function passkeySignedIn(state, msg) {
  // A first factor that is not enough: no session yet, and the page asks for the second.
  if (msg.body?.second_factor) return { state: factorsWaiting(state, msg.body), effects: [] };
  const outputs = contracts.decode('sign-in-at-site', msg.body, {});
  const outcome = forms.succeeded(SIGN_IN, scopeOf(state, { form: {}, response: outputs }));
  let signedIn = { ...state, session: outputs.session };
  signedIn = sessionStep(signedIn, 'ui-signed-in', {}, msg.at)?.state ?? signedIn;
  signedIn = clearUserState(signedIn);
  if (outcome.toast) signedIn = toast(signedIn, 'ok', outcome.toast);
  const reset = resetLive(signedIn, msg);
  const moved = navigate(reset.state, outcome.navigate ?? '/', false, msg);
  return { state: moved.state, effects: [...reset.effects, ...moved.effects] };
}

/** An emailed link was accepted: the same as a passkey being accepted, from the session on. @returns {Result} */
const linkSignedIn = passkeySignedIn;

/** An identity provider's answer was accepted: the same, from the session on. @returns {Result} */
const oidcSignedIn = passkeySignedIn;
const invitesHelpers = { call, scopeOf, toast, navigate };

/** Ask a refused action again, now that the person has authenticated. @returns {Result} */
function redispatch(state, retry, msg) {
  return onAction(state, { type: 'action', name: retry.name, arg: retry.arg, at: msg.at, rand: msg.rand });
}

/**
 * Whether a refused or failed request may have been carried out all the same: no answer came, or
 * the server failed. Only then is a key kept for the retry.
 * @param {{ ok: boolean, status: number }} msg
 */
function unknownOutcome(msg) {
  return !msg.ok && (msg.status === 0 || msg.status >= 500);
}

/**
 * Ask for the next page of a list section of the view, with the cursor the last answer gave.
 * @param {any} state @param {Message} msg @param {string} section
 * @returns {Result}
 */
function loadMore(state, msg, section) {
  const cursor = state.view.next?.[section];
  if (typeof cursor !== 'string' || state.view.more || state.view.status !== 'ready') return { state, effects: [] };
  const query = new URLSearchParams({ path: state.route.path, cursor });
  for (const [name, value] of Object.entries(state.route.query)) {
    // The view's own parameters; the delivery interface's names are not a page's to set.
    if (!['path', 'cursor', 'fields', 'limit'].includes(name)) query.set(name, String(value));
  }
  const [next, effect] = request(state, 'view.more', { path: `/api/view?${query}`, meta: { section } }, msg);
  return { state: { ...next, view: { ...next.view, more: effect.type === 'http' ? effect.id : 0 } }, effects: [effect] };
}

/** @returns {Result} */
function onHttp(state, msg) {
  const pending = state.pending[msg.id];
  if (!pending) return { state, effects: [] };
  const { [msg.id]: _done, ...rest } = state.pending;
  state = { ...state, pending: rest };
  // A reply that is not the latest of its kind, or that answers a request made for a person who
  // has since signed out, is dropped.
  if (LATEST_ONLY.includes(pending.kind) && state.latest?.[pending.kind] !== msg.id) return { state, effects: [] };
  if (isUserKind(pending.kind) && (pending.generation ?? 0) !== (state.generation ?? 0)) return { state, effects: [] };
  const error = msg.ok ? null : msg.body && typeof msg.body === 'object' ? msg.body : { code: 'unavailable', message: 'The site could not be reached.' };
  const waiting = msg.ok && msg.body?.second_factor ? msg.body : null;
  const outputs = msg.ok && pending.contract && !waiting ? contracts.decode(pending.contract, msg.body, pending.inputs) : null;

  // An operation that needs a fresh authentication is asked again once the person has given one.
  if (isStepUp({ ...msg, error }) && pending.retry) {
    return { state: factorsRequireStepUp(state, pending.retry), effects: [] };
  }
  if (pending.kind.startsWith('factors.')) {
    return factorsUpdate(state, { ...msg, pending, error }, { request, toast, signedIn: passkeySignedIn, redispatch }) ?? { state, effects: [] };
  }

  if (pending.kind.startsWith('lab.')) {
    return labUpdate(state, { ...msg, pending, error }, { request, toast }) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('sessions.')) {
    return sessionsUpdate(state, { ...msg, pending, error }, { request, toast }) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('passkeys.')) {
    return passkeysUpdate(state, { ...msg, pending, error }, { request, toast, signedIn: passkeySignedIn }) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('links.')) {
    return linksUpdate(state, { ...msg, pending, error }, { request, signedIn: linkSignedIn }) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('invites.')) {
    return invitesUpdate(state, { ...msg, pending, error, outputs }, invitesHelpers) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('oidc.')) {
    return oidcUpdate(state, { ...msg, pending, error }, { request, signedIn: oidcSignedIn }) ?? { state, effects: [] };
  }

  switch (pending.kind) {
    case 'session': {
      if (!msg.ok) return { state, effects: [] };
      let known = sessionRead({ ...state, session: outputs.session }, outputs.session.authenticated === true, msg.at);
      // The session was read because the socket upgrade was refused and the server answered: that
      // is a refusal, not an outage. After enough of them live updates stop, visibly.
      if (pending.refusal) {
        const refusals = (known.wsRefusals ?? 0) + 1;
        known = { ...known, wsRefusals: refusals, wsStopped: refusals >= WS_REFUSAL_LIMIT };
      }
      // A cold start could not open the socket before it knew the request token the gateway
      // issued, so it opens it now.
      /** @type {Effect[]} */
      const open = state.session ? [] : [{ type: 'ws_connect', csrf: outputs.session.csrf_token }];
      // A link the page was opened with waits for the request token that this reply brings.
      const linked = linksLand(known, msg, { request });
      const landed = oidcLand(linked.state, msg, { request });
      const entered = enterScreen(landed.state, msg, state.view.model?.view, { afterSession: true });
      return { state: entered.state, effects: [...open, ...linked.effects, ...landed.effects, ...entered.effects] };
    }
    case 'view': {
      if (state.view.request !== msg.id) return { state, effects: [] };
      if (msg.ok) {
        const accepted = acceptView(state, decodeView(state, msg.body), msg.body?.next);
        return enterScreen(accepted, msg, msg.body.view);
      }
      const status = msg.status === 404 ? 'missing' : 'error';
      return {
        state: { ...state, ready: true, view: { status, model: null, error, request: 0 }, forms: { ...state.forms, editor: null, settings: null } },
        effects: [],
      };
    }
    case 'view.more': {
      // A reply for a page that was left, or that a fresh reading of the view replaced, is dropped.
      if (state.view.more !== msg.id || !state.view.model) return { state, effects: [] };
      const { section } = pending;
      const idle = { ...state, view: { ...state.view, more: 0 } };
      const rows = msg.body?.data?.[section];
      if (!msg.ok || !Array.isArray(rows) || !Array.isArray(state.view.model.data[section])) {
        return { state: toast(idle, 'warn', 'More could not be loaded.'), effects: [] };
      }
      // The next page continues the rows held, with none repeated.
      const held = new Set(state.view.model.data[section].map((/** @type {any} */ row) => row.id));
      const model = { ...state.view.model, data: { ...state.view.model.data, [section]: [...state.view.model.data[section], ...rows.filter((row) => !held.has(row.id))] } };
      const { [section]: _read, ...others } = state.view.next ?? {};
      const next = msg.body.next?.[section] ? { ...others, [section]: msg.body.next[section] } : others;
      return { state: { ...idle, view: { ...idle.view, model, next } }, effects: [] };
    }
    case 'login': {
      const login = state.forms.login;
      if (!msg.ok) {
        const flow = submitStep(login.flow, 'ui-form-refused', {}, msg.at)?.flow ?? login.flow;
        const text = refusals.refusal(SIGN_IN, error).formError;
        return {
          state: { ...state, announce: text, forms: { ...state.forms, login: { ...login, flow, busy: busy(flow), error: text, values: { ...login.values, password: '' } } }, focus: { target: 'login-password', token: msg.id } },
          effects: [],
        };
      }
      const flow = submitStep(login.flow, 'ui-form-accepted', {}, msg.at)?.flow ?? login.flow;
      if (waiting) {
        const cleared = { ...state, forms: { ...state.forms, login: { ...login, flow, busy: busy(flow), error: '', values: { ...login.values, password: '' } } } };
        return { state: factorsWaiting(cleared, waiting), effects: [] };
      }
      const outcome = forms.succeeded(SIGN_IN, scopeOf(state, { form: pending.values, response: outputs }));
      let signedIn = { ...state, session: outputs.session, forms: { ...state.forms, login: { values: forms.initialValues(SIGN_IN, {}), busy: busy(flow), error: '', flow } } };
      signedIn = sessionStep(signedIn, 'ui-signed-in', {}, msg.at)?.state ?? signedIn;
      // Whoever signs in, nothing of the person before them is kept.
      signedIn = clearUserState(signedIn);
      if (outcome.toast) signedIn = toast(signedIn, 'ok', outcome.toast);
      // The socket was opened as an anonymous visitor; open it again as this person.
      const reset = resetLive(signedIn, msg);
      const moved = navigate(reset.state, outcome.navigate ?? '/', false, msg);
      return { state: moved.state, effects: [...reset.effects, ...moved.effects] };
    }
    case 'logout': {
      if (!msg.ok) return { state: toast(state, 'warn', refusals.refusal(pending.action, error).text), effects: [] };
      const outcome = actions.succeeded(pending.action, scopeOf(state, { response: outputs }));
      let out = sessionStep({ ...state, session: outputs.session }, 'ui-signed-out', {}, msg.at)?.state ?? { ...state, session: outputs.session };
      out = clearUserState(out);
      if (outcome.toast) out = toast(out, 'ok', outcome.toast);
      const reset = resetLive(out, msg);
      // Signing out where the action says nowhere to go reads the same address again.
      const moved = navigate(reset.state, outcome.navigate ?? hrefOf(state.route), true, msg);
      return { state: moved.state, effects: [...reset.effects, ...moved.effects] };
    }
    case 'editor.save': {
      const editor = state.forms.editor;
      if (!editor) return { state, effects: [] };
      if (!msg.ok) {
        const flow = submitStep(editor.flow, 'ui-form-refused', {}, msg.at)?.flow ?? editor.flow;
        const { errors, formError, announce } = refusals.refusal(pending.form, error);
        // Focus goes to the first failing field in the order the form shows them.
        const first = fieldNames(pending.form).find((name) => errors[name]);
        // The server's refusal stands. A draft the page passed and the server refused is drift
        // between the page's evaluator and the kernel, and is logged so it is seen.
        const drift = pending.page ? disagreement(pending.page, error) : null;
        return {
          state: {
            ...state,
            announce,
            forms: { ...state.forms, editor: { ...editor, flow, busy: busy(flow), errors, formError, createKey: unknownOutcome(msg) ? pending.sent ?? editor.createKey ?? null : null } },
            focus: first ? { target: `editor-${first}`, token: msg.id } : state.focus,
          },
          effects: drift ? [{ type: 'log', level: 'warn', event: 'record-rules.disagreement', correlation: pending.correlation, detail: drift }] : [],
        };
      }
      let flow = submitStep(editor.flow, 'ui-form-accepted', {}, msg.at)?.flow ?? editor.flow;
      const response = pending.first ? forms.bothResponses(pending.first, outputs) : outputs;
      const saved = { ...editor, id: response.article_id, version: response.version, key: response.slug, dirty: false, remoteChanged: false, createKey: null };
      const scope = scopeOf(state, { form: editor.values, response });
      const second = pending.first ? null : forms.followUp(pending.form, scope);
      if (second) {
        const again = submitStep(flow, 'ui-form-submitted', { complete: 1 }, msg.at);
        flow = again?.flow ?? flow;
        const withSaved = { ...state, forms: { ...state.forms, editor: { ...saved, flow, busy: busy(flow) } } };
        const [next, effect] = call(withSaved, 'editor.save', second.contract, second.inputs, msg, { form: pending.form, status: pending.status, first: response });
        return { state: next, effects: [effect] };
      }
      const status = pending.status ?? editor.status;
      const outcome = forms.succeeded(pending.form, scope);
      const done = { ...state, forms: { ...state.forms, editor: { ...saved, flow, busy: busy(flow), status } } };
      return succeed(done, outcome, msg);
    }
    case 'editor.preview': {
      const editor = state.forms.editor;
      if (!editor || editor.preview?.request !== msg.id) return { state, effects: [] };
      const local = msg.ok ? actions.placeResult(pending.action, {}, outputs) : null;
      const preview = local ? { blocks: local.preview, request: 0, failed: false } : { blocks: editor.preview.blocks, request: 0, failed: true };
      return { state: { ...state, forms: { ...state.forms, editor: { ...editor, preview } } }, effects: [] };
    }
    case 'post.action': {
      const { [pending.id]: _gone, ...optimistic } = state.optimistic;
      if (msg.ok) return succeed(state, actions.succeeded(pending.action, scopeOf(state, { item: pending.item, response: outputs })), msg);
      return { state: toast({ ...state, optimistic }, 'warn', refusals.refusal(pending.action, error).text), effects: [] };
    }
    case 'settings.save': {
      const settings = state.forms.settings;
      if (!settings) return { state, effects: [] };
      if (!msg.ok) {
        const flow = submitStep(settings.flow, 'ui-form-refused', {}, msg.at)?.flow ?? settings.flow;
        const { errors, formError, announce } = refusals.refusal(SETTINGS, error);
        return { state: { ...state, announce, forms: { ...state.forms, settings: { ...settings, flow, busy: busy(flow), errors, formError } } }, effects: [] };
      }
      const flow = submitStep(settings.flow, 'ui-form-accepted', {}, msg.at)?.flow ?? settings.flow;
      const done = { ...state, forms: { ...state.forms, settings: { ...settings, flow, busy: busy(flow), dirty: false } } };
      return succeed(done, forms.succeeded(SETTINGS, scopeOf(done, { form: settings.values, response: outputs })), msg);
    }
    case 'ui.manifest': {
      if (!msg.ok || !msg.body || typeof msg.body.hash !== 'string') return { state, effects: [] };
      const next = { ...state, ui: acceptManifest(state.ui, msg.body) };
      const screen = screenOf(next);
      const shown = tenantScreenOfView(next.ui, next.view.model?.view);
      const wanted = shown?.id ?? (screen?.tenant ? screen.screen : null);
      // An address the platform's catch-all took before the manifest arrived may be a tenant screen.
      const refetch = !state.ui.manifest && screen?.tenant && !shown && next.view.status !== 'loading';
      if (refetch) return fetchView(next, msg, true);
      return fetchParts(next, msg, wanted);
    }
    case 'ui.part': {
      const { hash } = pending;
      if (!msg.ok || !msg.body) {
        const { [hash]: _failed, ...asked } = state.ui.asked;
        return { state: { ...state, ui: { ...state.ui, asked } }, effects: [] };
      }
      return { state: { ...state, ui: acceptPart(state.ui, hash, msg.body) }, effects: [] };
    }
    case 'tenant.submit':
    case 'tenant.action':
      return tenantReply(state, msg, pending, error);
    case 'dev':
      return { state: toast(state, msg.ok ? 'ok' : 'warn', msg.ok ? 'Done.' : 'That did not work.'), effects: [] };
    default:
      return { state, effects: [] };
  }
}

/** @returns {Result} */
function navigate(state, href, replace, msg) {
  const route = parseHref(href);
  const same = hrefOf(route) === hrefOf(state.route);
  const moved = { ...state, route, menuOpen: false, confirmDelete: null, focus: { target: 'main', token: state.nextId } };
  const fetched = fetchView(moved, msg, false);
  const providers = oidcEnter(fetched.state, msg, { request });
  /** @type {Effect[]} */
  const effects = same && !replace ? [] : [{ type: 'push_url', href: hrefOf(route), replace }];
  return { state: providers.state, effects: [...effects, { type: 'scroll_top' }, ...fetched.effects, ...providers.effects] };
}

/**
 * The reply to a tenant form or action: on success its message and its navigation (else the view is
 * read again); on a refusal the tenant's wording, beside the form or as a message.
 * @returns {Result}
 */
function tenantReply(state, msg, pending, error) {
  const { part } = tenantOnShow(state);
  const element = pending.kind === 'tenant.submit' ? part?.forms?.[pending.owner] : part?.actions?.[pending.owner];
  const held = pending.kind === 'tenant.submit' ? state.tenant.forms[pending.owner] : null;
  if (msg.ok) {
    const settled = held ? { ...state, tenant: { ...state.tenant, forms: { ...state.tenant.forms, [pending.owner]: formInit(element) } } } : state;
    const outcome = element?.on_success ?? {};
    const toasted = outcome.toast ? toast(settled, 'ok', outcome.toast) : settled;
    const href = outcome.navigate ? tenantHref(toasted.ui, outcome.navigate, screenOf(toasted)?.params ?? {}) : null;
    return href ? navigate(toasted, href, false, msg) : fetchView(toasted, msg, true);
  }
  const form = pending.kind === 'tenant.submit' ? element : null;
  const said = explainRefusal(errorsPart(state.ui), pending.owner, form, error ?? {});
  if (element?.on_error === 'toast' || !held) {
    const released = held ? { ...state, tenant: { ...state.tenant, forms: { ...state.tenant.forms, [pending.owner]: { ...held, busy: false } } } } : state;
    return { state: toast(released, 'warn', said.message), effects: [] };
  }
  const shown = { ...held, busy: false, error: said.message, errors: said.fields };
  return { state: { ...state, announce: said.message, tenant: { ...state.tenant, forms: { ...state.tenant.forms, [pending.owner]: shown } } }, effects: [] };
}

/**
 * Invoke a tenant operation for a form or an action of a tenant screen, with a key made once for
 * the attempt (D-6595).
 * @returns {[any, Effect]}
 */
function invokeOperation(state, kind, owner, operation, inputs, msg) {
  return request(state, kind, { method: 'POST', path: '/api/operations/invoke', body: { operation, inputs }, write: true, idempotencyKey: newKey(state, msg), meta: { owner } }, msg);
}

/** Submit a form of the tenant screen on show. @returns {Result} */
function submitTenantForm(state, msg) {
  const id = msg.form.slice('tenant:'.length);
  const form = tenantOnShow(state).part?.forms?.[id];
  if (!form) return { state, effects: [] };
  const held = state.tenant.forms[id] ?? formInit(form);
  if (held.busy) return { state, effects: [] };
  // What the page holds is what was typed; the values the submission carries agree with it.
  const values = { ...held.values, ...Object.fromEntries(Object.entries(msg.values ?? {}).filter(([name]) => name in held.values)) };
  const [next, effect] = invokeOperation(state, 'tenant.submit', id, form.operation, formInputs(form, values), msg);
  return { state: { ...next, tenant: { ...next.tenant, forms: { ...next.tenant.forms, [id]: { ...held, values, busy: true, error: '', errors: {} } } } }, effects: [effect] };
}

/** Press an action of the tenant screen on show: one with a confirmation takes two presses. @returns {Result} */
function tenantAction(state, msg) {
  const id = String(msg.arg ?? '');
  const action = tenantOnShow(state).part?.actions?.[id];
  if (!action) return { state, effects: [] };
  if (action.confirm && state.tenant.confirm !== id) return { state: { ...state, announce: action.confirm, tenant: { ...state.tenant, confirm: id } }, effects: [] };
  const values = action.form ? state.tenant.forms[action.form]?.values ?? {} : {};
  const screen = screenOf(state);
  const route = { ...(screen?.params ?? {}), ...(screen?.query ?? {}), ...(state.view.model?.params ?? {}) };
  const [next, effect] = invokeOperation({ ...state, tenant: { ...state.tenant, confirm: null } }, 'tenant.action', id, action.operation, actionInputs(action, values, route), msg);
  return { state: next, effects: [effect] };
}

function onInput(state, msg) {
  const f = state.forms;
  if (msg.form.startsWith('tenant:')) {
    const id = msg.form.slice('tenant:'.length);
    const form = tenantOnShow(state).part?.forms?.[id];
    if (!form) return state;
    const held = state.tenant.forms[id] ?? formInit(form);
    if (!(msg.name in held.values)) return state;
    const { [msg.name]: _cleared, ...errors } = held.errors;
    return { ...state, tenant: { ...state.tenant, forms: { ...state.tenant.forms, [id]: { ...held, values: { ...held.values, [msg.name]: msg.value }, errors } } } };
  }
  if (msg.form === 'link') return linksInput(state, msg.name, msg.value);
  if (msg.form === 'invite') return invitesInput(state, msg.name, msg.value);
  if (msg.form === 'second' || msg.form === 'totpconfirm' || msg.form === 'stepup') return factorsInput(state, msg.name, msg.value);
  if (msg.form === 'login') {
    const { values } = forms.input(SIGN_IN, f.login.values, [], msg.name, msg.value);
    return { ...state, forms: { ...f, login: { ...f.login, values } } };
  }
  if (msg.form === 'editor' && f.editor) {
    const editor = f.editor;
    const { values, edited } = forms.input(editorForm(editor), editor.values, editor.edited, msg.name, msg.value, scopeOf(state));
    const { [msg.name]: _cleared, ...errors } = editor.errors;
    return { ...state, forms: { ...f, editor: { ...editor, values, edited, dirty: true, errors, preview: editor.preview ? { ...editor.preview, stale: msg.name === 'body' || editor.preview.stale } : null } } };
  }
  if (msg.form === 'settings' && f.settings) {
    const settings = f.settings;
    const { values } = forms.input(SETTINGS, settings.values, [], msg.name, msg.value);
    return { ...state, forms: { ...f, settings: { ...settings, dirty: true, values } } };
  }
  return state;
}

/** @returns {Result} */
function requestPreview(state, msg) {
  const editor = state.forms.editor;
  if (!editor) return { state, effects: [] };
  const action = editor.id == null ? 'act-preview-new' : 'act-preview-edit';
  const contract = actions.ACTIONS[action].contract;
  const inputs = actions.args(action, scopeOf(state, { form: editor.values }));
  const [next, effect] = call(state, 'editor.preview', contract, inputs, msg, { action });
  const preview = { blocks: editor.preview?.blocks ?? null, request: effect.type === 'http' ? effect.id : 0, failed: false, stale: false };
  return { state: { ...next, forms: { ...next.forms, editor: { ...editor, tab: 'preview', preview } } }, effects: [effect] };
}

/**
 * Press a per-row action button (generated `ROW_ACTIONS`: its action, the status the row shows at
 * once, and the output of the row that holds the record id) on the row of this record.
 * @returns {Result}
 */
function rowAction(state, msg, button, arg) {
  const { action, marker, rowKey } = button;
  const model = state.view.model;
  const item = actions.rows(action, model).find((row) => row[rowKey] === arg.id) ?? { [rowKey]: arg.id, version: arg.version };
  const scope = scopeOf(state, { item });
  const inputs = actions.args(action, scope);
  if (!actions.visible(action, scope, inputs)) return { state, effects: [] };
  const [next, effect] = call(
    { ...state, optimistic: { ...state.optimistic, [arg.id]: marker } }, 'post.action', actions.ACTIONS[action].contract, inputs, msg, { id: arg.id, action, item },
  );
  return { state: next, effects: [effect] };
}

/** @returns {Result} */
function onAction(state, msg) {
  const arg = msg.arg;
  const pressed = rowButton(msg.name);
  // A confirmation is armed for one press: any other action disarms it.
  if (state.confirmDelete !== null && !pressed?.confirm) state = { ...state, confirmDelete: null };
  const asked = rowButtonAsked(msg.name);
  // A confirmed row action takes two presses on the same row: the first asks, the second does.
  if (asked) return { state: { ...state, confirmDelete: arg.id, announce: actions.ACTIONS[asked.action].confirmAnnounce ?? '' }, effects: [] };
  if (pressed) {
    if (pressed.confirm && state.confirmDelete !== arg.id) return { state, effects: [] };
    return rowAction({ ...state, confirmDelete: null }, msg, pressed, arg);
  }
  switch (msg.name) {
    case 'theme': {
      const mode = THEME_MODES[(THEME_MODES.indexOf(state.theme.mode) + 1) % THEME_MODES.length];
      const label = { auto: 'Theme follows your device.', light: 'Light theme.', dark: 'Dark theme.' }[mode];
      return { state: { ...state, theme: { ...state.theme, mode }, announce: label }, effects: [{ type: 'store', key: 'strata.theme', value: mode }] };
    }
    case 'menu':
      return { state: { ...state, menuOpen: !state.menuOpen }, effects: [] };
    case 'dismiss':
      return { state: { ...state, toasts: state.toasts.filter((t) => t.id !== arg) }, effects: [] };
    case 'logout': {
      // The page's sign-out: the action that signs out on this screen.
      const action = actions.actionsOn(screenOf(state)?.screen ?? '').find((a) => actions.ACTIONS[a].contract === 'sign-out-of-site');
      if (!action) return { state, effects: [] };
      const [next, effect] = call(state, 'logout', actions.ACTIONS[action].contract, actions.args(action, scopeOf(state)), msg, { action });
      return { state: next, effects: [effect] };
    }
    case 'editor.tab':
      if (!state.forms.editor) return { state, effects: [] };
      if (arg === 'preview') return requestPreview(state, msg);
      return { state: { ...state, forms: { ...state.forms, editor: { ...state.forms.editor, tab: 'write' } } }, effects: [] };
    case 'editor.publish':
      return saveEditor(state, msg, 'published');
    case 'editor.unpublish':
      return saveEditor(state, msg, 'draft');
    case 'editor.reload': {
      const cleared = { ...state, forms: { ...state.forms, editor: null } };
      return fetchView(cleared, msg, true);
    }
    case 'reload':
      return fetchView(state, msg, false);
    case 'view.more':
      return loadMore(state, msg, String(arg));
    case 'tenant.action':
      return tenantAction(state, msg);
    case 'dev.expire': {
      const [next, effect] = request(state, 'dev', { method: 'POST', path: '/api/dev/expire-session', write: true, body: {} }, msg);
      return { state: next, effects: [effect] };
    }
    default:
      if (msg.name.startsWith('lab.')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      if (msg.name.startsWith('sessions.')) return sessionsUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      if (msg.name.startsWith('factors.')) return factorsUpdate(state, msg, { request, toast, signedIn: passkeySignedIn, redispatch }) ?? { state, effects: [] };
      if (msg.name.startsWith('passkeys.')) return passkeysUpdate(state, msg, { request, toast, signedIn: passkeySignedIn }) ?? { state, effects: [] };
      if (msg.name.startsWith('oidc.')) return oidcUpdate(state, msg, { request, signedIn: oidcSignedIn }) ?? { state, effects: [] };
      return { state, effects: [] };
  }
}

/** @returns {Result} */
function onSubmit(state, msg) {
  switch (msg.form) {
    case 'link':
      return linksUpdate(state, msg, { request, signedIn: linkSignedIn }) ?? { state, effects: [] };
    case 'invite':
      return invitesUpdate(state, msg, invitesHelpers) ?? { state, effects: [] };
    case 'login': {
      const login = state.forms.login;
      const values = forms.submitted(SIGN_IN, msg.values);
      const errors = forms.validate(SIGN_IN, values);
      const stepped = submitStep(login.flow, 'ui-form-submitted', { complete: Object.keys(errors).length ? 0 : 1 }, msg.at);
      if (!stepped) return { state, effects: [] };
      if (!stepped.sends) {
        const message = fieldNames(SIGN_IN).map((name) => errors[name]).find(Boolean);
        if (!message) return { state: { ...state, forms: { ...state.forms, login: { ...login, flow: stepped.flow } } }, effects: [] };
        return { state: { ...state, announce: message, forms: { ...state.forms, login: { ...login, flow: stepped.flow, busy: busy(stepped.flow), values, error: message } } }, effects: [] };
      }
      // Signing in carries the request token too: the session's own, or the one the gateway issued to this browser.
      const inputs = forms.inputs(SIGN_IN, values, scopeOf(state));
      const [next, effect] = call(state, 'login', forms.FORMS[SIGN_IN].contract, inputs, msg, { values });
      return { state: { ...next, forms: { ...next.forms, login: { values, flow: stepped.flow, busy: busy(stepped.flow), error: '' } } }, effects: [effect] };
    }
    case 'second':
    case 'totpconfirm':
    case 'stepup':
      return factorsUpdate(state, msg, { request, toast, signedIn: passkeySignedIn, redispatch }) ?? { state, effects: [] };
    case 'search': {
      // The search form calls the view model of the search page, which opening the page does.
      const form = forms.submitted('form-search', msg.values);
      const outcome = forms.succeeded('form-search', scopeOf(state, { form, response: form }));
      return navigate(state, outcome.navigate ?? '/search', false, msg);
    }
    case 'editor':
      return saveEditor(state, msg, null);
    case 'settings': {
      const settings = state.forms.settings;
      if (!settings) return { state, effects: [] };
      const stepped = submitStep(settings.flow, 'ui-form-submitted', { complete: 1 }, msg.at);
      if (!stepped?.sends) return { state, effects: [] };
      const [next, effect] = call(state, 'settings.save', forms.FORMS[SETTINGS].contract, forms.inputs(SETTINGS, settings.values, scopeOf(state)), msg);
      return { state: { ...next, forms: { ...next.forms, settings: { ...settings, flow: stepped.flow, busy: true, errors: {}, formError: '' } } }, effects: [effect] };
    }
    default:
      if (msg.form.startsWith('tenant:')) return submitTenantForm(state, msg);
      if (msg.form.startsWith('lab')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      return { state, effects: [] };
  }
}

/**
 * The transition function of the frontend.
 * @param {ReturnType<typeof initialState>} state
 * @param {Message} msg
 * @returns {{ state: any, effects: Effect[] }}
 */
export function update(state, msg) {
  if (msg.type === 'timer' && msg.name === 'toast') {
    return { state: { ...state, toasts: state.toasts.filter((t) => t.id !== msg.token) }, effects: [] };
  }
  // An announcement is made once: the next message clears it, so the same words said again
  // (the same failed sign-in twice) change the live region and are read out again.
  const result = step(state.announce ? { ...state, announce: '' } : state, msg);
  // Every message that appears is given a time to go away again by itself.
  const known = new Set(state.toasts.map((t) => t.id));
  const fresh = result.state.toasts.filter((t) => !known.has(t.id));
  if (!fresh.length) return result;
  /** @type {Effect[]} */
  const timers = fresh.map((t) => ({ type: 'timer', name: 'toast', token: t.id, ms: t.kind === 'warn' ? 9000 : 5000 }));
  return { state: result.state, effects: [...result.effects, ...timers] };
}

/**
 * @param {ReturnType<typeof initialState>} state
 * @param {Message} msg
 * @returns {{ state: any, effects: Effect[] }}
 */
function step(state, msg) {
  switch (msg.type) {
    case 'init': {
      const route = parseHref(msg.path + msg.search);
      const mode = THEME_MODES.includes(msg.storedTheme ?? '') ? msg.storedTheme : 'auto';
      let next = { ...state, route, theme: { ...state.theme, mode, systemDark: msg.systemDark }, passkeys: { ...state.passkeys, supported: msg.passkeys === true } };
      /** @type {Effect[]} */
      const effects = [];
      const boot = msg.bootstrap;
      if (boot?.session?.session) {
        next = sessionRead({ ...next, session: boot.session.session }, boot.session.session.authenticated === true, msg.at);
      } else {
        const [withRequest, effect] = readSession(next, msg);
        next = withRequest;
        effects.push(effect);
      }
      if (boot?.view && boot.path === route.path) {
        next = acceptView(next, decodeView(next, boot.view), boot.view.next);
        const entered = enterScreen(next, msg, boot.view.view);
        next = entered.state;
        effects.push(...entered.effects);
      } else {
        const fetched = fetchView(next, msg, false);
        next = fetched.state;
        effects.push(...fetched.effects);
      }
      // An address the platform's own screens do not answer, showing a view no platform screen
      // shows, is a tenant screen: the tenant's interface definition is read (D-8250).
      if (wantsManifest(next) && !screenOfView(/** @type {any} */ (next.view).model?.view)) {
        const manifest = fetchManifest(next, msg);
        next = manifest.state;
        effects.push(...manifest.effects);
      }
      // A link the page was opened with is taken out of the address bar and, once the request
      // token is known, sent.
      const opened = linksOpened(next, msg.hash ?? '', hrefOf(route));
      const landed = linksLand(opened.state, msg, { request });
      next = landed.state;
      effects.push(...opened.effects, ...landed.effects);
      // An owner's invitation is kept the same way, and taken out of the address bar at once.
      const invited = invitesOpened(next, msg.hash ?? '', hrefOf(route));
      next = invited.state;
      effects.push(...invited.effects);
      // The same for the answer of an identity provider, and the sign-in page asks which
      // providers the site offers.
      const returned = oidcOpened(next, msg.hash ?? '', hrefOf(route));
      const answered = oidcLand(returned.state, msg, { request });
      const offered = oidcEnter(answered.state, msg, { request });
      next = offered.state;
      effects.push(...returned.effects, ...answered.effects, ...offered.effects);
      // The socket needs the request token, which a bootstrapped page already has. A cold start
      // gets it with the session reply and connects then.
      next = liveStep(next, 'ui-live-start', {}, msg)?.state ?? next;
      /** @type {Effect[]} */
      const connect = next.session ? [{ type: 'ws_connect', csrf: next.session.csrf_token }] : [];
      return { state: next, effects: [...effects, ...connect] };
    }
    case 'navigate':
      return navigate(state, msg.href, msg.replace ?? false, msg);
    case 'popstate': {
      const route = parseHref(msg.path + msg.search);
      return fetchView({ ...state, route, menuOpen: false, confirmDelete: null }, msg, false);
    }
    case 'input':
      if (msg.form.startsWith('lab')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      return { state: onInput(state, msg), effects: [] };
    case 'submit':
      return onSubmit(state, msg);
    case 'action':
      return onAction(state, msg);
    case 'system_theme':
      return { state: { ...state, theme: { ...state.theme, systemDark: msg.dark } }, effects: [] };
    case 'http_result':
      return onHttp(state, msg);
    case 'passkey_result':
      if (state.factors.ceremony?.id === msg.id) return factorsUpdate(state, msg, { request, toast, signedIn: passkeySignedIn, redispatch }) ?? { state, effects: [] };
      return passkeysUpdate(state, msg, { request, toast, signedIn: passkeySignedIn }) ?? { state, effects: [] };
    case 'ws_open':
      return { state: { ...state, wsRefusals: 0, wsStopped: false }, effects: [] };
    case 'ws_message':
      return onSocket(state, msg.message, msg);
    case 'ws_closed': {
      // The socket never opened although the network is up: the upgrade was refused, most likely
      // for an expired request token. Read the session again, so the next attempt carries the
      // current token; the reply counts the refusal (`wsRefusals`).
      /** @type {Effect[]} */
      const lead = [];
      let base = state;
      if (msg.refused && !Object.values(state.pending).some((p) => p.kind === 'session')) {
        const [withRequest, effect] = readSession(state, msg, { refusal: true });
        base = withRequest;
        lead.push(effect);
      }
      // A close while waiting to reconnect is an attempt that failed: it counts as one.
      const waiting = base.ws.status === 'waiting' ? liveStep(base, 'ui-live-reconnect-due', {}, msg) : null;
      const from = waiting ? { ...waiting.state, ws: { ...waiting.state.ws, timers: {} } } : base;
      const closed = liveStep(from, 'ui-live-closed', {}, msg) ?? { state: base, effects: [] };
      return { state: closed.state, effects: [...lead, ...closed.effects] };
    }
    case 'timer': {
      if (msg.name.startsWith('lab.')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      // Live updates gave up after repeated refusals: the attempt that was waiting does not run.
      if (msg.name === 'reconnect' && state.wsStopped) return { state, effects: [] };
      return liveTimer(state, msg) ?? { state, effects: [] };
    }
    default:
      return { state, effects: [] };
  }
}
