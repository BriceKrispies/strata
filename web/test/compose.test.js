// The interpreter of tenant screens (docs/design-system.md, "The interpreter"): the page model a
// screen's part, its section shapes and a view reply make, pattern by pattern and state by state.
// Plain functions: no browser, no worker.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SKELETON_ROWS, compose, humanise, nodeText, nodesOf, sectionOf } from '../src/kernel/compose.js';

const NOW = Date.UTC(2026, 10, 24, 12, 0, 0);
const HOUR = 3_600_000;
const EPOCH = NOW - 3 * HOUR;

const shape = (object, fields) => ({ object, fields, fingerprint: 'f'.repeat(64) });
const f = (id, type, required = true, extra = {}) => ({ id, type, required, ...extra });

/** A screen part shaped like the showcase's message centre, hub and directory. */
const part = {
  kind: 'screen',
  id: 'showcase_community__hub',
  screen: { route: '/hub', view: 'showcase_community__hub_view', title: 'Home', kind: 'list', auth: 'session', query: [] },
  sections: {
    messages: {
      single: false,
      shape: shape('showcase_community__message', [
        f('author', 'str'), f('body', 'str'), f('channel', 'str'),
        f('number', 'ref<showcase_community__message>', false), f('posted_at', 'time'), f('status', 'str'), f('title', 'str'),
      ]),
    },
    my_badges: {
      single: false,
      shape: shape('showcase_community__badge_award', [
        f('awarded_at', 'time'), f('badge_key', 'str'), f('badge_name', 'str'), f('holder', 'str'), f('reason', 'str'), f('source', 'str'),
      ]),
    },
    notifications: {
      single: false,
      shape: shape('showcase_community__notification', [
        f('at', 'time'), f('kind', 'str'), f('link', 'str', false), f('number', 'ref<showcase_community__notification>', false),
        f('recipient', 'str'), f('state', 'str'), f('text', 'str'),
      ]),
    },
    people: {
      single: false,
      shape: shape('showcase_identity__person', [
        f('access', 'str'), f('department', 'ref<showcase_identity__department>'), f('display_name', 'str'), f('email', 'str'),
        f('handle', 'str'), f('joined_at', 'time'), f('office', 'str', false), f('principal', 'str'), f('status', 'str'), f('title', 'str'),
      ]),
    },
    profile: {
      single: true,
      shape: shape('showcase_community__profile', [
        f('bio', 'str', false), f('display_name', 'str'), f('handle', 'str'), f('joined_at', 'time'), f('languages', 'list<str>', false),
        f('open_to_mentoring', 'bool', false), f('principal', 'str'), f('title', 'str', false),
      ]),
    },
    reactions_received: { single: false, shape: { object: 'showcase_community__reaction', aggregate: 'received_by_person', fingerprint: 'a'.repeat(64) } },
    total: { single: false, shape: { object: 'showcase_community__reaction', aggregate: 'count', fingerprint: 'b'.repeat(64) } },
  },
  forms: {
    showcase_community__form_post: {
      screen: 'showcase_community__hub',
      operation: 'showcase_community__post_message',
      fields: [
        { name: 'channel', input: 'channel', label: 'Channel', control: 'select', options: [{ value: 'general', label: 'General' }] },
        { name: 'title', input: 'title', label: 'Title', control: 'text', trim: true },
        { name: 'body', input: 'body', label: 'Message', control: 'textarea' },
      ],
      submit_label: 'Post',
      shape: { operation: 'showcase_community__post_message', inputs: [{ name: 'channel', type: 'str', required: true }, { name: 'title', type: 'str', required: true }, { name: 'body', type: 'str', required: false }] },
    },
  },
  actions: {},
  live: {},
};

const row = (id, fields, extra = {}) => ({ id, object: 'x', version: 1, created_at: EPOCH, updated_at: EPOCH, created_by: null, fields, refs: {}, ...extra });

