// Impact Lab: the developer-facing view of simulation, replay, verification and diagnostics.
// Everything shown here was computed by the backend with the production kernel.
// @ts-check

import { h } from '../components/dom.js';
import { date } from '../components/parts.js';

const TABS = [
  ['projection', 'Projection'],
  ['metadata', 'Metadata'],
  ['evidence', 'Evidence'],
  ['diagnostics', 'Diagnostics'],
];

const CLASS_LABEL = {
  proven: 'Proven',
  reproduced: 'Reproduced',
  sound_approximation: 'Sound approximation',
  empirical: 'Estimated',
  unknown: 'Unknown',
};

function badge(cls) {
  return h('span', { class: `evidence evidence--${cls}` }, CLASS_LABEL[cls] ?? cls);
}

function arg(value) {
  return JSON.stringify(value);
}

function short(hash) {
  return h('code', { title: hash }, String(hash ?? '').slice(0, 12));
}

function table(caption, heads, rows, extra = {}) {
  return h(
    'div',
    { class: 'scroll', tabindex: '0', role: 'group', 'aria-label': caption },
    h(
      'table',
      { class: 'grid', ...extra },
      h('caption', { class: 'visually-hidden' }, caption),
      h('thead', null, h('tr', null, heads.map((head) => h('th', { scope: 'col', key: head }, head)))),
      h('tbody', null, rows),
    ),
  );
}

function verdict(text) {
  const accepted = text === 'accepted';
  return h('span', { class: accepted ? 'verdict verdict--yes' : 'verdict verdict--no' }, accepted ? 'Accepted' : text.replace('rejected: ', 'Rejected: '));
}

function value(v) {
  if (v === 'Null' || v == null) return 'empty';
  const [kind, inner] = Object.entries(v)[0] ?? [];
  if (kind === 'Str') return `“${String(inner).length > 60 ? `${String(inner).slice(0, 60)}…` : inner}”`;
  if (kind === 'List') return `[${inner.map(value).join(', ')}]`;
  if (kind === 'Time') return new Date(inner).toISOString().slice(0, 16).replace('T', ' ');
  return String(inner);
}

function checks(list) {
  if (!list?.length) return null;
  return h('ul', { class: 'checks' }, list.map((c, i) => h('li', { key: i }, h('code', null, c.rule), ' ', c.detail)));
}

