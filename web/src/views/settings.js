// The Settings page: one section per product area, each setting with the value it reads, where
// that value comes from, and a control for each level this person may change (WG-5601). The model
// is built by the kernel (kernel/settings.js, settingsPage); this file only draws it from the
// design system's primitives. A value is said in words; a read-only setting says why it is read only.
// @ts-check

import { h } from '../components/dom.js';
import { button, emptyState, notice, sectionFrame } from '../components/ds.js';

/** The control of one level: a select, a number field, a switch or a text field, named `value`. */
function input(control, current, id, busy) {
  const common = { id, name: 'value', disabled: busy };
  if (control.kind === 'switch') {
    return h('input', { ...common, type: 'checkbox', checked: current === true, class: 'ds-check' });
  }
  if (control.kind === 'select') {
    return h(
      'select',
      { ...common, class: 'ds-select' },
      control.options.map((o) => h('option', { key: String(o.value), value: String(o.value), selected: String(o.value) === String(current) }, o.label)),
    );
  }
  if (control.kind === 'number') {
    return h('input', {
      ...common,
      type: 'number',
      step: 1,
      inputmode: 'numeric',
      min: control.min,
      max: control.max,
      value: current === null || current === undefined ? '' : String(current),
      class: 'ds-input',
    });
  }
  return h('input', { ...common, type: 'text', value: current === null || current === undefined ? '' : String(current), class: 'ds-input' });
}

/** The control for one level of a setting, as a form of its own: the level names the save. */
function levelForm(row, level) {
  const id = `setting-${row.id}-${level.level}`;
  return h(
    'form',
    { class: 'ds-form settings-level', 'data-form': `settings:${level.level}:${row.id}`, novalidate: true, key: level.level },
    h(
      'fieldset',
      { class: 'ds-fieldset' },
      h('legend', { class: 'ds-field__label' }, level.label),
      h(
        'div',
        { class: 'ds-field' },
        h('label', { class: 'ds-field__label', for: id }, row.label),
        input(level.control, level.current, id, level.busy),
      ),
      h(
        'div',
        { class: 'ds-form__actions' },
        button({ variant: 'primary', type: 'submit', disabled: level.busy }, level.busy ? 'Saving…' : 'Save'),
      ),
    ),
  );
}

/** One setting: its value and where it comes from, then its controls or the reason it is read only. */
function settingCard(row) {
  const facts = [
    ['Current value', row.value],
    ['Where it comes from', row.source],
  ];
  if (row.locked) facts.push(['Locked', 'Your organisation has locked this setting.']);
  return h(
    'div',
    { class: 'ds-form-card settings-setting', key: row.id, id: `setting-${row.id}` },
    h('h3', { class: 'ds-form__title' }, row.label),
    row.description ? h('p', { class: 'ds-field__help' }, row.description) : null,
    h(
      'dl',
      { class: 'ds-dl' },
      facts.map(([label, text]) => h('div', { class: 'ds-dl__item', key: label }, h('dt', null, label), h('dd', null, text))),
    ),
    row.refusal ? notice(row.refusal, { tone: 'danger', role: 'alert' }) : null,
    row.readOnly ? h('p', { class: 'ds-muted' }, row.readOnly) : null,
    row.levels.map((level) => levelForm(row, level)),
  );
}

/**
 * The Settings page.
 * @param {any} page the page model the kernel built
 */
export function preferences(page) {
  if (page.status === 'loading') {
    return [h('h1', { class: 'page-title' }, 'Settings'), h('p', { class: 'ds-muted', role: 'status' }, 'Reading your settings…')];
  }
  if (page.status === 'error') {
    return [h('h1', { class: 'page-title' }, 'Settings'), notice(page.error, { tone: 'danger', role: 'alert' })];
  }
  return [
    h('h1', { class: 'page-title' }, 'Settings'),
    h('p', { class: 'standfirst' }, 'What the site uses for you and for your organisation. A value you choose is kept for you alone.'),
    page.areas.length
      ? page.areas.map((area) =>
          sectionFrame({ id: `area-${area.id}`, label: area.label }, area.settings.map((row) => settingCard(row))),
        )
      : emptyState({ title: 'No settings yet', text: 'When your organisation or the platform defines a setting, it appears here.' }),
  ];
}
