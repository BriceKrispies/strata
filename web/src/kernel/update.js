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
import { labEnter, labInit, labUpdate } from './lab.js';
import { sessionsEnter, sessionsInit, sessionsUpdate } from './sessions.js';
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

export function initialState() {
  return {
    ready: false,
    route: { path: '/', query: {} },
    /** @type {import('./types.js').Session | null} */
    session: null,
    sessionFlow: sessionInit(),
    site: { name: '', tagline: '' },
    theme: { mode: 'auto', systemDark: false, accent: 'moss', paper: 'linen' },
    view: { status: 'loading', model: null, error: null, request: 0 },
    ws: liveInit(),
    forms: {
      login: { values: forms.initialValues(SIGN_IN, {}), busy: false, error: '', flow: submitInit() },
      editor: null,
      settings: null,
    },
    optimistic: {},
    confirmDelete: null,
    lab: labInit(),
    sessions: sessionsInit(),
    /** @type {{ id: number, kind: string, text: string }[]} */
    toasts: [],
    announce: '',
    menuOpen: false,
    nextId: 1,
    pending: {},
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
 * @param {{ method?: 'GET' | 'POST', path: string, body?: any, write?: boolean, meta?: any }} spec
 * @param {Message} msg
 * @returns {[any, Effect]}
 */
export function request(state, kind, spec, msg) {
  const id = state.nextId;
  const correlation = `c-${id}-${Math.floor(msg.rand * 0xffffff).toString(16).padStart(6, '0')}`;
  const next = {
    ...state,
    nextId: id + 1,
    pending: { ...state.pending, [id]: { kind, correlation, ...(spec.meta ?? {}) } },
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
      correlation,
    },
  ];
}

/**
 * Register the call of a contract, as the generated request builder describes it.
 * @param {any} state @param {string} kind @param {string} contract
 * @param {Record<string, any>} inputs @param {Message} msg @param {any} [meta]
 * @returns {[any, Effect]}
 */
function call(state, kind, contract, inputs, msg, meta = {}) {
  const req = contracts.request(contract, inputs, state.route.path);
  return request(state, kind, { ...req, meta: { contract, inputs, ...meta } }, msg);
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

/** The screen the current address opens. */
function screenOf(state) {
  return matchRoute(state.route.path, state.route.query);
}

/** The principal as the generated policy checks read it. */
function principalOf(session) {
  if (!session?.authenticated) return null;
  const roles = session.roles ?? [];
  return { id: session.principal, tenant: session.tenant, kind: 'member', roles, role: roles.find((r) => r !== 'public') ?? 'public' };
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
  const [next, effect] = viewModel
    ? call(state, 'view', viewModel, { ...screen?.params, ...screen?.query }, msg, { href: hrefOf(route) })
    : request(state, 'view', { path: `/api/view?path=${encodeURIComponent(route.path)}`, meta: { href: hrefOf(route) } }, msg);
  const view = quiet
    ? { ...state.view, request: effect.type === 'http' ? effect.id : 0 }
    : { status: 'loading', model: state.view.model, error: null, request: effect.type === 'http' ? effect.id : 0 };
  return { state: { ...next, view }, effects: [effect] };
}

/** Ask for the session again. */
function readSession(state, msg) {
  return call(state, 'session', 'read-session-view', {}, msg);
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
function acceptView(state, model) {
  const site = model.data.site?.fields;
  let next = {
    ...state,
    ready: true,
    view: { status: 'ready', model, error: null, request: 0 },
    site: site ? { name: site.name ?? '', tagline: site.tagline ?? '' } : state.site,
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

/** Save the editor's article, optionally moving it to a new status in the same request. */
/** @returns {Result} */
function saveEditor(state, msg, status) {
  const editor = state.forms.editor;
  if (!editor) return { state, effects: [] };
  const stepped = submitStep(editor.flow, 'ui-form-submitted', { complete: 1 }, msg.at);
  if (!stepped?.sends) return { state, effects: [] };
  const form = editorSubmission(editor, status);
  const contract = forms.FORMS[form].contract;
  const [next, effect] = call(state, 'editor.save', contract, forms.inputs(form, editor.values, scopeOf(state)), msg, { form, status });
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
    case 'welcome':
      return liveStep(state, 'ui-live-welcome', { head: frame.head, 'heartbeat-ms': frame.heartbeat_ms }, msg) ?? { state, effects: [] };
    case 'event': {
      const stepped = liveStep(state, 'ui-live-frame', { n: frame.n, seq: frame.seq }, msg);
      if (!stepped) return { state, effects: [] };
      // A frame went missing on this connection: the machine starts over rather than carry on stale.
      if (stepped.effects.some((e) => e.type === 'ws_close')) return stepped;
      const notice = frame.notice;
      if (notice.kind === 'membership') {
        const [withRequest, effect] = readSession(stepped.state, msg);
        return refreshAfter({ state: withRequest, effects: [...stepped.effects, effect] }, msg);
      }
      const relevant = live.refreshes(screenOf(state)?.screen ?? '', 'ui-notice-received', { kind: notice.kind, object: notice.object }, state.view.model);
      return relevant && state.view.status === 'ready' ? refreshAfter(stepped, msg) : stepped;
    }
    case 'resync':
      // Too far behind to replay: carry on from the server's head, with the view read again.
      return refreshAfter(liveStep(state, 'ui-live-resync', { head: frame.head }, msg) ?? { state, effects: [] }, msg);
    case 'pong':
      // Everything visible up to `head` has already arrived on this ordered connection.
      return liveStep(state, 'ui-live-pong', { head: frame.head }, msg) ?? { state, effects: [] };
    case 'session': {
      if (frame.state === 'active') return { state, effects: [] };
      // Our own sign-out ends the session too; its reply will say so, once.
      if (Object.values(state.pending).some((p) => p.kind === 'logout')) return { state, effects: [] };
      const ended = sessionStep(state, 'ui-session-ended', {}, msg.at);
      if (!ended) return { state, effects: [] };
      const text = frame.state === 'expired' ? 'Your session has expired. Sign in again to keep writing.' : 'You have been signed out.';
      let next = toast(ended.state, 'warn', text);
      /** @type {Effect[]} */
      const effects = [];
      if (ended.readSession) {
        const [withRequest, effect] = readSession(next, msg);
        next = withRequest;
        effects.push(effect);
      }
      next = { ...next, session: { ...state.session, authenticated: false, csrf_token: undefined, roles: ['public'], may: {}, may_manage_schema: false } };
      const refresh = live.refreshes(screenOf(state)?.screen ?? '', 'ui-session-ended', {}, state.view.model);
      return refresh ? refreshAfter({ state: next, effects }, msg) : { state: next, effects };
    }
    case 'run':
      return labUpdate(state, { ...msg, type: 'lab_run', frame }, { request, toast }) ?? { state, effects: [] };
    default:
      return { state, effects: [] };
  }
}

/** @returns {Result} */
function onHttp(state, msg) {
  const pending = state.pending[msg.id];
  if (!pending) return { state, effects: [] };
  const { [msg.id]: _done, ...rest } = state.pending;
  state = { ...state, pending: rest };
  const error = msg.ok ? null : msg.body && typeof msg.body === 'object' ? msg.body : { code: 'unavailable', message: 'The site could not be reached.' };
  const outputs = msg.ok && pending.contract ? contracts.decode(pending.contract, msg.body, pending.inputs) : null;

  if (pending.kind.startsWith('lab.')) {
    return labUpdate(state, { ...msg, pending, error }, { request, toast }) ?? { state, effects: [] };
  }
  if (pending.kind.startsWith('sessions.')) {
    return sessionsUpdate(state, { ...msg, pending, error }, { request, toast }) ?? { state, effects: [] };
  }

  switch (pending.kind) {
    case 'session': {
      if (!msg.ok) return { state, effects: [] };
      const known = sessionRead({ ...state, session: outputs.session }, outputs.session.authenticated === true, msg.at);
      // A cold start could not open the socket before it knew the request token the gateway
      // issued, so it opens it now.
      /** @type {Effect[]} */
      const open = state.session ? [] : [{ type: 'ws_connect', csrf: outputs.session.csrf_token }];
      if (state.view.model?.view === 'lab') {
        const entered = labEnter(known, msg, { request });
        return { state: entered.state, effects: [...open, ...entered.effects] };
      }
      return { state: known, effects: open };
    }
    case 'view': {
      if (state.view.request !== msg.id) return { state, effects: [] };
      if (msg.ok) {
        const accepted = acceptView(state, decodeView(state, msg.body));
        if (msg.body.view === 'desk') return sessionsEnter(accepted, msg, { request });
        return msg.body.view === 'lab' ? labEnter(accepted, msg, { request }) : { state: accepted, effects: [] };
      }
      const status = msg.status === 404 ? 'missing' : 'error';
      return {
        state: { ...state, ready: true, view: { status, model: null, error, request: 0 }, forms: { ...state.forms, editor: null, settings: null } },
        effects: [],
      };
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
      const outcome = forms.succeeded(SIGN_IN, scopeOf(state, { form: pending.values, response: outputs }));
      let signedIn = { ...state, session: outputs.session, forms: { ...state.forms, login: { values: forms.initialValues(SIGN_IN, {}), busy: busy(flow), error: '', flow } } };
      signedIn = sessionStep(signedIn, 'ui-signed-in', {}, msg.at)?.state ?? signedIn;
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
        return {
          state: {
            ...state,
            announce,
            forms: { ...state.forms, editor: { ...editor, flow, busy: busy(flow), errors, formError } },
            focus: first ? { target: `editor-${first}`, token: msg.id } : state.focus,
          },
          effects: [],
        };
      }
      let flow = submitStep(editor.flow, 'ui-form-accepted', {}, msg.at)?.flow ?? editor.flow;
      const response = pending.first ? forms.bothResponses(pending.first, outputs) : outputs;
      const saved = { ...editor, id: response.article_id, version: response.version, key: response.slug, dirty: false, remoteChanged: false };
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
  const moved = { ...state, route, menuOpen: false, focus: { target: 'main', token: state.nextId } };
  const fetched = fetchView(moved, msg, false);
  /** @type {Effect[]} */
  const effects = same && !replace ? [] : [{ type: 'push_url', href: hrefOf(route), replace }];
  return { state: fetched.state, effects: [...effects, { type: 'scroll_top' }, ...fetched.effects] };
}

function onInput(state, msg) {
  const f = state.forms;
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

/** The action a desk row's button presses. */
const ROW_ACTIONS = { 'post.publish': 'act-desk-publish', 'post.unpublish': 'act-desk-unpublish', 'post.delete': 'act-desk-delete' };

/**
 * Press a per-row action of the desk on the row of this article.
 * @returns {Result}
 */
function rowAction(state, msg, action, arg, marker) {
  const model = state.view.model;
  const item = actions.rows(action, model).find((row) => row.article_id === arg.id) ?? { article_id: arg.id, version: arg.version };
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
  if (state.confirmDelete !== null && msg.name !== 'post.delete') state = { ...state, confirmDelete: null };
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
    case 'post.publish':
      return rowAction(state, msg, ROW_ACTIONS[msg.name], arg, 'published');
    case 'post.unpublish':
      return rowAction(state, msg, ROW_ACTIONS[msg.name], arg, 'draft');
    case 'post.ask_delete':
      // Deleting takes two presses on the same row; any other action disarms it.
      return { state: { ...state, confirmDelete: arg.id, announce: actions.ACTIONS[ROW_ACTIONS['post.delete']].confirmAnnounce ?? '' }, effects: [] };
    case 'reload':
      return fetchView(state, msg, false);
    case 'post.delete': {
      if (!actions.ACTIONS[ROW_ACTIONS[msg.name]].confirm || state.confirmDelete !== arg.id) return { state, effects: [] };
      return rowAction({ ...state, confirmDelete: null }, msg, ROW_ACTIONS[msg.name], arg, 'deleted');
    }
    case 'dev.expire': {
      const [next, effect] = request(state, 'dev', { method: 'POST', path: '/api/dev/expire-session', write: true, body: {} }, msg);
      return { state: next, effects: [effect] };
    }
    default:
      if (msg.name.startsWith('lab.')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      if (msg.name.startsWith('sessions.')) return sessionsUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      return { state, effects: [] };
  }
}

/** @returns {Result} */
function onSubmit(state, msg) {
  switch (msg.form) {
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
  const result = step(state, msg);
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
      let next = { ...state, route, theme: { ...state.theme, mode, systemDark: msg.systemDark } };
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
        next = acceptView(next, decodeView(next, boot.view));
        if (boot.view.view === 'lab') {
          const entered = labEnter(next, msg, { request });
          next = entered.state;
          effects.push(...entered.effects);
        }
        if (boot.view.view === 'desk') {
          const entered = sessionsEnter(next, msg, { request });
          next = entered.state;
          effects.push(...entered.effects);
        }
      } else {
        const fetched = fetchView(next, msg, false);
        next = fetched.state;
        effects.push(...fetched.effects);
      }
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
      return fetchView({ ...state, route, menuOpen: false }, msg, false);
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
    case 'ws_open':
      return { state, effects: [] };
    case 'ws_message':
      return onSocket(state, msg.message, msg);
    case 'ws_closed': {
      // A close while waiting to reconnect is an attempt that failed: it counts as one.
      const waiting = state.ws.status === 'waiting' ? liveStep(state, 'ui-live-reconnect-due', {}, msg) : null;
      const from = waiting ? { ...waiting.state, ws: { ...waiting.state.ws, timers: {} } } : state;
      return liveStep(from, 'ui-live-closed', {}, msg) ?? { state, effects: [] };
    }
    case 'timer': {
      if (msg.name.startsWith('lab.')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      return liveTimer(state, msg) ?? { state, effects: [] };
    }
    default:
      return { state, effects: [] };
  }
}