const model = {
  view: 'showcase_community__hub_view',
  title: 'Home',
  people: { p1: { name: 'Ada Okafor', avatar: null, self: true } },
  data: {
    messages: [
      row(12, {
        author: 'user:ada', body: 'Thanks to everyone who came to the offsite. The notes are in the shared folder now.', channel: 'general',
        number: { ref: { object: 'showcase_community__message', id: 12, title: 'Offsite notes' } }, posted_at: EPOCH, status: 'published', title: 'Offsite notes',
      }, { created_by: { person: 'p1' } }),
    ],
    my_badges: [
      row(4, { awarded_at: EPOCH - 30 * 24 * HOUR, badge_key: 'polyglot', badge_name: 'Polyglot', holder: 'user:ada', reason: 'Speaks as many languages as their programme asks.', source: 'profile rule' }),
    ],
    notifications: [
      row(7, { at: EPOCH, kind: 'reaction', link: '/messages/12', number: { ref: { object: 'showcase_community__notification', id: 7, title: null } }, recipient: 'user:ada', state: 'unread', text: 'Priya celebrated your message.' }),
      row(6, { at: EPOCH - HOUR, kind: 'badge', link: null, number: null, recipient: 'user:ada', state: 'read', text: 'You earned Polyglot.' }),
    ],
    people: [
      row(1, { access: 'tenant admin', department: { ref: { object: 'showcase_identity__department', id: 2, title: 'Engineering' } }, display_name: 'Ada Okafor', email: 'ada@northwind.test', handle: 'ada', joined_at: EPOCH - 400 * 24 * HOUR, office: 'Lisbon', principal: 'user:ada', status: 'active', title: 'Head of platform' }),
      row(2, { access: 'member', department: { ref: null }, display_name: 'Priya Raman', email: 'priya@northwind.test', handle: 'priya-raman', joined_at: EPOCH - 90 * 24 * HOUR, office: null, principal: 'user:priya', status: 'on_leave', title: 'Designer' }),
    ],
    profile: row(9, { bio: 'I look after the platform team and our tools.', display_name: 'Ada Okafor', handle: 'ada', joined_at: EPOCH - 400 * 24 * HOUR, languages: ['English', 'Portuguese'], open_to_mentoring: true, principal: 'user:ada', title: 'Head of platform' }),
    reactions_received: { query: 'q', aggregate: 'received_by_person', scanned: 3, groups: [{ key: 'user:kwame', value: 14, rows: [] }, { key: 'user:ada', value: 7, rows: [] }, { key: null, value: 1, rows: [] }], depends_on: [], revision: 1, schema_version: 1 },
    total: { query: 'q', aggregate: 'count', scanned: 3, groups: [{ key: null, value: 122, rows: [] }], depends_on: [], revision: 1, schema_version: 1 },
  },
  next: {},
};

const ctx = { now: NOW, screens: [{ id: 'showcase_community__message_page', route: '/messages/:id' }, { id: 'showcase_identity__person_page', route: '/directory/people/:handle' }] };

const byId = (page, id) => nodesOf(page).find((n) => n.id === id);

/** Every string a page model would draw, with the keys that are machine names left out. */
function drawnStrings(node, out = []) {
  const MACHINE = new Set(['pattern', 'id', 'key', 'section', 'form', 'kind', 'iso', 'href', 'column', 'control', 'state', 'template', 'tone', 'layout', 'src', 'tag', 'type']);
  if (Array.isArray(node)) node.forEach((n) => drawnStrings(n, out));
  else if (node && typeof node === 'object') {
    for (const [k, v] of Object.entries(node)) {
      if (typeof v === 'string') {
        if (!MACHINE.has(k)) out.push(v);
      } else drawnStrings(v, out);
    }
  }
  return out;
}

test('a list with a title and a body is a list of cards, every value formatted by its display type', () => {
  const page = compose(part, model, ctx);
  const messages = byId(page, 'section:messages');
  assert.equal(messages.pattern, 'record-list');
  assert.equal(messages.state, 'ready');
  assert.equal(messages.label, 'Messages');
  const card = messages.items[0];
  assert.equal(card.title, 'Offsite notes');
  assert.equal(card.href, '/messages/12', 'a route named after the object type opens the record');
  assert.equal(card.state, null, 'a published message says nothing of its state (D-DS-31)');
  assert.equal(card.edge, null);
  assert.equal(card.person.kind, 'person');
  assert.equal(card.person.name, 'Ada Okafor', 'the author comes from the people table');
  assert.equal(card.time.kind, 'time');
  assert.equal(card.time.text, '3 hours ago');
  assert.equal(card.body.text.startsWith('Thanks to everyone'), true);
  assert.equal(card.number, 'No. 12', 'the self-reference the forms ask for');
  assert.deepEqual(card.meta.map((m) => [m.label, nodeText(m.value)]), [['Channel', 'General']], 'the principal in author is not drawn');
});