function form(page) {
  const f = page.form;
  const running = page.run?.status === 'running' || page.run?.status === 'starting';
  const radio = (name, val, label, hint) =>
    h(
      'label',
      { class: 'choice' },
      h('input', { type: 'radio', name, value: val, checked: f[name] === val }),
      h('span', null, h('strong', null, label), h('span', { class: 'choice__hint' }, hint)),
    );
  return h(
    'form',
    { class: 'form lab-form', 'data-form': 'lab', novalidate: true },
    h(
      'div',
      { class: 'field' },
      h('span', { class: 'field__label', id: 'lab-presets' }, 'Start from an example'),
      h(
        'div',
        { class: 'presets', role: 'group', 'aria-labelledby': 'lab-presets' },
        (page.overview?.presets ?? []).map((p) =>
          h('button', { key: p.id, type: 'button', class: 'button button--small button--quiet', title: p.about, 'data-action': 'lab.preset', 'data-arg': arg(p.id) }, p.title),
        ),
      ),
    ),
    h(
      'div',
      { class: page.formError ? 'field field--error' : 'field' },
      h('label', { for: 'lab-change' }, 'Proposed change'),
      h(
        'p',
        { class: 'field__hint', id: 'lab-change-hint' },
        'One change per line: require post.excerpt, optional post.excerpt, unique post.title, field post.subtitle: string, remove field post.tags, constraint post.name: expression :: message, remove constraint post.name.',
      ),
      h('textarea', {
        id: 'lab-change', name: 'change', rows: 3, class: 'lab-form__change', spellcheck: 'false', value: f.change, 'data-rev': page.formRev,
        'aria-describedby': page.formError ? 'lab-change-hint lab-change-error' : 'lab-change-hint', 'aria-invalid': page.formError ? 'true' : null,
      }),
      page.formError ? h('p', { class: 'field__error', id: 'lab-change-error', role: 'alert' }, page.formError) : null,
    ),
    h(
      'fieldset',
      { class: 'fieldset' },
      h('legend', null, 'Records that already break the new rule'),
      radio('existing', 'grandfather', 'Leave them as they are', 'The change is accepted; those records are refused the next time someone edits them.'),
      radio('existing', 'reject', 'Refuse the change', 'The kernel rejects the change while any record would not conform.'),
    ),
    h(
      'fieldset',
      { class: 'fieldset' },
      h('legend', null, 'Activity to replay'),
      radio('source', 'generated', 'Generated activity', 'Plausible operations made up from the schema and a seed, starting from today’s data.'),
      radio('source', 'journal', 'What actually happened', 'Every committed operation since the publication was created, replayed from nothing.'),
      h(
        'div',
        { class: 'field-row' },
        h('div', { class: 'field' }, h('label', { for: 'lab-steps' }, 'Operations to generate'), h('input', { id: 'lab-steps', name: 'steps', type: 'text', inputmode: 'numeric', value: f.steps })),
        h('div', { class: 'field' }, h('label', { for: 'lab-seed' }, 'Seed'), h('input', { id: 'lab-seed', name: 'seed', type: 'text', inputmode: 'numeric', value: f.seed })),
      ),
      h(
        'div',
        { class: 'field field--check' },
        h('input', { id: 'lab-faults', name: 'faults', type: 'checkbox', checked: f.faults === true }),
        h('label', { for: 'lab-faults' }, 'Inject faults: lost commit races and dropped event deliveries'),
      ),
    ),
    h(
      'div',
      { class: 'form__actions' },
      h('button', { class: 'button', type: 'submit', disabled: running }, running ? 'Running…' : 'Run the projection'),
      running && page.run.id ? h('button', { class: 'button button--quiet', type: 'button', 'data-action': 'lab.cancel' }, 'Cancel') : null,
    ),
  );
}

function progress(run) {
  if (!run || (run.status !== 'running' && run.status !== 'starting')) return null;
  const p = run.progress;
  const phases = { baseline: 'Running the schedule as it is', branch: 'Running it again with the change', counterexample: 'Looking for the shortest schedule that shows the difference' };
  return h(
    'div',
    { class: 'lab-progress', role: 'status' },
    h('p', null, p ? `${phases[p.phase] ?? p.phase}: ${p.done} of ${p.total}` : 'Loading a copy of the publication…'),
    h('progress', { max: p?.total || 1, value: p?.done ?? 0, 'aria-label': 'Progress of the projection' }),
  );
}

function timeline(result) {
  return h(
    'ol',
    { class: 'strip', 'aria-label': 'Every operation in schedule order, without and with the change' },
    result.timeline.map((s) => {
      const a = s.baseline === 'accepted';
      const b = s.branch === 'accepted';
      const text = `Step ${s.step + 1}: ${s.label}, by ${s.principal}. Without the change: ${s.baseline}. With it: ${s.branch}.`;
      return h(
        'li',
        { key: s.step, class: s.diverged ? 'strip__item strip__item--diverged' : 'strip__item', title: text },
        h('span', { class: a ? 'strip__mark strip__mark--yes' : 'strip__mark strip__mark--no', 'aria-hidden': 'true' }),
        h('span', { class: b ? 'strip__mark strip__mark--yes' : 'strip__mark strip__mark--no', 'aria-hidden': 'true' }),
        h('span', { class: 'visually-hidden' }, text),
      );
    }),
  );
}

