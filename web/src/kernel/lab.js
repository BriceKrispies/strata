// Impact Lab part of the frontend kernel. Filled in with the simulator.
// @ts-check

export function labInit() {
  return {};
}

/** @returns {{ state: any, effects: any[] } | null} */
export function labUpdate(_state, _msg, _helpers) {
  return null;
}

export function labView(_state) {
  return { kind: 'lab', pending: true };
}