test('a state that departs from normal is a state line with its meaning and an edge; a normal one is nothing', () => {
  const declared = {
    ...part,
    sections: {
      messages: {
        single: false,
        shape: shape('showcase_community__message', [
          f('status', 'str', true, { displayType: 'status', props: { 'state.published': 'normal', 'state.hidden': 'attention', 'means.hidden': 'Members cannot see this message.' } }),
          f('title', 'str'),
        ]),
      },
    },
  };
  const data = { messages: [row(1, { status: 'published', title: 'Seen' }), row(2, { status: 'hidden', title: 'Unseen' }), row(3, { status: 'draft', title: 'Undeclared' })] };
  const [first, second, third] = byId(compose(declared, { data }, ctx), 'section:messages').items;
  assert.equal(first.state, null);
  assert.equal(first.edge, null);
  assert.deepEqual(second.state, { kind: 'state', value: 'hidden', label: 'Hidden', mark: 'attention', means: 'Members cannot see this message.' });
  assert.equal(second.edge, 'attention');
  assert.deepEqual(third.state, { kind: 'state', value: 'draft', label: 'Draft', mark: 'note', means: null }, 'a fallback exception has the lead word only');
  assert.equal(third.edge, 'note');
});

test('a list of many short fields and no body is a data table; identifiers are not columns', () => {
  // On /directory, a record of the list opens on the screen whose route continues it.
  const page = compose(part, model, { ...ctx, route: '/directory' });
  const people = byId(page, 'section:people');
  assert.equal(people.pattern, 'data-table');
  const labels = people.columns.map((c) => c.label);
  assert.deepEqual(labels, ['Name', 'Title', 'Status', 'Access', 'Department', 'Email', 'Office', 'Joined']);
  const first = people.rows[0].cells;
  assert.equal(first[0].value.kind, 'title');
  assert.equal(first[0].value.text, 'Ada Okafor');
  assert.equal(first[0].value.href, '/directory/people/ada');
  assert.deepEqual(first[0].value.avatar, { initials: 'AO', src: null });
  const second = Object.fromEntries(people.rows[1].cells.map((c) => [c.label, nodeText(c.value)]));
  assert.equal(second.Department, 'Not available', 'an unreadable reference is not a link and not an id');
  assert.equal(second.Status, 'On leave');
  assert.equal(first.find((c) => c.label === 'Status').value.text, '', 'a normal state says nothing in its column');
  assert.deepEqual(people.rows.map((r) => r.edge), [null, 'note'], 'the row that departs from normal has the edge rule');
  assert.equal(second.Office, '');
});

test('a single section is a record detail with its title, avatar and facts', () => {
  const page = compose(part, model, ctx);
  const profile = byId(page, 'section:profile');
  assert.equal(profile.pattern, 'record-detail');
  assert.equal(page.main[0], profile, 'a record shown in detail leads the page');
  assert.equal(profile.record.title, 'Ada Okafor');
  assert.equal(profile.record.subtitle, 'Head of platform');
  assert.deepEqual(profile.record.avatar, { initials: 'AO', src: null });
  assert.equal(profile.record.body.text, 'I look after the platform team and our tools.');
  const facts = Object.fromEntries(profile.facts.map((x) => [x.label, x.value]));
  assert.equal(facts.Languages.kind, 'list');
  assert.equal(nodeText(facts.Languages), 'English and Portuguese', 'a short list is a sentence, not chips');
  assert.equal(nodeText(facts['Open to mentoring']), 'Yes');
  assert.equal(facts.Joined.kind, 'time');
  assert.ok(!('Principal' in facts) && !('Handle' in facts));
});

test('a single section with no record is empty, not blank', () => {
  const page = compose(part, { ...model, data: { ...model.data, profile: null } }, ctx);
  const profile = byId(page, 'section:profile');
  assert.equal(profile.state, 'empty');
  assert.ok(profile.empty.title);
});

