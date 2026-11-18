// The frontend kernel's transition function. Pure: no fetch, no socket, no clock, no DOM.
// @ts-check

import { labInit, labUpdate } from './lab.js';

/** @typedef {import('./types.js').Message} Message */
/** @typedef {import('./types.js').Effect} Effect */
/** @typedef {{ state: any, effects: Effect[] }} Result */

const THEME_MODES = ['auto', 'light', 'dark'];
const DESK_VIEWS = ['desk', 'desk_new', 'desk_edit', 'desk_settings', 'lab'];
const EDITOR_FIELDS = ['title', 'slug', 'excerpt', 'body', 'category', 'author', 'tags', 'cover', 'cover_alt', 'featured'];

export function initialState() {
  return {
    ready: false,
    route: { path: '/', query: {} },
    /** @type {import('./types.js').Session | null} */
    session: null,
    site: { name: '', tagline: '' },
    theme: { mode: 'auto', systemDark: false, accent: 'moss', paper: 'linen' },
    view: { status: 'loading', model: null, error: null, request: 0 },
    ws: { status: 'idle', attempts: 0, lastSeq: null, lastN: 0, heartbeatMs: 10000, timer: 0, missedPongs: 0 },
    forms: {
      login: { values: { login: '', password: '' }, busy: false, error: '' },
      editor: null,
      settings: null,
    },
    optimistic: {},
    confirmDelete: null,
    lab: labInit(),
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
export function slugify(title) {
  return title
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80)
    .replace(/-+$/, '');
}

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

function toast(state, kind, text) {
  const id = state.nextId;
  return {
    ...state,
    nextId: id + 1,
    toasts: [...state.toasts.slice(-2), { id, kind, text }],
    announce: text,
  };
}

/** Ask for the view of the current route. `quiet` keeps the current page on screen meanwhile. */
/** @returns {Result} */
function fetchView(state, msg, quiet) {
  const route = state.route;
  const query = Object.entries({ path: route.path, ...route.query })
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join('&');
  const [next, effect] = request(state, 'view', { path: `/api/view?${query}`, meta: { href: hrefOf(route) } }, msg);
  const view = quiet
    ? { ...state.view, request: effect.type === 'http' ? effect.id : 0 }
    : { status: 'loading', model: state.view.model, error: null, request: effect.type === 'http' ? effect.id : 0 };
  return { state: { ...next, view }, effects: [effect] };
}

function authorFor(session, authors) {
  const own = (authors ?? []).find((a) => a.fields.principal && a.fields.principal === session?.principal);
  return String((own ?? authors?.[0])?.id ?? '');
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
      values: {
        title: '', slug: '', excerpt: '', body: '', category: '',
        author: authorFor(state.session, model.data.authors),
        tags: '', cover: '', cover_alt: '', featured: false,
      },
      slugTouched: false, dirty: false, busy: false, errors: {}, formError: '',
      tab: 'write', preview: null, remoteChanged: false, rev: state.nextId,
    };
  }
  const post = model.data.post;
  if (!post) return null;
  if (current?.id === post.id && current.dirty) {
    // Someone else saved while we were typing: keep our text, remember theirs exists.
    return current.version === post.version ? current : { ...current, remoteChanged: true };
  }
  const f = post.fields;
  return {
    key: f.slug,
    id: post.id,
    version: post.version,
    status: f.status,
    values: {
      title: f.title ?? '', slug: f.slug ?? '', excerpt: f.excerpt ?? '', body: f.body ?? '',
      category: f.category == null ? '' : String(f.category),
      author: f.author == null ? '' : String(f.author),
      tags: (f.tags ?? []).join(', '), cover: f.cover ?? '', cover_alt: f.cover_alt ?? '', featured: f.featured === true,
    },
    slugTouched: true, dirty: false, busy: false, errors: {}, formError: '',
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
  const f = site.fields;
  return {
    id: site.id, version: site.version,
    values: { name: f.name ?? '', tagline: f.tagline ?? '', accent: f.accent ?? 'moss', paper: f.paper ?? 'linen' },
    dirty: false, busy: false, errors: {}, formError: '', rev: state.nextId,
  };
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
  const forms = { ...next.forms };
  forms.editor = model.view === 'desk_new' || model.view === 'desk_edit' ? editorFor(next, model) : null;
  forms.settings = model.view === 'desk_settings' ? settingsFor(next, model) : null;
  next = { ...next, forms };
  return next;
}

