// Contracts of the frontend kernel. Types only; nothing here runs.
//
// The kernel is two pure functions:
//
//   update(state, message) -> { state, effects }
//   view(state)            -> view model
//
// The worker driver feeds it messages and executes the effects it returns. The kernel never
// calls fetch, opens a socket, reads the clock or draws a random number: time and randomness
// arrive on every message as `at` and `rand`.

/**
 * @typedef {{ path: string, query: Record<string, string> }} Route
 *
 * @typedef {object} Session
 * @property {boolean} authenticated
 * @property {string} tenant
 * @property {string} tenant_name
 * @property {string} [principal]
 * @property {string} [display_name]
 * @property {string[]} roles
 * @property {string} [csrf_token]
 * @property {Record<string, string[]>} may
 * @property {boolean} may_manage_schema
 *
 * @typedef {object} Row  A projected record.
 * @property {number} id
 * @property {string} object
 * @property {number} version
 * @property {number} created_at
 * @property {number} updated_at
 * @property {string | null} created_by
 * @property {Record<string, any>} fields
 * @property {Record<string, any>} refs
 * @property {Record<string, any[]>} [blocks]
 * @property {number} [reading_minutes]
 *
 * @typedef {object} ViewModelData  What the BFF returns for a path.
 * @property {string} view
 * @property {string} title
 * @property {string} path
 * @property {Record<string, string>} params
 * @property {Record<string, any>} data
 * @property {string[]} depends_on
 * @property {number} revision
 * @property {{ accent: string, paper: string }} theme
 *
 * @typedef {{ check: string, detail: string }} FailedCheck
 * @typedef {{ code: string, message: string, kind?: string, failed?: { rule: string, detail: string }[], correlation?: string }} ApiError
 */

/**
 * Messages into the kernel. Every one carries `at` (milliseconds) and `rand` (0 to 1).
 *
 * @typedef {{ at: number, rand: number }} Stamp
 *
 * @typedef {(
 *   | { type: 'init', path: string, search: string, bootstrap: any, systemDark: boolean, storedTheme: string | null }
 *   | { type: 'navigate', href: string, replace?: boolean }
 *   | { type: 'popstate', path: string, search: string }
 *   | { type: 'input', form: string, name: string, value: string | boolean }
 *   | { type: 'submit', form: string, values: Record<string, string | boolean> }
 *   | { type: 'action', name: string, arg?: any }
 *   | { type: 'system_theme', dark: boolean }
 *   | { type: 'http_result', id: number, ok: boolean, status: number, body: any }
 *   | { type: 'ws_open' }
 *   | { type: 'ws_message', message: any }
 *   | { type: 'ws_closed' }
 *   | { type: 'timer', name: string, token: number }
 * ) & Stamp} Message
 */

/**
 * Effects out of the kernel. Descriptions only; the driver performs them.
 *
 * @typedef {(
 *   | { type: 'http', id: number, method: 'GET' | 'POST', path: string, body?: any, csrf?: string, write?: boolean, correlation: string }
 *   | { type: 'ws_connect', csrf?: string }
 *   | { type: 'ws_send', message: any }
 *   | { type: 'ws_close' }
 *   | { type: 'timer', name: string, token: number, ms: number }
 *   | { type: 'push_url', href: string, replace: boolean }
 *   | { type: 'store', key: string, value: string }
 *   | { type: 'scroll_top' }
 *   | { type: 'log', level: string, event: string, correlation: string, detail: string }
 * )} Effect
 */

export {};