function result(page) {
  const r = page.result;
  if (!r) return null;
  const c = r.comparison;
  const accepted = r.change_outcome.result === 'accepted';
  const affected = new Map();
  for (const a of r.affected_existing) affected.set(`${a.object}/${a.id}`, a);
  return h(
    'section',
    { class: 'lab-result', 'aria-labelledby': 'lab-result' },
    h('h2', { class: 'lab-result__title', id: 'lab-result', tabindex: '-1' }, 'What would change'),
    h(
      'p',
      { class: accepted ? 'lab-verdict' : 'lab-verdict lab-verdict--refused' },
      accepted
        ? c.identical
          ? 'The kernel accepts the change, and every replayed operation behaves exactly as before.'
          : `The kernel accepts the change. ${c.outcome_changes.length} of ${r.request.steps} replayed operations then end differently.`
        : `The kernel refuses the change: ${r.change_outcome.message}.`,
    ),
    !accepted ? checks(r.change_outcome.failed?.slice(0, 8)) : null,

    h('h3', null, 'Findings, by the kind of evidence behind them'),
    h(
      'ul',
      { class: 'findings' },
      r.findings.map((f, i) => h('li', { key: i, class: 'finding' }, badge(f.class), h('div', null, h('p', { class: 'finding__title' }, f.title), h('p', { class: 'finding__detail' }, f.detail)))),
    ),

    affected.size
      ? [
          h('h3', null, 'Existing records that do not meet the proposed rule'),
          table(
            'Existing records that do not meet the proposed rule',
            ['Record', 'Title', 'Rule it fails', 'Detail'],
            [...affected.values()].map((a) =>
              h('tr', { key: `${a.object}/${a.id}` }, h('th', { scope: 'row' }, `${a.object} ${a.id}`), h('td', null, a.title), h('td', null, h('code', null, a.rule)), h('td', null, a.detail)),
            ),
          ),
        ]
      : null,

    h('h3', null, 'The schedule, operation by operation'),
    h('p', { class: 'lab-note' }, 'Each column is one operation. Top mark: without the change. Bottom mark: with it. Filled means accepted, hollow means rejected; outlined columns ended differently.'),
    timeline(r),

    c.outcome_changes.length
      ? [
          h('h3', null, 'Operations that end differently'),
          table(
            'Operations that end differently',
            ['Step', 'Operation', 'By', 'Without the change', 'With the change', 'Why'],
            c.outcome_changes.slice(0, 40).map((o) =>
              h(
                'tr',
                { key: o.step, class: o.step === c.first_divergence ? 'grid__first' : null },
                h('th', { scope: 'row' }, String(o.step + 1), o.step === c.first_divergence ? h('span', { class: 'grid__flag' }, 'first divergence') : null),
                h('td', null, o.label),
                h('td', null, o.principal),
                h('td', null, verdict(o.baseline)),
                h('td', null, verdict(o.branch)),
                h('td', null, checks(o.because) ?? 'A record it refers to differs because of an earlier step.'),
              ),
            ),
          ),
          c.outcome_changes.length > 40 ? h('p', { class: 'lab-note' }, `Showing the first 40 of ${c.outcome_changes.length}.`) : null,
        ]
      : null,

    r.counterexample
      ? [
          h('h3', null, 'Smallest schedule found that still shows the first divergence'),
          h(
            'div',
            { class: 'counterexample' },
            h('ol', null, r.counterexample.labels.map((label, i) => h('li', { key: i }, `${label} (step ${r.counterexample.steps[i] + 1})`))),
            h('p', null, 'The last of these is ', verdict(r.counterexample.baseline), ' without the change and ', verdict(r.counterexample.branch), ' with it.'),
            checks(r.counterexample.because),
            h('p', { class: 'lab-note' }, `Found by removing operations one at a time over ${r.counterexample.runs_tried} runs. It is a real counterexample; a shorter one may exist.`),
          ),
        ]
      : null,

    Object.keys(c.new_failures).length
      ? [
          h('h3', null, 'Rules that fail more often with the change'),
          h('ul', { class: 'checks' }, Object.entries(c.new_failures).map(([rule, n]) => h('li', { key: rule }, h('code', null, rule), ` ${n} more time${n === 1 ? '' : 's'}`))),
        ]
      : null,

    c.record_changes.length
      ? [
          h('h3', null, 'Records that differ at the end'),
          table(
            'Records that differ at the end',
            ['Record', 'Title', 'Difference'],
            c.record_changes.slice(0, 30).map((rc) =>
              h(
                'tr',
                { key: `${rc.object}/${rc.id}` },
                h('th', { scope: 'row' }, `${rc.object} ${rc.id}`),
                h('td', null, rc.title),
                h(
                  'td',
                  null,
                  rc.kind === 'modified'
                    ? h('ul', { class: 'checks' }, rc.fields.map((fc) => h('li', { key: fc.field }, h('code', null, fc.field), `: ${value(fc.baseline)} without, ${value(fc.branch)} with`)))
                    : rc.kind === 'only_in_baseline'
                      ? 'Exists only without the change'
                      : 'Exists only with the change',
                ),
              ),
            ),
          ),
          c.record_changes.length > 30 ? h('p', { class: 'lab-note' }, `Showing the first 30 of ${c.record_changes.length}.`) : null,
        ]
      : null,

    h('h3', null, 'What readers and members would see'),
    c.query_changes.length
      ? table(
          'Query results that differ',
          ['Query', 'Seen by', 'Missing with the change', 'New with the change'],
          c.query_changes.slice(0, 30).map((q, i) =>
            h(
              'tr',
              { key: i },
              h('th', { scope: 'row' }, h('code', null, q.query)),
              h('td', null, q.observer),
              h('td', null, q.missing_in_branch.length ? q.missing_in_branch.join(', ') : q.reordered ? 'same records, different order' : 'none'),
              h('td', null, q.new_in_branch.length ? q.new_in_branch.join(', ') : 'none'),
            ),
          ),
        )
      : h('p', { class: 'lab-note' }, `All ${c.queries_compared} combinations of query and observer return the same records in the same order.`),
    h('p', { class: 'lab-note' }, `Events: ${c.events_only_in_baseline.length} emitted only without the change, ${c.events_only_in_branch.length} only with it.`),

    h('h3', null, 'Invariants'),
    table(
      'Invariants without and with the change',
      ['Invariant', 'Without the change', 'With the change'],
      c.invariants.map((inv) =>
        h(
          'tr',
          { key: inv.id },
          h('th', { scope: 'row' }, inv.statement),
          h('td', null, inv.baseline_holds ? 'Holds' : h('strong', null, 'Fails')),
          h(
            'td',
            null,
            inv.branch_holds ? 'Holds' : h('strong', null, `Fails (${inv.branch_violation_count})`),
            inv.branch_holds ? null : h('ul', { class: 'checks' }, inv.branch_violations.slice(0, 5).map((v, i) => h('li', { key: i }, v))),
          ),
        ),
      ),
    ),
    h(
      'p',
      { class: 'lab-note' },
      c.other_tenants_untouched
        ? 'Tenant isolation: every other tenant in the simulated world is identical in both executions.'
        : h('strong', null, 'Tenant isolation violated: another tenant differs between the two executions.'),
    ),

    h('h3', null, 'Not modelled'),
    h('ul', { class: 'unmodelled' }, r.unmodelled.map((line, i) => h('li', { key: i }, line))),

    h('h3', null, 'How to reproduce this'),
    h(
      'dl',
      { class: 'facts' },
      h('div', null, h('dt', null, 'Kernel'), h('dd', null, r.kernel_version)),
      h('div', null, h('dt', null, 'Snapshot'), h('dd', null, `revision ${r.snapshot.revision}, schema version ${r.snapshot.schema_version}, `, short(r.snapshot.state_hash))),
      h('div', null, h('dt', null, 'Schedule'), h('dd', null, `${r.request.source}, ${r.request.steps} operations, seed ${r.request.seed}, change applied before step ${r.request.apply_at + 1}`)),
      h('div', null, h('dt', null, 'Baseline trace'), h('dd', null, short(r.baseline.trace_hash), ` ${r.baseline.accepted} accepted, ${r.baseline.rejected} rejected`)),
      h('div', null, h('dt', null, 'Branch trace'), h('dd', null, short(r.branch.trace_hash), ` ${r.branch.accepted} accepted, ${r.branch.rejected} rejected`)),
      h('div', null, h('dt', null, 'Trace replay'), h('dd', null, `${r.trace_replay.agreed} of ${r.trace_replay.steps} steps agree when re-run against the bare kernel`)),
      r.request.faults.delivery_drop
        ? h('div', null, h('dt', null, 'Faults'), h('dd', null, `${r.baseline.dropped} of ${r.baseline.deliveries} event deliveries dropped and retried; every event still arrived once, in order`))
        : null,
    ),
  );
}