/** Which form field a failed kernel check is about, and what to tell the writer. */
export function explainCheck(check, object, fieldNames) {
  const [kind, rest = ''] = check.rule.split(/:(.*)/s);
  const name = rest.startsWith(`${object}.`) ? rest.slice(object.length + 1) : rest;
  const direct = fieldNames.includes(name) ? name : null;
  switch (kind) {
    case 'required':
      return { field: direct, text: 'This is required.' };
    case 'unique':
      return { field: direct, text: 'Another entry already uses this value.' };
    case 'type':
    case 'input':
      return { field: direct, text: 'This value is not in the expected form.' };
    case 'reference':
      return { field: direct, text: 'The chosen entry no longer exists.' };
    case 'constraint': {
      // A constraint is named for the field it guards: `title_length` is about `title`.
      const field = [...fieldNames].sort((a, b) => b.length - a.length).find((f) => name === f || name.startsWith(`${f}_`));
      return { field: field ?? null, text: check.detail.replace(/\s*\[.*\]\s*$/s, '') + '.' };
    }
    case 'workflow':
    case 'workflow_state':
      return { field: null, text: 'That change of status is not allowed from here.' };
    case 'policy':
      return { field: null, text: 'You are not allowed to do this.' };
    default:
      return { field: null, text: check.detail };
  }
}

function formErrors(error, object, fieldNames) {
  /** @type {Record<string, string>} */
  const errors = {};
  const general = [];
  for (const check of error?.failed ?? []) {
    const { field, text } = explainCheck(check, object, fieldNames);
    if (field && !errors[field]) errors[field] = text;
    else if (!field) general.push(text);
  }
  let formError = general[0] ?? '';
  if (!Object.keys(errors).length && !formError) {
    formError =
      error?.code === 'conflict'
        ? 'This was changed somewhere else since you opened it. Reload to see the latest version.'
        : error?.code === 'unauthenticated'
          ? 'Your session has ended. Sign in again to save.'
          : error?.message || 'That did not work. Try again.';
  }
  return { errors, formError };
}

function editorFields(values) {
  const tags = values.tags.split(',').map((t) => t.trim()).filter(Boolean);
  return {
    title: values.title.trim(),
    slug: values.slug.trim(),
    excerpt: values.excerpt.trim() || null,
    body: values.body,
    category: values.category ? Number(values.category) : null,
    author: values.author ? Number(values.author) : null,
    tags: tags.length ? tags : null,
    cover: values.cover || null,
    cover_alt: values.cover_alt.trim() || null,
    featured: values.featured ? true : null,
  };
}

/** Save the editor's article, optionally moving it to a new status in the same request. */
/** @returns {Result} */
function saveEditor(state, msg, status) {
  const editor = state.forms.editor;
  if (!editor || editor.busy) return { state, effects: [] };
  /** @type {Record<string, any>} */
  const fields = editorFields(editor.values);
  if (status) fields.status = status;
  const body =
    editor.id == null
      ? { op: 'create', object: 'post', fields: Object.fromEntries(Object.entries(fields).filter(([, v]) => v !== null)) }
      : { op: 'update', object: 'post', id: editor.id, fields, expected_version: editor.version };
  // A new article is always created as a draft; publishing it is a second step the workflow allows.
  if (editor.id == null) delete body.fields.status;
  const [next, effect] = request(
    state, 'editor.save',
    { method: 'POST', path: '/api/records', body, write: true, meta: { status, publishAfterCreate: editor.id == null && status === 'published', slug: fields.slug } },
    msg,
  );
  return {
    state: { ...next, forms: { ...next.forms, editor: { ...editor, busy: true, errors: {}, formError: '' } } },
    effects: [effect],
  };
}

/** @returns {Effect} */
function wsSubscribe(state) {
  return {
    type: 'ws_send',
    message: { type: 'subscribe', req: `r${state.nextId}`, sub: 'site', ...(state.ws.lastSeq == null ? {} : { after: state.ws.lastSeq }) },
  };
}