test('an aggregate grouped by a field is bars, one with a single value a stat tile, never JSON', () => {
  const page = compose(part, model, ctx);
  const bars = byId(page, 'section:reactions_received');
  assert.equal(bars.pattern, 'aggregate-bars');
  assert.deepEqual(bars.bars.map((b) => [b.label, b.text, b.fill]), [['A member', '14', 20], ['A member', '7', 10], ['Not set', '1', 1]]);
  assert.equal(bars.total, '22 in all');
  assert.ok(page.aside.includes(bars), 'an aggregate sits in the side column');
  const tile = byId(page, 'section:total');
  assert.equal(tile.pattern, 'stat-tile');
  assert.equal(tile.value, '122');
  for (const text of drawnStrings([bars, tile])) assert.doesNotMatch(text, /[{}[\]"]/, `JSON in ${text}`);
});

test('a list with an unread and read state is a notification list with its unread count', () => {
  const page = compose(part, model, ctx);
  const notices = byId(page, 'section:notifications');
  assert.equal(notices.pattern, 'notification-list');
  assert.equal(notices.unread, 1);
  assert.equal(notices.count, '1 unread', 'the heading says the count in words');
  assert.deepEqual(notices.items.map((i) => i.edge), ['unread', null]);
  assert.deepEqual(notices.items.map((i) => [i.text, i.unread, i.kind, i.href]), [
    ['Priya celebrated your message.', true, 'Reaction', '/messages/12'],
    ['You earned Polyglot.', false, 'Badge', null],
  ]);
  assert.ok(page.aside.includes(notices));
});

test('a form has labelled controls, the required flags of its operation and a primary submit', () => {
  const held = { values: { channel: 'general', title: 'Hi', body: '' }, busy: false, error: 'The message was refused.', errors: { title: 'A title is needed.' } };
  const page = compose(part, model, { ...ctx, forms: { showcase_community__form_post: held } });
  const form = byId(page, 'form:showcase_community__form_post');
  assert.equal(form.pattern, 'form');
  assert.equal(form.title, 'Post message');
  assert.equal(form.submitLabel, 'Post');
  assert.equal(form.error, 'The message was refused.');
  assert.deepEqual(form.fields.map((x) => [x.name, x.label, x.required, x.value, x.error]), [
    ['channel', 'Channel', true, 'general', ''],
    ['title', 'Title', true, 'Hi', 'A title is needed.'],
    ['body', 'Message', false, '', ''],
  ]);
  assert.equal(form.open, true);
});

test('several forms on a settings screen are disclosures; the first and any with an error are open', () => {
  const forms = Object.fromEntries(['a', 'b', 'c'].map((id) => [id, { operation: `p__do_${id}`, fields: [{ name: 'x', input: 'x', label: 'X', control: 'text' }], submit_label: 'Go' }]));
  const page = compose({ ...part, sections: {}, forms }, { data: {} }, { ...ctx, layout: 'settings', forms: { c: { values: { x: '' }, busy: false, error: 'No.', errors: {} } } });
  const nodes = nodesOf(page).filter((n) => n.pattern === 'form');
  assert.deepEqual(nodes.map((n) => [n.title, n.collapsible, n.open]), [['Do a', true, true], ['Do b', true, false], ['Do c', true, true]]);
});

test('loading draws every section as a skeleton with the same fixed row count', () => {
  const page = compose(part, null, ctx);
  assert.equal(page.state, 'loading');
  const sections = nodesOf(page).filter((n) => n.pattern !== 'form');
  assert.equal(sections.length, Object.keys(part.sections).length);
  for (const node of sections) {
    assert.equal(node.state, 'loading', node.id);
    if (node.pattern !== 'stat-tile') assert.equal(node.skeleton.rows, SKELETON_ROWS, `${node.id} says nothing about how many records there are`);
  }
  assert.ok(!('items' in byId(page, 'section:messages')));
});

test('an empty list is an empty state with the section\'s label; a missing one an error', () => {
  const data = { ...model.data, messages: [] };
  delete data.people;
  const page = compose(part, { ...model, data }, ctx);
  const messages = byId(page, 'section:messages');
  assert.equal(messages.state, 'empty');
  assert.match(messages.empty.title, /messages/);
  const people = byId(page, 'section:people');
  assert.equal(people.state, 'error');
  assert.ok(people.error);
});

test('the page is a main column and a side column, sections kept in their order', () => {
  const page = compose(part, model, ctx);
  assert.equal(page.template, 'split');
  assert.deepEqual(page.main.map((n) => n.id), ['section:profile', 'section:messages', 'section:my_badges', 'section:people']);
  assert.deepEqual(page.aside.map((n) => n.id), ['form:showcase_community__form_post', 'section:notifications', 'section:reactions_received', 'section:total']);
});

test('a section label is the humanised id', () => {
  assert.equal(humanise('my_badges'), 'My badges');
  assert.equal(humanise('reactions_received'), 'Reactions received');
  assert.equal(humanise('showcase_community__post_message'), 'Post message');
  const page = compose(part, model, ctx);
  assert.equal(byId(page, 'section:my_badges').label, 'My badges');
});

test('no text of a page model is an epoch, a principal or JSON, in any state', () => {
  const states = [compose(part, model, ctx), compose(part, null, ctx), compose(part, { ...model, data: {} }, ctx), compose(part, model, { forms: {} })];
  for (const page of states) {
    for (const text of drawnStrings(page)) {
      assert.doesNotMatch(text, /\d{10,}/, `an epoch or id in "${text}"`);
      assert.doesNotMatch(text, /\buser:/, `a principal in "${text}"`);
      assert.doesNotMatch(text, /^[{[]|"\w+":/, `JSON in "${text}"`);
    }
  }
});

test('a part without section shapes is drawn from its rows', () => {
  const bare = { screen: { kind: 'list' }, forms: {} };
  const page = compose(bare, { data: { open: [{ id: 3, fields: { title: 'Leak', status: 'open', opened_at: EPOCH } }, { id: 4, fields: { title: 'Drip', status: 'draft', opened_at: EPOCH } }] } }, ctx);
  const open = byId(page, 'section:open');
  assert.equal(open.pattern, 'record-list');
  assert.equal(open.items[0].title, 'Leak');
  assert.equal(open.items[0].state ?? null, null, 'a normal state (open) is not drawn');
  assert.equal(open.items[1].state?.label, 'Draft', 'a state that departs from normal is said in words');
  assert.equal(open.items[0].time.text, '3 hours ago', 'a number in a field named *_at is a time');
});

// Row actions (WG-5466, D-DS-32): the answer of preflight for each row decides how the action is
// drawn on that row, and a refused one says why in words (D-DS-31).
const rowActionPart = {
  ...part,
  actions: {
    showcase_community__react: { screen: 'showcase_community__hub', label: 'React', operation: 'showcase_community__react_to_message', form: null, row: 'messages', arguments: { message: 'row.id', kind: 'form.kind' }, confirm: null, on_success: {}, on_error: 'inline', when_refused: 'disable' },
    showcase_community__hide: { screen: 'showcase_community__hub', label: 'Hide', operation: 'showcase_community__hide_message', form: null, row: 'messages', arguments: { message: 'row.id' }, confirm: null, on_success: {}, on_error: 'inline', when_refused: 'hide' },
    showcase_community__pin: { screen: 'showcase_community__hub', label: 'Pin', operation: 'showcase_community__hide_message', form: null, row: 'messages', arguments: { message: 'row.id' }, confirm: null, on_success: {}, on_error: 'inline', when_refused: 'disable' },
  },
};

test('a row action is drawn on its row as the answer of preflight says: refused disabled with its wording, refused and hidden removed, unknown enabled', () => {
  const rowId = model.data.messages[0].id;
  const affordances = {
    messages: {
      [String(rowId)]: {
        showcase_community__react: { state: 'refused', code: 'own-message', detail: 'You cannot react to your own message.' },
        showcase_community__hide: { state: 'refused', code: 'not-moderator', detail: 'Only moderators hide messages.' },
        showcase_community__pin: { state: 'unknown', reason: 'withheld-read' },
      },
    },
  };
  const page = compose(rowActionPart, { ...model, affordances }, ctx);
  const card = byId(page, 'section:messages').items[0];
  assert.deepEqual(card.actions.map((a) => a.id), ['showcase_community__react', 'showcase_community__pin'], 'a refused hide is removed from the row');
  const [react, pin] = card.actions;
  assert.equal(react.label, 'React');
  assert.equal(react.disabled, true, 'a refused disable is drawn disabled');
  assert.equal(react.note, 'You cannot react to your own message.', 'the condition wording says why');
  assert.equal(pin.disabled, false, 'unknown is drawn enabled');
  assert.equal(pin.note, null);
  assert.equal(react.row, String(rowId), 'a press names the row it sits on');
  const said = card.actions.map((a) => `${a.label} ${a.note ?? ''}`).join(' ');
  assert.equal(/epoch|user:|[{}"]/.test(said), false, 'no epoch, principal or JSON text reaches the page model');
});

test('a row without an answer draws its row actions enabled, and a page with no row action draws none', () => {
  const page = compose(rowActionPart, model, ctx);
  const card = byId(page, 'section:messages').items[0];
  assert.deepEqual(card.actions.map((a) => [a.id, a.disabled]), [['showcase_community__react', false], ['showcase_community__hide', false], ['showcase_community__pin', false]], 'no answer is unknown, drawn enabled');
  const plain = compose(part, model, ctx);
  assert.equal(byId(plain, 'section:messages').items[0].actions.length, 0);
});

// A kernel-computed field (D-8307) reaches the client as a stored field: the shape lists it with its
// type and the row carries its value. A write recomputes the value on the server, and the next view
// reply carries the recomputed value, which the page draws. The composite "when equals" of the task
// (WG-5513, criterion 4) does not exist yet, so this tests the value the page draws.
test('a kernel-computed field is drawn like a stored one, and the value a write recomputed is the one drawn', () => {
  const ticketPart = {
    kind: 'screen',
    id: 'kernel_tasks__tickets',
    screen: { route: '/tickets', view: 'kernel_tasks__tickets_view', title: 'Tickets', kind: 'list', auth: 'session', query: [] },
    sections: {
      tickets: { single: false, shape: shape('kernel_tasks__ticket', [f('status', 'str'), f('task', 'ref<kernel_tasks__task>', false), f('title', 'str')]) },
    },
    forms: {},
    actions: {},
    live: {},
  };
  const viewOf = (title) => ({
    view: 'kernel_tasks__tickets_view',
    title: 'Tickets',
    people: {},
    data: {
      tickets: [row(3, { status: 'open', task: { ref: { object: 'kernel_tasks__task', id: 1, title } }, title })],
    },
    next: {},
  });

  const before = compose(ticketPart, viewOf('Draft the plan'), ctx);
  const card = byId(before, 'section:tickets').items[0];
  assert.equal(card.title, 'Draft the plan', 'the computed value is drawn as the title of the card');
  assert.equal(drawnStrings(before).includes('Draft the plan'), true);

  // The task is retitled: the kernel recomputes the ticket's title in the same commit, and the
  // client draws what the next view reply carries.
  const after = compose(ticketPart, viewOf('Final plan'), ctx);
  const recomputed = byId(after, 'section:tickets').items[0];
  assert.equal(recomputed.title, 'Final plan');
  assert.equal(drawnStrings(after).includes('Draft the plan'), false, 'the stale value is not drawn');
  assert.equal(drawnStrings(after).includes('Final plan'), true);
});

/**
 * WG-5599: a list section's shape declares its row marks and joins, and each row of the reply
 * carries their values, typed, in the page model. Nothing is drawn for them yet.
 */
test('a row carries its reader-relative mark and its joined groups in the page model, as the shape declares them', () => {
  const reactionPart = {
    ...part,
    sections: {
      ...part.sections,
      messages: {
        single: false,
        shape: {
          ...part.sections.messages.shape,
          marks: { mine: { object: 'showcase_community__reaction', aggregate: 'mine', key: 'message', second: null } },
          joins: { reactions: { object: 'showcase_community__reaction', aggregate: 'per_emoji', key: 'message', second: 'emoji' } },
        },
      },
    },
  };
  const declared = sectionOf(reactionPart, 'messages', model.data.messages);
  assert.deepEqual(declared.marks.mine, { object: 'showcase_community__reaction', aggregate: 'mine', key: 'message', second: null });
  assert.equal(declared.joins.reactions.second, 'emoji', 'a join says which field its groups are made by');

  const reacted = {
    ...model,
    data: {
      ...model.data,
      messages: [{ ...model.data.messages[0], marks: { mine: true }, joins: { reactions: [{ key: '👍', value: 3 }, { key: '🎉', value: 1 }] } }],
    },
  };
  const page = compose(reactionPart, reacted, ctx);
  const card = byId(page, 'section:messages').items[0];
  assert.deepEqual(card.marks, { mine: true }, 'the reader\'s mark is a boolean on the row');
  assert.deepEqual(card.joins, { reactions: [{ key: '👍', value: 3 }, { key: '🎉', value: 1 }] }, 'the joined groups are a typed list of key and value');

  const bare = compose(reactionPart, model, ctx);
  const plain = byId(bare, 'section:messages').items[0];
  assert.deepEqual(plain.marks, {}, 'a row the reply sends no mark for carries none');
  assert.deepEqual(plain.joins, {});
});