function projection(page) {
  const t = page.overview.tenant;
  return [
    h(
      'p',
      { class: 'lab-snapshot', role: 'region', 'aria-label': 'Snapshot' },
      `${t.name} at revision ${t.revision}: schema version ${t.schema_version}, ${t.records} records, ${t.journal_entries} journalled operations. State `,
      short(t.state_hash),
      '. A projection works on a copy; the live publication is never changed from this page.',
    ),
    form(page),
    progress(page.run),
    page.run?.status === 'cancelled' ? h('p', { class: 'lab-note', role: 'status' }, 'The last projection was cancelled before it finished.') : null,
    result(page),
  ];
}

function metadata(page) {
  const o = page.overview;
  return [
    h('p', { class: 'lab-snapshot', role: 'region', 'aria-label': 'Snapshot' }, `Everything below is data stored for ${o.tenant.name} at schema version ${o.tenant.schema_version}. None of it is written into the platform’s code.`),
    o.objects.map((object) =>
      h(
        'section',
        { key: object.id, class: 'lab-block', 'aria-labelledby': `object-${object.id}` },
        h('h2', { id: `object-${object.id}` }, object.label, h('span', { class: 'lab-block__count' }, `${object.records} record${object.records === 1 ? '' : 's'}`)),
        table(
          `Fields of ${object.label}`,
          ['Field', 'Type', 'Required', 'Unique', 'Default'],
          object.fields.map((f) =>
            h('tr', { key: f.id }, h('th', { scope: 'row' }, h('code', null, `${object.id}.${f.id}`)), h('td', null, f.type), h('td', null, f.required ? 'yes' : 'no'), h('td', null, f.unique ? 'yes' : 'no'), h('td', null, f.default ? h('code', null, f.default) : '')),
          ),
        ),
        object.constraints.length
          ? h('ul', { class: 'checks' }, object.constraints.map((c) => h('li', { key: c.id }, h('code', null, `${c.id}: ${c.rule}`), ` ${c.message}`)))
          : null,
      ),
    ),
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'meta-workflows' },
      h('h2', { id: 'meta-workflows' }, 'Workflows'),
      o.workflows.map((w) =>
        h(
          'div',
          { key: w.id },
          h('p', null, h('code', null, `${w.object}.${w.field}`), ` starts in ${w.initial.join(' or ')}.`),
          h('ul', { class: 'checks' }, w.transitions.map((t) => h('li', { key: t.name }, h('code', null, t.name), ` ${t.from} to ${t.to}`, t.guard ? ` when ${t.guard}` : '', t.sets.length ? `, setting ${t.sets.join('; ')}` : ''))),
        ),
      ),
    ),
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'meta-policies' },
      h('h2', { id: 'meta-policies' }, 'Roles and policies'),
      table(
        'Policies',
        ['Policy', 'Role', 'On', 'May', 'When'],
        o.policies.map((p) => h('tr', { key: p.id }, h('th', { scope: 'row' }, h('code', null, p.id)), h('td', null, p.role), h('td', null, p.resource), h('td', null, p.actions.join(', ')), h('td', null, p.when ? h('code', null, p.when) : 'always'))),
      ),
      h('ul', { class: 'checks' }, o.members.map((m) => h('li', { key: m.principal }, h('code', null, m.principal), ` holds ${m.roles.join(', ')}`))),
    ),
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'meta-queries' },
      h('h2', { id: 'meta-queries' }, 'Queries and views'),
      table(
        'Named queries',
        ['Query', 'Object', 'Where', 'Parameters'],
        o.queries.map((q) => h('tr', { key: q.id }, h('th', { scope: 'row' }, h('code', null, q.id)), h('td', null, q.object), h('td', null, q.where ? h('code', null, q.where) : 'everything'), h('td', null, q.params.join(', ')))),
      ),
      h('ul', { class: 'checks' }, o.views.map((v) => h('li', { key: v.id }, h('code', null, v.route), ` is the view ${v.id}`))),
    ),
  ];
}

