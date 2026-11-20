// Impact Lab part of the frontend kernel. Pure, like the rest of it.
//
// The Lab holds no domain logic: it sends a proposed change as text, the backend parses it,
// runs the simulation with the production kernel, and sends back what happened. This module
// only tracks what was asked, how far the run has got, and what came back.
// @ts-check

/** What the page says when the site moved on after the revision a projection was asked at. */
export const STALE = 'The site changed since this projection: project again.';

const TABS = ['projection', 'metadata', 'evidence', 'diagnostics'];

export function labInit() {
  return {
    tab: 'projection',
    overview: null,
    loading: false,
    error: '',
    form: { change: 'require post.excerpt', existing: 'grandfather', source: 'generated', steps: '60', seed: '1', faults: false },
    formError: '',
    run: null,
    result: null,
    replay: null,
    verification: null,
    diagnostics: null,
    /** What came of the apply of the projection shown. */
    applied: null,
    applyError: '',
    timer: 0,
  };
}

/** An apply is waiting for the site's answer. */
export function applying(state) {
  return Object.values(state.pending).some((p) => p.kind === 'lab.apply');
}

/**
 * Whether the projection shown may be applied: it is finished, the kernel accepted the change, and
 * it was taken at the revision the page last saw the site at, so the site has not moved on to the
 * page's knowledge. The backend decides again at the commit and refuses a site that has.
 */
export function canApply(state) {
  const lab = state.lab;
  const shown = lab.overview?.tenant?.revision;
  return lab.run?.status === 'finished'
    && Boolean(lab.result)
    && lab.result.change_outcome?.result === 'accepted'
    && Number.isInteger(shown)
    && lab.result.snapshot?.revision === shown
    && !applying(state)
    && lab.applied === null;
}

function withLab(state, patch) {
  return { ...state, lab: { ...state.lab, ...patch } };
}

/** Called when the Lab page has been loaded: fetch its overview if we do not have one. */
export function labEnter(state, msg, { request }) {
  if (state.lab.overview || state.lab.loading || !state.session?.may_manage_schema) return { state, effects: [] };
  const [next, effect] = request(withLab(state, { loading: true, error: '' }), 'lab.overview', { path: '/api/lab/overview' }, msg);
  return { state: next, effects: [effect] };
}

const POLL_MS = 1500;
const POLL_CAP_MS = 15000;
/** Consecutive failed polls after which the run is no longer followed. */
const POLL_FAILURES = 8;

function poll(state, ms = POLL_MS) {
  const token = state.lab.timer + 1;
  return { state: withLab(state, { timer: token }), effect: { type: 'timer', name: 'lab.poll', token, ms } };
}

/**
 * @returns {{ state: any, effects: any[] } | null}
 */
