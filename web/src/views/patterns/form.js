// The `form` pattern: a form of the screen, titled by the operation it invokes, with a labelled
// control per field, the tenant's wording of a refusal on the form and on each field, and one
// primary submit. On a screen with several forms each is a disclosure (a native <details>), so the
// page stays scannable; one with an error is always open.
// @ts-check

import { h } from '../../components/dom.js';
import { button, notice } from '../../components/ds.js';

/**
 * The label of a field, marked when its input is optional.
 * @param {any} field @param {string} id
 */
function label(field, id) {
  return h('label', { class: 'ds-field__label', for: id }, field.label, field.required === false ? h('span', { class: 'ds-field__optional' }, ' (optional)') : null);
}

/** The ids a control is described by: its error. @param {any} field @param {string} id */
function described(field, id) {
  return field.error ? `${id}-error` : null;
}

/** @param {any} field @param {string} id */
function fieldError(field, id) {
  return field.error ? h('p', { class: 'ds-field__error', id: `${id}-error` }, field.error) : null;
}

/** The control of one field. @param {string} formId @param {any} field */
function control(formId, field) {
  const id = `${formId}-${field.name}`;
  const value = field.value;
  const aria = { 'aria-describedby': described(field, id), 'aria-invalid': field.error ? 'true' : null, required: field.required === true };
  const wrap = (/** @type {any} */ input) => h('div', { class: field.error ? 'ds-field ds-field--error' : 'ds-field', key: field.name }, label(field, id), input, fieldError(field, id));
  switch (field.control) {
    case 'textarea':
      return wrap(h('textarea', { id, class: 'ds-textarea', name: field.name, rows: 4, value: String(value ?? ''), ...aria }));
    case 'checkbox':
      return h(
        'div',
        { class: field.error ? 'ds-field ds-field--error' : 'ds-field', key: field.name },
        h('label', { class: 'ds-check' }, h('input', { id, name: field.name, type: 'checkbox', checked: value === true, 'aria-describedby': described(field, id), 'aria-invalid': field.error ? 'true' : null }), h('span', null, field.label)),
        fieldError(field, id),
      );
    case 'select':
      return wrap(
        h(
          'select',
          { id, class: 'ds-select', name: field.name, ...aria },
          (field.options ?? []).map((/** @type {any} */ o) => h('option', { value: o.value, selected: o.value === value, key: o.value }, o.label)),
        ),
      );
    case 'radio':
      return h(
        'fieldset',
        { class: field.error ? 'ds-field ds-fieldset ds-field--error' : 'ds-field ds-fieldset', key: field.name, 'aria-describedby': described(field, id) },
        h('legend', { class: 'ds-field__label' }, field.label, field.required === false ? h('span', { class: 'ds-field__optional' }, ' (optional)') : null),
        h(
          'div',
          { class: 'ds-options' },
          (field.options ?? []).map((/** @type {any} */ o, /** @type {number} */ i) =>
            h('label', { class: 'ds-check', key: o.value }, h('input', { id: `${id}-${i}`, name: field.name, type: 'radio', value: o.value, checked: o.value === value }), h('span', null, o.label)),
          ),
        ),
        fieldError(field, id),
      );
    default: {
      const type = field.control === 'password' || field.control === 'date' || field.control === 'number' ? field.control : 'text';
      return wrap(h('input', { id, class: 'ds-input', name: field.name, type, value: String(value ?? ''), inputmode: type === 'number' ? 'numeric' : null, ...aria }));
    }
  }
}

/** @param {any} node */
export function form(node) {
  const headingId = `form-${node.form.replace(/[^a-zA-Z0-9_-]/g, '-')}-title`;
  const body = h(
    'form',
    { class: 'ds-form', 'data-form': `tenant:${node.form}`, novalidate: true, 'aria-labelledby': headingId, 'aria-busy': node.busy ? 'true' : null },
    node.notice ? notice(node.notice, { tone: 'warning', role: 'status' }) : null,
    node.error ? notice(node.error, { tone: 'danger', role: 'alert' }) : null,
    h('div', { class: 'ds-form__fields' }, node.fields.map((/** @type {any} */ field) => control(node.form, field))),
    h('div', { class: 'ds-form__actions' }, button({ variant: 'primary', type: 'submit', disabled: node.busy }, node.busy ? 'Working…' : node.submitLabel)),
  );
  const title = h('h2', { class: 'ds-form__title', id: headingId }, node.title);
  if (node.collapsible) {
    return h(
      'section',
      { class: 'ds-form-card ds-form-card--disclosure', key: node.id },
      h('details', { class: 'ds-disclosure', open: node.open ? true : null }, h('summary', { class: 'ds-disclosure__summary' }, title), body),
    );
  }
  return h('section', { class: 'ds-form-card', key: node.id }, title, body);
}