function verificationReport(v) {
  if (!v) return h('p', { class: 'lab-note' }, 'No verification run yet in this session.');
  if (v.pending) return h('p', { class: 'lab-note', role: 'status' }, 'Building the Lean project, auditing its axioms and running the correspondence scenarios…');
  if (v.failed) return h('p', { class: 'form__error', role: 'alert' }, v.failed);
  const corr = v.correspondence;
  return [
    h(
      'p',
      { class: v.all_checked ? 'lab-verdict' : 'lab-verdict lab-verdict--refused', role: 'status' },
      v.all_checked
        ? `All ${v.theorems.length} theorems are machine-checked by ${v.lean_version.replace(/,.*$/, '')}, with no placeholder and no axiom beyond Lean’s standard three.`
        : `Verification did not complete: ${v.problem ?? 'see the details below'}.`,
    ),
    table(
      'Properties and the theorems behind them',
      ['Property', 'What the theorem says', 'Lean theorems', 'What it does not cover'],
      v.obligations.map((o) =>
        h(
          'tr',
          { key: o.id },
          h('th', { scope: 'row' }, o.property, ' ', badge(o.class)),
          h('td', null, o.statement),
          h('td', null, h('ul', { class: 'checks' }, o.theorems.map((t) => {
            const checked = v.theorems.find((c) => c.theorem === t);
            return h('li', { key: t }, h('code', null, t.replace('Strata.', '')), checked ? (checked.clean ? ' checked' : ' NOT CLEAN') : ' not found');
          }))),
          h('td', null, o.limits),
        ),
      ),
    ),
    corr
      ? h(
          'p',
          { class: 'lab-note' },
          badge('reproduced'),
          corr.agree
            ? ` The Lean model and the Rust kernel print identical results for ${corr.operations} operations across ${corr.scenarios} generated scenarios.`
            : ` The Lean model and the Rust kernel differ at line ${corr.first_difference?.[0]}.`,
        )
      : null,
    h('h3', null, 'Assumptions'),
    h('ul', { class: 'unmodelled' }, v.assumptions.map((a, i) => h('li', { key: i }, a))),
    h('details', null, h('summary', null, 'Axioms each theorem depends on'), h('ul', { class: 'checks' }, v.theorems.map((t) => h('li', { key: t.theorem }, h('code', null, t.theorem), `: ${t.axioms.length ? t.axioms.join(', ') : 'none'}`)))),
  ];
}