function reconnect(state) {
  const attempts = state.ws.attempts + 1;
  const token = state.ws.timer + 1;
  return {
    state: { ...state, ws: { ...state.ws, status: 'waiting', attempts, timer: token } },
    token,
    attempts,
  };
}

/** Handle one frame from the server. */
/** @returns {Result} */
function onSocket(state, frame, msg) {
  const ws = state.ws;
  switch (frame.type) {
    case 'welcome': {
      const token = ws.timer + 1;
      const next = { ...state, ws: { ...ws, status: 'open', attempts: 0, lastN: 0, heartbeatMs: frame.heartbeat_ms, timer: token, missedPongs: 0 } };
      /** @type {Effect[]} */
      const effects = [wsSubscribe(next), { type: 'timer', name: 'heartbeat', token, ms: frame.heartbeat_ms }];
      // First connection: nothing was missed before `head`. Later ones resume from where we were.
      if (ws.lastSeq == null) next.ws.lastSeq = frame.head;
      return { state: next, effects };
    }
    case 'event': {
      if (frame.n !== ws.lastN + 1) {
        // A frame went missing on this connection. Start over rather than carry on stale.
        return { state: { ...state, ws: { ...ws, lastSeq: null } }, effects: [{ type: 'ws_close' }] };
      }
      const next = { ...state, ws: { ...ws, lastN: frame.n, lastSeq: Math.max(ws.lastSeq ?? 0, frame.seq) } };
      /** @type {Effect[]} */
      const effects = [{ type: 'ws_send', message: { type: 'ack', n: frame.n } }];
      const notice = frame.notice;
      const model = state.view.model;
      const relevant =
        notice.kind === 'schema' || notice.kind === 'membership' || (model?.depends_on ?? []).includes(notice.object);
      if (notice.kind === 'membership') {
        const [withRequest, effect] = request(next, 'session', { path: '/api/session' }, msg);
        const refreshed = fetchView(withRequest, msg, true);
        return { state: refreshed.state, effects: [...effects, effect, ...refreshed.effects] };
      }
      if (relevant && state.view.status === 'ready') {
        const refreshed = fetchView(next, msg, true);
        return { state: refreshed.state, effects: [...effects, ...refreshed.effects] };
      }
      return { state: next, effects };
    }
    case 'resync': {
      const next = { ...state, ws: { ...ws, lastSeq: frame.head } };
      const refreshed = fetchView(next, msg, true);
      return { state: refreshed.state, effects: refreshed.effects };
    }
    case 'pong':
      // Everything visible up to `head` has already arrived on this ordered connection.
      return { state: { ...state, ws: { ...ws, missedPongs: 0, lastSeq: Math.max(ws.lastSeq ?? 0, frame.head) } }, effects: [] };
    case 'session': {
      if (frame.state === 'active' || !state.session?.authenticated) return { state, effects: [] };
      const text = frame.state === 'expired' ? 'Your session has expired. Sign in again to keep writing.' : 'You have been signed out.';
      const [withRequest, effect] = request(toast(state, 'warn', text), 'session', { path: '/api/session' }, msg);
      const signedOut = { ...withRequest, session: { ...state.session, authenticated: false, csrf_token: undefined, roles: ['public'], may: {}, may_manage_schema: false } };
      const refreshed = fetchView(signedOut, msg, true);
      return { state: refreshed.state, effects: [effect, ...refreshed.effects] };
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

  if (pending.kind.startsWith('lab.')) {
    return labUpdate(state, { ...msg, pending, error }, { request, toast }) ?? { state, effects: [] };
  }

  switch (pending.kind) {
    case 'session': {
      if (!msg.ok) return { state, effects: [] };
      return { state: { ...state, session: msg.body.session }, effects: [] };
    }
    case 'view': {
      if (state.view.request !== msg.id) return { state, effects: [] };
      if (msg.ok) return { state: acceptView(state, msg.body), effects: [] };
      const status = msg.status === 404 ? 'missing' : 'error';
      return {
        state: { ...state, ready: true, view: { status, model: null, error, request: 0 }, forms: { ...state.forms, editor: null, settings: null } },
        effects: [],
      };
    }
    case 'login': {
      const login = state.forms.login;
      if (!msg.ok) {
        const text = error.code === 'throttled' || error.code === 'unauthenticated' ? error.message : 'Signing in did not work. Try again.';
        return {
          state: { ...state, announce: text, forms: { ...state.forms, login: { ...login, busy: false, error: text, values: { ...login.values, password: '' } } }, focus: { target: 'login-password', token: msg.id } },
          effects: [],
        };
      }
      const signedIn = toast(
        { ...state, session: msg.body.session, forms: { ...state.forms, login: { values: { login: '', password: '' }, busy: false, error: '' } } },
        'ok', `Signed in as ${msg.body.session.display_name}.`,
      );
      const target = state.route.query.next?.startsWith('/') && !state.route.query.next.startsWith('//') ? state.route.query.next : '/desk';
      const moved = navigate({ ...signedIn, ws: { ...signedIn.ws, lastSeq: null } }, target, false, msg);
      // The socket was opened as an anonymous visitor; open it again as this person.
      return { state: moved.state, effects: [{ type: 'ws_close' }, ...moved.effects] };
    }
    case 'logout': {
      if (!msg.ok) return { state: toast(state, 'warn', 'Signing out did not work. Try again.'), effects: [] };
      const out = toast({ ...state, session: msg.body.session, ws: { ...state.ws, lastSeq: null } }, 'ok', 'Signed out.');
      const moved = navigate(out, DESK_VIEWS.includes(state.view.model?.view) ? '/' : hrefOf(state.route), true, msg);
      return { state: moved.state, effects: [{ type: 'ws_close' }, ...moved.effects] };
    }
    case 'editor.save': {
      const editor = state.forms.editor;
      if (!editor) return { state, effects: [] };
      if (!msg.ok) {
        const { errors, formError } = formErrors(error, 'post', EDITOR_FIELDS);
        const first = Object.keys(errors)[0];
        return {
          state: {
            ...state,
            announce: formError || 'Some fields need attention.',
            forms: { ...state.forms, editor: { ...editor, busy: false, errors, formError } },
            focus: first ? { target: `editor-${first}`, token: msg.id } : state.focus,
          },
          effects: [],
        };
      }
      const saved = { ...editor, id: msg.body.id, version: msg.body.version, key: pending.slug, busy: false, dirty: false, remoteChanged: false };
      if (pending.publishAfterCreate) {
        const [next, effect] = request(
          { ...state, forms: { ...state.forms, editor: { ...saved, busy: true } } }, 'editor.save',
          { method: 'POST', path: '/api/records', write: true, body: { op: 'update', object: 'post', id: msg.body.id, fields: { status: 'published' }, expected_version: msg.body.version }, meta: { status: 'published', slug: pending.slug } },
          msg,
        );
        return { state: next, effects: [effect] };
      }
      const status = pending.status ?? editor.status;
      const text = pending.status === 'published' ? 'Published.' : pending.status === 'draft' ? 'Moved back to drafts.' : 'Draft saved.';
      const done = toast({ ...state, forms: { ...state.forms, editor: { ...saved, status } } }, 'ok', editor.status === 'published' && !pending.status ? 'Changes saved.' : text);
      const href = `/desk/edit/${pending.slug}`;
      if (hrefOf(state.route) === href) return fetchView(done, msg, true);
      const moved = { ...done, route: parseHref(href) };
      const refreshed = fetchView(moved, msg, true);
      return { state: refreshed.state, effects: [{ type: 'push_url', href, replace: true }, ...refreshed.effects] };
    }
    case 'editor.preview': {
      const editor = state.forms.editor;
      if (!editor || editor.preview?.request !== msg.id) return { state, effects: [] };
      const preview = msg.ok ? { blocks: msg.body.blocks, request: 0, failed: false } : { blocks: editor.preview.blocks, request: 0, failed: true };
      return { state: { ...state, forms: { ...state.forms, editor: { ...editor, preview } } }, effects: [] };
    }
    case 'post.status':
    case 'post.delete': {
      const { [pending.id]: _gone, ...optimistic } = state.optimistic;
      if (msg.ok) {
        const text = pending.kind === 'post.delete' ? 'Deleted.' : pending.status === 'published' ? 'Published.' : 'Moved back to drafts.';
        return fetchView(toast(state, 'ok', text), msg, true);
      }
      const { formError } = formErrors(error, 'post', EDITOR_FIELDS);
      const explained = error.failed?.length ? explainCheck(error.failed[0], 'post', EDITOR_FIELDS).text : formError;
      return { state: toast({ ...state, optimistic }, 'warn', `Not done. ${explained}`), effects: [] };
    }
    case 'settings.save': {
      const settings = state.forms.settings;
      if (!settings) return { state, effects: [] };
      if (!msg.ok) {
        const { errors, formError } = formErrors(error, 'publication', ['name', 'tagline', 'accent', 'paper']);
        return { state: { ...state, announce: formError || 'Some fields need attention.', forms: { ...state.forms, settings: { ...settings, busy: false, errors, formError } } }, effects: [] };
      }
      const done = toast({ ...state, forms: { ...state.forms, settings: { ...settings, busy: false, dirty: false, version: msg.body.version } } }, 'ok', 'Settings saved.');
      return fetchView(done, msg, true);
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
  const forms = state.forms;
  if (msg.form === 'login') {
    return { ...state, forms: { ...forms, login: { ...forms.login, values: { ...forms.login.values, [msg.name]: msg.value } } } };
  }
  if (msg.form === 'editor' && forms.editor) {
    const editor = forms.editor;
    const values = { ...editor.values, [msg.name]: msg.value };
    let slugTouched = editor.slugTouched;
    if (msg.name === 'slug') slugTouched = true;
    if (msg.name === 'title' && !slugTouched) values.slug = slugify(String(msg.value));
    const { [msg.name]: _cleared, ...errors } = editor.errors;
    return { ...state, forms: { ...forms, editor: { ...editor, values, slugTouched, dirty: true, errors, preview: editor.preview ? { ...editor.preview, stale: msg.name === 'body' || editor.preview.stale } : null } } };
  }
  if (msg.form === 'settings' && forms.settings) {
    const settings = forms.settings;
    return { ...state, forms: { ...forms, settings: { ...settings, dirty: true, values: { ...settings.values, [msg.name]: msg.value } } } };
  }
  return state;
}

/** @returns {Result} */
function requestPreview(state, msg) {
  const editor = state.forms.editor;
  if (!editor) return { state, effects: [] };
  const [next, effect] = request(state, 'editor.preview', { method: 'POST', path: '/api/preview', body: { markdown: editor.values.body } }, msg);
  const preview = { blocks: editor.preview?.blocks ?? null, request: effect.type === 'http' ? effect.id : 0, failed: false, stale: false };
  return { state: { ...next, forms: { ...next.forms, editor: { ...editor, tab: 'preview', preview } } }, effects: [effect] };
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
      const [next, effect] = request(state, 'logout', { method: 'POST', path: '/api/logout', body: {}, write: true }, msg);
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
    case 'post.unpublish': {
      const status = msg.name === 'post.publish' ? 'published' : 'draft';
      const [next, effect] = request(
        { ...state, optimistic: { ...state.optimistic, [arg.id]: status } }, 'post.status',
        { method: 'POST', path: '/api/records', write: true, body: { op: 'update', object: 'post', id: arg.id, fields: { status }, expected_version: arg.version }, meta: { id: arg.id, status } },
        msg,
      );
      return { state: next, effects: [effect] };
    }
    case 'post.ask_delete':
      // Deleting takes two presses on the same row; any other action disarms it.
      return { state: { ...state, confirmDelete: arg.id, announce: 'Press Confirm delete to remove the article for good.' }, effects: [] };
    case 'reload':
      return fetchView(state, msg, false);
    case 'post.delete': {
      if (state.confirmDelete !== arg.id) return { state, effects: [] };
      const [next, effect] = request(
        { ...state, confirmDelete: null, optimistic: { ...state.optimistic, [arg.id]: 'deleted' } }, 'post.delete',
        { method: 'POST', path: '/api/records', write: true, body: { op: 'delete', object: 'post', id: arg.id, expected_version: arg.version }, meta: { id: arg.id } },
        msg,
      );
      return { state: next, effects: [effect] };
    }
    case 'dev.expire': {
      const [next, effect] = request(state, 'dev', { method: 'POST', path: '/api/dev/expire-session', body: {} }, msg);
      return { state: next, effects: [effect] };
    }
    default:
      if (msg.name.startsWith('lab.')) return labUpdate(state, msg, { request, toast }) ?? { state, effects: [] };
      return { state, effects: [] };
  }
}

/** @returns {Result} */
function onSubmit(state, msg) {
  switch (msg.form) {
    case 'login': {
      const values = { login: String(msg.values.login ?? ''), password: String(msg.values.password ?? '') };
      if (!values.login.trim() || !values.password) {
        const error = 'Enter your name and password.';
        return { state: { ...state, announce: error, forms: { ...state.forms, login: { ...state.forms.login, values, error } } }, effects: [] };
      }
      const [next, effect] = request(state, 'login', { method: 'POST', path: '/api/login', body: values, meta: {} }, msg);
      return { state: { ...next, forms: { ...next.forms, login: { values, busy: true, error: '' } } }, effects: [effect] };
    }
    case 'search': {
      const q = String(msg.values.q ?? '').trim();
      return navigate(state, q ? `/search?q=${encodeURIComponent(q)}` : '/search', false, msg);
    }
    case 'editor':
      return saveEditor(state, msg, null);
    case 'settings': {
      const settings = state.forms.settings;
      if (!settings || settings.busy) return { state, effects: [] };
      const v = settings.values;
      const [next, effect] = request(
        state, 'settings.save',
        { method: 'POST', path: '/api/records', write: true, body: { op: 'update', object: 'publication', id: settings.id, fields: { name: v.name.trim(), tagline: v.tagline.trim() || null, accent: v.accent, paper: v.paper }, expected_version: settings.version } },
        msg,
      );
      return { state: { ...next, forms: { ...next.forms, settings: { ...settings, busy: true, errors: {}, formError: '' } } }, effects: [effect] };
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
  switch (msg.type) {
    case 'init': {
      const route = parseHref(msg.path + msg.search);
      const mode = THEME_MODES.includes(msg.storedTheme ?? '') ? msg.storedTheme : 'auto';
      let next = { ...state, route, theme: { ...state.theme, mode, systemDark: msg.systemDark } };
      /** @type {Effect[]} */
      const effects = [];
      const boot = msg.bootstrap;
      if (boot?.session?.session) {
        next = { ...next, session: boot.session.session };
      } else {
        const [withRequest, effect] = request(next, 'session', { path: '/api/session' }, msg);
        next = withRequest;
        effects.push(effect);
      }
      if (boot?.view && boot.path === route.path) {
        next = acceptView(next, boot.view);
      } else {
        const fetched = fetchView(next, msg, false);
        next = fetched.state;
        effects.push(...fetched.effects);
      }
      return { state: { ...next, ws: { ...next.ws, status: 'connecting' } }, effects: [...effects, { type: 'ws_connect' }] };
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
      const { state: next, token, attempts } = reconnect(state);
      // Exponential backoff with jitter, capped at eight seconds.
      const ms = Math.min(8000, 250 * 2 ** Math.min(attempts, 5)) * (0.5 + msg.rand / 2);
      return { state: next, effects: [{ type: 'timer', name: 'reconnect', token, ms: Math.round(ms) }] };
    }
    case 'timer': {
      if (msg.token !== state.ws.timer) return { state, effects: [] };
      if (msg.name === 'reconnect') {
        return { state: { ...state, ws: { ...state.ws, status: 'connecting' } }, effects: [{ type: 'ws_connect' }] };
      }
      if (msg.name === 'heartbeat' && state.ws.status === 'open') {
        if (state.ws.missedPongs >= 2) return { state, effects: [{ type: 'ws_close' }] };
        return {
          state: { ...state, ws: { ...state.ws, missedPongs: state.ws.missedPongs + 1 } },
          effects: [
            { type: 'ws_send', message: { type: 'ping', nonce: msg.token } },
            { type: 'timer', name: 'heartbeat', token: msg.token, ms: state.ws.heartbeatMs },
          ],
        };
      }
      return { state, effects: [] };
    }
    default:
      return { state, effects: [] };
  }
}
