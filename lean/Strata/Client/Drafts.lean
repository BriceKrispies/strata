/-
The person's own typed text after a revocation or an erasure (owner decision D-FE-13 of
2026-11-22; docs/frontend.md, "The person's own typed text").

A draft holds the person's typed text and, separately, values that came from the server. When a
revocation or an erasure orphans the draft, the server values are removed at once and the typed
text is kept so the person can copy it out ("copy and dismiss"). The draft records that it is
orphaned, with the erasure epoch and the time at orphaning. The typed text is deleted at the realm's
idle session lifetime after orphaning, or by the next erasure after that, whichever comes first;
dismissing deletes it too.

Proved:

- **orphaning removes other people's data at once** (`orphan_drops_server_values`);
- **the typed text survives the erasure that orphaned it** (`survives_orphaning_erasure`);
- **it is gone after the next erasure, or after the idle lifetime, whichever comes first**
  (`gone_after_next_erasure`, `gone_after_idle`), and **kept until then** (`kept_until`).

No `sorry`, no added axioms.
-/

namespace Strata.Client.Drafts

structure Draft where
  /-- The person's typed text (present or deleted). -/
  typed : Option Nat
  /-- Values that came from the server. -/
  server : Option Nat
  /-- Orphaned: the erasure epoch and the time at orphaning. -/
  orphaned : Option (Nat × Nat)

inductive DOp where
  /-- A revocation or an erasure orphans the draft; `x` is the erasure epoch after it, `t` the time. -/
  | orphan (x t : Nat)
  /-- The browser hears erasure epoch `x` at time `t`. -/
  | hear (x t : Nat)
  /-- Time passes to `t`. -/
  | tick (t : Nat)
  /-- Copy and dismiss. -/
  | dismiss

/-- One step, with the realm's idle session lifetime `I`. -/
def dstep (I : Nat) (d : Draft) : DOp → Draft
  | .orphan x t =>
    match d.orphaned with
    | none => { d with server := none, orphaned := some (x, t) }
    | some _ => { d with server := none }
  | .hear x t =>
    match d.orphaned with
    | some (x0, t0) => if x0 < x ∨ t0 + I ≤ t then { d with typed := none } else d
    | none => d
  | .tick t =>
    match d.orphaned with
    | some (_, t0) => if t0 + I ≤ t then { d with typed := none } else d
    | none => d
  | .dismiss => { d with typed := none }

/-- **Orphaning removes the server's values at once.**
    Verifies: FE-017 -/
theorem orphan_drops_server_values (I : Nat) (d : Draft) (x t : Nat) :
    (dstep I d (.orphan x t)).server = none := by
  unfold dstep; cases d.orphaned <;> rfl

/-- **The typed text survives the erasure that orphaned it**: hearing the same epoch before the idle
    lifetime keeps it.
    Verifies: FE-017 -/
theorem survives_orphaning_erasure (I : Nat) (d : Draft) (x t t' : Nat) (hnew : d.orphaned = none)
    (ht : t' < t + I) :
    (dstep I (dstep I d (.orphan x t)) (.hear x t')).typed = d.typed := by
  have h1 : ¬ (t + I ≤ t') := by omega
  simp [dstep, hnew, h1]

/-- **The next erasure after orphaning deletes the typed text.**
    Verifies: FE-017 -/
theorem gone_after_next_erasure (I : Nat) (d : Draft) (x0 t0 x t : Nat)
    (ho : d.orphaned = some (x0, t0)) (hx : x0 < x) : (dstep I d (.hear x t)).typed = none := by
  simp [dstep, ho, hx]

/-- **The idle lifetime after orphaning deletes the typed text.**
    Verifies: FE-017 -/
theorem gone_after_idle (I : Nat) (d : Draft) (x0 t0 t : Nat) (ho : d.orphaned = some (x0, t0))
    (ht : t0 + I ≤ t) : (dstep I d (.tick t)).typed = none := by
  simp [dstep, ho, ht]

/-- Until the next erasure and the idle lifetime, a hearing or a tick keeps the typed text. -/
theorem kept_until (I : Nat) (d : Draft) (x0 t0 x t : Nat) (ho : d.orphaned = some (x0, t0))
    (hx : x ≤ x0) (ht : t < t0 + I) :
    (dstep I d (.hear x t)).typed = d.typed ∧ (dstep I d (.tick t)).typed = d.typed := by
  have h1 : ¬ (x0 < x ∨ t0 + I ≤ t) := by omega
  have h2 : ¬ (t0 + I ≤ t) := by omega
  exact ⟨by simp [dstep, ho, h1], by simp [dstep, ho, h2]⟩

end Strata.Client.Drafts