function replayReport(r) {
  if (!r) return h('p', { class: 'lab-note' }, 'Not replayed yet in this session.');
  if (r.pending) return h('p', { class: 'lab-note', role: 'status' }, 'Replaying…');
  if (r.failed) return h('p', { class: 'form__error', role: 'alert' }, r.failed);
  const rep = r.report;
  return [
    h(
      'p',
      { class: rep.matches_live ? 'lab-verdict' : 'lab-verdict lab-verdict--refused', role: 'status' },
      rep.matches_live
        ? `${rep.agreed} of ${rep.entries} committed operations replay to exactly the recorded result, and the replayed state is the live state.`
        : `The replay does not match the live state${rep.first_mismatch ? `: revision ${rep.first_mismatch[0]}, ${rep.first_mismatch[1]}` : ''}.`,
    ),
    h(
      'dl',
      { class: 'facts' },
      h('div', null, h('dt', null, 'Evidence'), h('dd', null, badge(r.class))),
      h('div', null, h('dt', null, 'Replayed state'), h('dd', null, short(rep.replayed_hash))),
      h('div', null, h('dt', null, 'Live state'), h('dd', null, short(rep.live_hash), ` at revision ${r.live_revision}`)),
    ),
    h('p', { class: 'lab-note' }, r.establishes),
    h('h3', null, 'What a replay does not show'),
    h('ul', { class: 'unmodelled' }, r.does_not_establish.map((line, i) => h('li', { key: i }, line))),
  ];
}