export function labUpdate(state, msg, { request, toast }) {
  const lab = state.lab;
  switch (msg.type) {
    case 'input': {
      const value = msg.name === 'faults' ? msg.value === true : String(msg.value);
      return { state: withLab(state, { form: { ...lab.form, [msg.name]: value }, formError: '' }), effects: [] };
    }
    case 'submit': {
      if (lab.run?.status === 'running') return { state, effects: [] };
      const form = { ...lab.form, ...Object.fromEntries(Object.entries(msg.values).map(([k, v]) => [k, k === 'faults' ? v === true : String(v)])) };
      /** @type {Record<string, unknown>} */
      const body = {
        change: form.change,
        existing: form.existing,
        source: form.source,
        steps: Math.max(5, Math.min(300, Number(form.steps) || 60)),
        seed: Math.max(0, Math.floor(Number(form.seed) || 0)),
        faults: form.faults === true,
      };
      // The projection is taken at the revision the page showed; if the site has moved on, the
      // backend refuses and the person projects again.
      const shown = lab.overview?.tenant?.revision;
      if (Number.isInteger(shown)) {
        body.revision = shown;
        body.require_current = true;
      }
      const [next, effect] = request(
        withLab(state, { form, formError: '', run: { id: null, status: 'starting', progress: null }, result: null }),
        'lab.run', { method: 'POST', path: '/api/lab/run', body, write: true }, msg,
      );
      return { state: { ...next, announce: 'Starting the projection.' }, effects: [effect] };
    }
    case 'action': {
      const name = msg.name.slice(4);
      if (name === 'tab' && TABS.includes(msg.arg)) {
        const moved = withLab(state, { tab: msg.arg });
        if (msg.arg === 'diagnostics' && !lab.diagnostics) {
          const [next, effect] = request(moved, 'lab.diagnostics', { path: '/api/diagnostics' }, msg);
          return { state: next, effects: [effect] };
        }
        return { state: moved, effects: [] };
      }
      if (name === 'preset') {
        const preset = lab.overview?.presets.find((p) => p.id === msg.arg);
        if (!preset) return { state, effects: [] };
        return { state: withLab(state, { form: { ...lab.form, change: preset.change }, formError: '', formRev: (lab.formRev ?? 0) + 1 }), effects: [] };
      }
      if (name === 'cancel' && lab.run?.id) {
        return { state, effects: [{ type: 'ws_send', message: { type: 'cancel', req: `r${state.nextId}`, run: lab.run.id } }] };
      }
      if (name === 'apply') {
        if (!canApply(state)) return { state, effects: [] };
        const [next, effect] = request(
          withLab(state, { applyError: '' }),
          'lab.apply', { method: 'POST', path: '/api/lab/apply', body: { run: lab.run.id }, write: true, meta: { retry: { name: msg.name, arg: msg.arg } } }, msg,
        );
        return { state: { ...next, announce: 'Applying the change.' }, effects: [effect] };
      }
      if (name === 'replay') {
        const [next, effect] = request(withLab(state, { replay: { pending: true } }), 'lab.replay', { method: 'POST', path: '/api/lab/replay', body: {}, write: true }, msg);
        return { state: next, effects: [effect] };
      }
      if (name === 'verify') {
        const [next, effect] = request(withLab(state, { verification: { pending: true } }), 'lab.verify', { method: 'POST', path: '/api/lab/verify', body: {}, write: true }, msg);
        return { state: { ...next, announce: 'Checking the proofs. This takes a few seconds.' }, effects: [effect] };
      }
      if (name === 'diagnostics') {
        const [next, effect] = request(state, 'lab.diagnostics', { path: '/api/diagnostics' }, msg);
        return { state: next, effects: [effect] };
      }
      return { state, effects: [] };
    }
    case 'timer': {
      if (msg.token !== lab.timer || lab.run?.status !== 'running') return { state, effects: [] };
      const [next, effect] = request(state, 'lab.result', { path: `/api/lab/runs/${lab.run.id}`, meta: { run: lab.run.id } }, msg);
      return { state: next, effects: [effect] };
    }
    case 'lab_run': {
      const frame = msg.frame;
      if (frame.kind === 'verification') {
        const text = frame.payload.all_checked ? 'The proofs check and the model agrees with the kernel.' : 'The verification run found a problem.';
        return { state: toast(withLab(state, { verification: frame.payload }), frame.payload.all_checked ? 'ok' : 'warn', text), effects: [] };
      }
      if (!lab.run || lab.run.id !== frame.run) return { state, effects: [] };
      if (frame.kind === 'progress') {
        return { state: withLab(state, { run: { ...lab.run, progress: frame.payload } }), effects: [] };
      }
      if (frame.kind === 'cancelled') {
        return { state: toast(withLab(state, { run: { ...lab.run, status: 'cancelled' } }), 'ok', 'The projection was cancelled.'), effects: [] };
      }
      if (frame.kind === 'finished') {
        const [next, effect] = request(state, 'lab.result', { path: `/api/lab/runs/${frame.run}`, meta: { run: frame.run } }, msg);
        return { state: next, effects: [effect] };
      }
      return { state, effects: [] };
    }
    case 'http_result': {
      const { pending, error } = msg;
      switch (pending.kind) {
        case 'lab.overview':
          if (!msg.ok) return { state: withLab(state, { loading: false, error: error.message }), effects: [] };
          return { state: withLab(state, { loading: false, overview: msg.body, verification: lab.verification ?? msg.body.verification }), effects: [] };
        case 'lab.run': {
          if (!msg.ok && (error.code === 'conflict' || msg.status === 409)) {
            const text = STALE;
            const [next, effect] = request(withLab(state, { run: null, formError: text }), 'lab.overview', { path: '/api/lab/overview' }, msg);
            return { state: { ...next, announce: text, focus: { target: 'lab-change', token: msg.id } }, effects: [effect] };
          }
          if (!msg.ok) {
            return { state: { ...withLab(state, { run: null, formError: error.message }), announce: error.message, focus: { target: 'lab-change', token: msg.id } }, effects: [] };
          }
          const started = withLab(state, { run: { id: msg.body.run, status: 'running', progress: null, steps: msg.body.steps } });
          const { state: next, effect } = poll(started);
          return { state: next, effects: [effect] };
        }
        case 'lab.result': {
          if (!lab.run || lab.run.id !== pending.run || lab.run.status !== 'running') return { state, effects: [] };
          if (msg.ok && msg.body.status === 'finished') {
            const changes = msg.body.result.comparison.outcome_changes.length;
            const text = changes ? `Projection finished: ${changes} operation${changes === 1 ? '' : 's'} would end differently.` : 'Projection finished: nothing would end differently.';
            const done = withLab(state, { run: { ...lab.run, status: 'finished' }, result: msg.body.result });
            return { state: { ...toast(done, 'ok', text), focus: { target: 'lab-result', token: msg.id } }, effects: [] };
          }
          if (msg.ok && msg.body.status === 'cancelled') {
            return { state: withLab(state, { run: { ...lab.run, status: 'cancelled' } }), effects: [] };
          }
          if (!msg.ok && msg.status === 404) {
            return { state: withLab(state, { run: null, formError: 'The run is no longer available. Start it again.' }), effects: [] };
          }
          // Not signed in (any more): following the run cannot work, so stop.
          if (!msg.ok && (msg.status === 401 || msg.status === 403)) {
            return { state: withLab(state, { run: null, formError: 'You are not signed in. Sign in again to follow the projection.' }), effects: [] };
          }
          // Any other failure backs off, doubling up to a cap, and ends after enough in a row.
          const failures = msg.ok ? 0 : (lab.run.failures ?? 0) + 1;
          if (failures >= POLL_FAILURES) {
            return { state: withLab(state, { run: null, formError: 'The projection could not be followed. Start it again.' }), effects: [] };
          }
          const wait = Math.min(POLL_CAP_MS, POLL_MS * 2 ** failures);
          const { state: next, effect } = poll(withLab(state, { run: { ...lab.run, failures } }), wait);
          return { state: next, effects: [effect] };
        }
        case 'lab.apply': {
          if (msg.ok) {
            const done = withLab(state, { applied: { version: msg.body.version, revision: msg.body.revision }, applyError: '' });
            // The site is at a new revision: the page reads its overview again.
            const [next, effect] = request(toast(done, 'ok', `The change was applied. The schema is now at version ${msg.body.version}.`), 'lab.overview', { path: '/api/lab/overview' }, msg);
            return { state: { ...next, announce: 'The change was applied.' }, effects: [effect] };
          }
          if (error.code === 'conflict' || msg.status === 409) {
            // The site moved on after the projection: say so, and read the site again so that the
            // page shows the revision it is at now.
            const [next, effect] = request(withLab(state, { applyError: STALE }), 'lab.overview', { path: '/api/lab/overview' }, msg);
            return { state: { ...next, announce: STALE }, effects: [effect] };
          }
          return { state: { ...withLab(state, { applyError: error.message }), announce: error.message }, effects: [] };
        }
        case 'lab.replay':
          return { state: withLab(state, { replay: msg.ok ? msg.body : { failed: error.message } }), effects: [] };
        case 'lab.verify':
          if (msg.ok) return { state, effects: [] };
          return { state: withLab(state, { verification: { failed: error.message } }), effects: [] };
        case 'lab.diagnostics':
          return { state: withLab(state, { diagnostics: msg.ok ? msg.body : { failed: error.message } }), effects: [] };
        default:
          return { state, effects: [] };
      }
    }
    default:
      return null;
  }
}

export function labView(state) {
  const lab = state.lab;
  return {
    kind: 'lab',
    tab: lab.tab,
    loading: lab.loading || (!lab.overview && !lab.error),
    error: lab.error,
    overview: lab.overview,
    form: lab.form,
    formRev: lab.formRev ?? 0,
    formError: lab.formError,
    run: lab.run,
    result: lab.result,
    replay: lab.replay,
    verification: lab.verification,
    diagnostics: lab.diagnostics,
    canApply: canApply(state),
    applying: applying(state),
    applied: lab.applied,
    applyError: lab.applyError,
    // The prompt that asks for a fresh authentication, when the apply was refused for want of one.
    factors: state.factors,
  };
}
