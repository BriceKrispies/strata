"""The keep/delete plan of the spec-only test cut (WG-5357).

Every hand-written integration test function in crates/*/tests is either:
- kept as an exception (generated suites, the evolution catalogue, the differentials, purity);
- kept because it is in a minimal set of tests that together carry every Verifies id (greedy set
  cover, preferring tests that carry more ids, then shorter names);
- deleted.

The spec compiler and spec-expr are listed separately for the toolchain guard, which a person
chooses. Writes var/spec-only-plan.json and prints a summary.
"""
import json, os, re, sys
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXCEPT_FILES = re.compile(
    r"(tests/conformance/|/generated/|contract_conformance|kernel/tests/evolution\.rs|"
    r"generated_differential\.rs|strata_differential\.rs|crates/system-differential/|"
    r"verification/tests/correspondence\.rs|verification/tests/purity\.rs)"
)
TOOLCHAIN = re.compile(r"^crates/(spec-compiler|spec-expr|spec-expr-cases|spec-compiler-[a-z]+-fixture)/")
ATTR = re.compile(r"^\s*#\[(tokio::)?test\b")
FN = re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
ID = re.compile(r"\b([A-Za-z]{2,5}-\d{1,5})\b")


def tests_of(path):
    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
    out = []
    for i, line in enumerate(lines):
        if not ATTR.match(line):
            continue
        name = None
        for j in range(i + 1, min(i + 6, len(lines))):
            m = FN.match(lines[j])
            if m:
                name = m.group(1)
                break
        if not name:
            continue
        ids = set()
        k = i - 1
        while k >= 0:
            t = lines[k].strip()
            if t.startswith("#[") or t.startswith("//"):
                if "Verifies:" in t:
                    ids |= {x.upper() for x in ID.findall(t.split("Verifies:", 1)[1])}
                k -= 1
                continue
            break
        out.append({"name": name, "ids": sorted(ids)})
    return out


def main():
    tests = []
    for root, dirs, files in os.walk(os.path.join(ROOT, "crates")):
        dirs[:] = [d for d in dirs if d not in ("target", "fixtures")]
        rel = os.path.relpath(root, ROOT).replace(os.sep, "/")
        if "/tests" not in "/" + rel:
            continue
        for f in files:
            if f.endswith(".rs"):
                p = f"{rel}/{f}"
                for t in tests_of(os.path.join(ROOT, p)):
                    t["file"] = p
                    tests.append(t)
    plan = {}
    candidates = []
    for t in tests:
        key = f"{t['file']}::{t['name']}"
        if EXCEPT_FILES.search(t["file"]):
            plan[key] = "keep:exception"
        elif TOOLCHAIN.match(t["file"]):
            plan[key] = "toolchain" if not t["ids"] else "toolchain:verifies"
            if t["ids"]:
                candidates.append(t)
        elif not t["ids"]:
            plan[key] = "delete:untraced"
        else:
            candidates.append(t)
    # Ids already carried by exception tests need no other carrier.
    covered = set()
    for t in tests:
        if plan.get(f"{t['file']}::{t['name']}") == "keep:exception":
            covered |= set(t["ids"])
    remaining = set(i for t in candidates for i in t["ids"]) - covered
    chosen = set()
    while remaining:
        best = max(candidates, key=lambda t: (len(set(t["ids"]) & remaining), -len(t["name"])))
        gain = set(best["ids"]) & remaining
        if not gain:
            break
        chosen.add(f"{best['file']}::{best['name']}")
        remaining -= gain
    for t in candidates:
        key = f"{t['file']}::{t['name']}"
        if key in chosen:
            plan[key] = "keep:verifies"
        elif plan.get(key) == "toolchain:verifies":
            plan[key] = "toolchain"
        else:
            plan[key] = "delete:redundant"
    by_file = defaultdict(lambda: defaultdict(list))
    for key, verdict in plan.items():
        f, n = key.split("::")
        by_file[f][verdict].append(n)
    json.dump({f: dict(v) for f, v in sorted(by_file.items())}, open(os.path.join(ROOT, "var", "spec-only-plan.json"), "w"), indent=1)
    counts = defaultdict(int)
    for v in plan.values():
        counts[v] += 1
    ids_before = set(i for t in tests for i in t["ids"])
    ids_after = set(i for t in tests if plan[f"{t['file']}::{t['name']}"].startswith(("keep", "toolchain")) for i in t["ids"])
    print(f"{len(tests)} tests; " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    print(f"ids before {len(ids_before)}, kept {len(ids_after)}, lost {sorted(ids_before - ids_after)}")


if __name__ == "__main__":
    sys.exit(main())