function evidence(page) {
  return [
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'ev-classes' },
      h('h2', { id: 'ev-classes' }, 'The five kinds of evidence'),
      h('ul', { class: 'findings' }, page.overview.classes.map((c) => h('li', { key: c.id, class: 'finding' }, badge(c.id), h('div', null, h('p', { class: 'finding__title' }, c.label), h('p', { class: 'finding__detail' }, c.meaning))))),
    ),
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'ev-proofs' },
      h('h2', { id: 'ev-proofs' }, 'Machine-checked proofs'),
      h('p', { class: 'lab-note' }, 'The proofs are about a small abstract model written in Lean 4, not about the Rust code. Checking them needs the Lean toolchain on this machine; serving the publication does not.'),
      h('p', null, h('button', { class: 'button', type: 'button', disabled: page.verification?.pending === true, 'data-action': 'lab.verify' }, 'Check the proofs now')),
      verificationReport(page.verification),
    ),
    h(
      'section',
      { class: 'lab-block', 'aria-labelledby': 'ev-replay' },
      h('h2', { id: 'ev-replay' }, 'Replay of what really happened'),
      h('p', { class: 'lab-note' }, 'Runs every committed operation of this publication through the kernel again, from nothing, and compares the result with the live data.'),
      h('p', null, h('button', { class: 'button', type: 'button', disabled: page.replay?.pending === true, 'data-action': 'lab.replay' }, 'Replay the journal')),
      replayReport(page.replay),
    ),
  ];
}

