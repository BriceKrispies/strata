// Exemption lists of the audit checks of scripts/spec-gate.mjs (WG-5038).
//
// An exemption list is a list of groups, { reason, items }: every item is a key a check can report
// as a miss, and the reason says why that group of misses is accepted and names the task that
// removes it. The list can only shrink: an item that no miss matches is stale and fails the check
// like a miss does, so a fixed gap has to leave the list in the same change.

// Splits the misses of a check into those an exemption covers and those it does not, and finds the
// exemptions no miss matches. `problems` holds what is wrong with the list itself: a group without
// a reason, an item that is not a string, an item listed twice.
export function applyExemptions(misses, groups) {
  const problems = [];
  const owner = new Map();
  for (const [i, group] of (groups ?? []).entries()) {
    if (typeof group?.reason !== 'string' || group.reason.trim() === '') problems.push(`exemption group ${i} has no reason`);
    if (!Array.isArray(group?.items)) {
      problems.push(`exemption group ${i} has no items`);
      continue;
    }
    for (const item of group.items) {
      if (typeof item !== 'string' || item === '') problems.push(`exemption group ${i} has an item that is not a string`);
      else if (owner.has(item)) problems.push(`exemption ${item} is listed twice`);
      else owner.set(item, i);
    }
  }
  const missed = new Set(misses);
  const remaining = [...missed].filter((miss) => !owner.has(miss));
  const exempt = [...missed].filter((miss) => owner.has(miss));
  const stale = [...owner.keys()].filter((item) => !missed.has(item));
  return { remaining, exempt, stale, problems };
}

// The verdict of one audit check: the failures are the misses no exemption covers, the stale
// exemptions and the problems of the list. `total` is how many things the check looked at.
export function auditVerdict(name, total, misses, groups) {
  const { remaining, exempt, stale, problems } = applyExemptions(misses, groups);
  const lines = [
    ...remaining.map((miss) => `missing: ${miss}`),
    ...stale.map((item) => `stale exemption (it matches nothing; delete it): ${item}`),
    ...problems,
  ];
  return {
    name,
    ok: lines.length === 0,
    detail: `${total - remaining.length - exempt.length} of ${total} ok, ${exempt.length} exempt, ${remaining.length} missing, ${stale.length} stale exemptions`,
    lines,
  };
}
