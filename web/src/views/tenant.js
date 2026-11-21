// The interpreter's drawing of a tenant screen (D-8250, INT-3257): the page model that
// `kernel/tenant-ui.js` builds from the screen's part of the tenant's interface definition, drawn
// with the page's own building blocks. Nothing here belongs to a product: every tenant screen of
// every product is drawn by these functions.
// @ts-check

import { h } from '../components/dom.js';
import { control, loadMore } from '../components/parts.js';

/** One record: its fields as a description list. @param {{ id: any, fields: { name: string, value: string }[] }} record */
function record(record) {
  return h(
    'dl',
    { class: 'record' },
    record.fields.map((f) => [h('dt', { key: `${f.name}-name` }, f.name), h('dd', { key: `${f.name}-value` }, f.value)]),
  );
}

/** One section of the view: a list of records, or one record. @param {any} section */
function section(section) {
  const id = `section-${section.name}`;
  return h(
    'section',
    { class: 'section', 'aria-labelledby': id, key: section.name },
    h('h2', { class: 'section__title', id }, section.name),
    section.single
      ? record(section.record)
      : section.rows.length
        ? h('ul', { class: 'records' }, section.rows.map((/** @type {any} */ row) => h('li', { key: String(row.id) }, record(row))))
        : h('p', { class: 'empty' }, 'Nothing here yet.'),
    section.single ? null : loadMore(section.more ? { section: section.name, busy: false } : null),
  );
}

/** The control of one field of a form. @param {string} formId @param {any} field */
function input(formId, field) {
  const id = `${formId}-${field.name}`;
  const value = field.value;
  switch (field.control) {
    case 'textarea':
      return control(id, field.label, field, h('textarea', { name: field.name, rows: 4, value: String(value ?? '') }));
    case 'checkbox':
      return h(
        'div',
        { class: 'field field--check' },
        h('input', { id, name: field.name, type: 'checkbox', checked: value === true }),
        h('label', { for: id }, field.label),
        field.error ? h('p', { class: 'field__error', id: `${id}-error` }, field.error) : null,
      );
    case 'select':
      return control(
        id,
        field.label,
        field,
        h('select', { name: field.name }, field.options.map((/** @type {any} */ o) => h('option', { value: o.value, selected: o.value === value, key: o.value }, o.label))),
      );
    case 'radio':
      return h(
        'fieldset',
        { class: field.error ? 'field field--error' : 'field' },
        h('legend', null, field.label),
        field.options.map((/** @type {any} */ o, /** @type {number} */ i) => h(
          'div',
          { class: 'field--check', key: o.value },
          h('input', { id: `${id}-${i}`, name: field.name, type: 'radio', value: o.value, checked: o.value === value }),
          h('label', { for: `${id}-${i}` }, o.label),
        )),
        field.error ? h('p', { class: 'field__error' }, field.error) : null,
      );
    default: {
      const type = field.control === 'password' || field.control === 'date' || field.control === 'number' ? field.control : 'text';
      return control(id, field.label, field, h('input', { name: field.name, type, value: String(value ?? '') }));
    }
  }
}

/** One form of the screen. @param {any} form */
function form(form) {
  const dataForm = `tenant:${form.id}`;
  return h(
    'form',
    { class: 'form form--narrow', 'data-form': dataForm, novalidate: true, key: form.id },
    form.notice ? h('p', { class: 'form__notice', role: 'status' }, form.notice) : null,
    form.error ? h('p', { class: 'form__error', role: 'alert' }, form.error) : null,
    form.fields.map((/** @type {any} */ field) => input(form.id, field)),
    h('div', { class: 'form__actions' }, h('button', { class: 'button', type: 'submit', disabled: form.busy }, form.busy ? 'Working…' : form.submitLabel)),
  );
}

/** The actions of the screen, as buttons; one with a confirmation asks for a second press. @param {any[]} actions */
function actions(actions) {
  if (!actions.length) return null;
  return h(
    'p',
    { class: 'toolbar' },
    actions.map((a) => h(
      'button',
      { class: a.armed ? 'button button--danger' : 'button button--quiet', type: 'button', 'data-action': 'tenant.action', 'data-arg': JSON.stringify(a.id), key: a.id },
      a.armed && a.confirm ? a.confirm : a.label,
    )),
  );
}

/** A tenant screen. @param {any} page */
export function screen(page) {
  const forms = page.forms.map(form);
  const sections = page.sections.map(section);
  return [
    h('h1', { class: 'page-title' }, page.heading),
    actions(page.actions),
    ...(page.formsFirst ? [forms, sections] : [sections, forms]),
  ];
}