function diagnostics(page) {
  const d = page.diagnostics;
  if (!d) return [h('p', { class: 'loading', role: 'status' }, 'Loading diagnostics…')];
  if (d.failed) return [h('p', { class: 'form__error', role: 'alert' }, d.failed)];
  const b = d.backend;
  const count = (obj) => Object.entries(obj ?? {});
  return [
    h('p', null, h('button', { class: 'button button--quiet', type: 'button', 'data-action': 'lab.diagnostics' }, 'Refresh')),
    h(
      'dl',
      { class: 'facts' },
      h('div', null, h('dt', null, 'Kernel'), h('dd', null, b.kernel_version)),
      h('div', null, h('dt', null, 'Store'), h('dd', null, `${b.store.capabilities.adapter}, ${b.store.capabilities.durable ? 'durable' : 'not durable'}, ${b.store.health.detail}`)),
      h('div', null, h('dt', null, 'Isolation'), h('dd', null, b.store.capabilities.isolation)),
      h('div', null, h('dt', null, 'Outbox'), h('dd', null, `${b.store.health.outbox_pending} event(s) waiting to be published`)),
      h('div', null, h('dt', null, 'In flight'), h('dd', null, `${b.pending_operations} backend operation(s), ${d.bff.pending_requests} BFF request(s)`)),
      h('div', null, h('dt', null, 'Sockets'), h('dd', null, `${d.bff.open_sockets} browser connection(s) through the BFF`)),
      h('div', null, h('dt', null, 'Commit races retried'), h('dd', null, String(b.conflicts_retried))),
    ),
    h('h2', null, 'Tenants'),
    table('Tenants', ['Tenant', 'Revision', 'Schema version', 'Events', 'Records'], b.tenants.map((t) => h('tr', { key: t.id }, h('th', { scope: 'row' }, t.name), h('td', null, String(t.revision)), h('td', null, String(t.schema_version)), h('td', null, String(t.event_seq)), h('td', null, String(t.records))))),
    h('h2', null, 'Sessions'),
    table('Active sessions', ['Session', 'Tenant', 'Principal', 'Last seen', 'Ends'], b.sessions.map((s) => h('tr', { key: s.id }, h('th', { scope: 'row' }, h('code', null, s.id)), h('td', null, s.tenant), h('td', null, s.principal), h('td', null, date(s.last_seen_at, 'stamp')), h('td', null, date(s.expires_at, 'stamp'))))),
    h('h2', null, 'WebSocket connections'),
    table('WebSocket connections', ['Connection', 'Tenant', 'Principal', 'Subscriptions', 'Delivered', 'Acknowledged'], count(b.connections).map(([id, c]) => h('tr', { key: id }, h('th', { scope: 'row' }, h('code', null, id)), h('td', null, c.tenant), h('td', null, c.principal), h('td', null, c.subscriptions.join(', ')), h('td', null, String(c.delivered)), h('td', null, String(c.acknowledged))))),
    h('h2', null, 'Kernel transitions'),
    table('Kernel transitions by intent', ['Intent', 'Accepted', 'Rejected'], count(b.transitions).map(([name, [yes, no]]) => h('tr', { key: name }, h('th', { scope: 'row' }, h('code', null, name)), h('td', null, String(yes)), h('td', null, String(no))))),
    h('h2', null, 'Recent rejections'),
    b.recent_rejections.length
      ? table('Recent rejections', ['When', 'Intent', 'Kind', 'Failed checks', 'Correlation'], [...b.recent_rejections].reverse().slice(0, 15).map((r, i) => h('tr', { key: i }, h('th', { scope: 'row' }, date(r.at, 'stamp')), h('td', null, r.intent), h('td', null, r.kind), h('td', null, r.failed.join(', ')), h('td', null, h('code', null, r.correlation)))))
      : h('p', { class: 'lab-note' }, 'Nothing has been rejected since the backend started.'),
    h('h2', null, 'Recent events'),
    table('Recent domain events', ['Sequence', 'Tenant', 'Kind', 'Record', 'Correlation'], [...b.recent_events].reverse().slice(0, 15).map((e) => h('tr', { key: `${e.tenant}-${e.seq}` }, h('th', { scope: 'row' }, String(e.seq)), h('td', null, e.tenant), h('td', null, e.kind), h('td', null, e.object ? `${e.object} ${e.id}` : ''), h('td', null, h('code', null, e.correlation))))),
    h('h2', null, 'Recent simulation runs'),
    b.recent_runs.length
      ? table('Recent simulation runs', ['Run', 'Status', 'Change', 'Steps'], b.recent_runs.map((r) => h('tr', { key: r.id }, h('th', { scope: 'row' }, h('code', null, r.id)), h('td', null, r.status), h('td', null, h('code', null, r.summary.change)), h('td', null, String(r.summary.steps)))))
      : h('p', { class: 'lab-note' }, 'No simulation has been run since the backend started.'),
    h('details', null, h('summary', null, 'The whole report as JSON'), h('pre', { class: 'lab-json', tabindex: '0' }, JSON.stringify(d, null, 2))),
  ];
}

export function lab(page) {
  const head = [
    h('h1', { class: 'page-title' }, 'Impact Lab'),
    h('p', { class: 'standfirst' }, 'Try a change to the publication’s rules against a copy of its data, and see exactly what it would do, before making it.'),
    h(
      'div',
      { class: 'segmented lab-tabs', role: 'group', 'aria-label': 'Sections of the Impact Lab' },
      TABS.map(([id, label]) => h('button', { key: id, type: 'button', class: 'segmented__item', 'aria-pressed': page.tab === id ? 'true' : 'false', 'data-action': 'lab.tab', 'data-arg': arg(id) }, label)),
    ),
  ];
  if (page.error) return [...head, h('p', { class: 'form__error', role: 'alert' }, page.error)];
  if (page.loading || !page.overview) return [...head, h('p', { class: 'loading', role: 'status' }, 'Loading the publication’s metadata…')];
  const body = page.tab === 'metadata' ? metadata(page) : page.tab === 'evidence' ? evidence(page) : page.tab === 'diagnostics' ? diagnostics(page) : projection(page);
  return [...head, h('div', { class: `lab lab--${page.tab}`, key: page.tab }, body)];
}
