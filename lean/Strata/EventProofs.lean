/-
Events, revisions and effects (slice 9): the properties of `Strata.transition`.

An accepted transition against a tenant that exists is the step of `stepTenant` with the tenant committed
(`commitT`: the revision raised by one, the event sequence by the events, the schema version by a schema change),
and the events the kernel emits for the intent, numbered after the sequence, one `Publish` effect per event in
order. A refused step has no transition, so it emits no event and leaves the revision where it was.
-/

import Strata.Model
import Strata.StepParts
import Strata.Workflows
import Strata.References

namespace Strata

/-- An accepted transition is a step: its world is the committed world, its events the events of the step, its
    effects one `Publish` per event. -/
theorem transition_ok {w : World} {i : Intent} {c : Ctx} {tr : Transition}
    (h : transition w i c = .ok tr) :
    ∃ w', step w i c = .ok w' ∧ tr.world = commitWorld w w' c i ∧
      tr.events = stepEvents w tr.world c i ∧ tr.effects = tr.events.map (fun e => .publish e.seq) := by
  unfold transition at h
  cases hs : step w i c with
  | error e => simp [hs] at h
  | ok w' =>
    simp only [hs, Except.ok.injEq] at h
    subst h
    exact ⟨w', rfl, rfl, rfl, rfl⟩

/-- A refused step has no transition, so it emits no event and changes no state. -/
theorem transition_refused {w : World} {i : Intent} {c : Ctx} {e : Err}
    (h : step w i c = .error e) : transition w i c = .error e := by
  unfold transition
  simp [h]

/-- The events do not depend on the revision, the event sequence or the schema version of the tenant they are
    read from: committing a tenant leaves its events as they were. -/
theorem eventBodies_commit (t t' : Tenant) (p : Principal) (i : Intent) :
    eventBodies t p i (commitT t t' p i) = eventBodies t p i t' := by
  cases i <;> first
    | rfl
    | simp [eventBodies, commitT, Intent.line, Tenant.recordAt, Tenant.typ?, Tenant.env, Intent.touched]

/-- An accepted step against a tenant that exists: the tenant it wrote, committed, and the events of the intent,
    numbered after the tenant's event sequence and committed at the next revision. -/
theorem transition_existing {w : World} {i : Intent} {c : Ctx} {tr : Transition} {t : Tenant}
    (h : transition w i c = .ok tr) (ht : w c.tenant = some t) :
    ∃ t₁, stepTenant t c.principal i = .ok t₁ ∧ tr.world c.tenant = some (commitT t t₁ c.principal i) ∧
      tr.events = numberEvents (t.eventSeq + 1) (t.revision + 1) c.principal 0
        (eventBodies t c.principal i t₁) := by
  obtain ⟨w', hs, hw, hev, -⟩ := transition_ok h
  rcases step_cases hs with ⟨t₂, t₁, ht₂, hst, hw'⟩ | ⟨_, _, _, hnone, _⟩
  · have heq : t₂ = t := Option.some.inj (ht₂.symm.trans ht)
    rw [heq] at hst
    subst hw'
    have hc : commitWorld w (fun x => if x = c.tenant then some t₁ else w x) c i c.tenant
        = some (commitT t t₁ c.principal i) := by simp [commitWorld, ht]
    refine ⟨t₁, hst, ?_, ?_⟩
    · rw [hw]; exact hc
    · rw [hev, hw]
      simp only [stepEvents, ht, hc]
      rw [eventBodies_commit]
      rfl
  · rw [hnone] at ht
    cases ht

/-- The numbering of a list of events: the `k`-th is `s + k`, so the sequence numbers are consecutive. -/
theorem numberEvents_map_seq (s r : Nat) (p : Principal) :
    ∀ (k : Nat) (l : List EventBody),
      (numberEvents s r p k l).map (fun e => e.seq) = (List.range l.length).map (fun j => s + k + j)
  | _, [] => rfl
  | k, _ :: bs => by
    have ih := numberEvents_map_seq s r p (k + 1) bs
    have e : (List.range (bs.length + 1)).map (fun j => s + k + j) =
        (s + k) :: (List.range bs.length).map (fun j => s + (k + 1) + j) := by
      rw [List.range_succ_eq_map, List.map_cons, List.map_map]
      simp only [List.cons.injEq]
      refine ⟨by simp, ?_⟩
      apply List.map_congr_left
      intro a _
      simp only [Function.comp_def]
      omega
    simp only [numberEvents, List.map_cons, ih, List.length_cons]
    rw [e]

theorem numberEvents_length (s r : Nat) (p : Principal) :
    ∀ (k : Nat) (l : List EventBody), (numberEvents s r p k l).length = l.length
  | _, [] => rfl
  | k, _ :: bs => by
    simp [numberEvents, numberEvents_length s r p (k + 1) bs]

/-- The events of an accepted step against a tenant that exists are numbered after its event sequence, one
    each, and their sequence numbers are consecutive. -/
theorem transition_seqs {w : World} {i : Intent} {c : Ctx} {tr : Transition} {t : Tenant}
    (h : transition w i c = .ok tr) (ht : w c.tenant = some t) :
    tr.events.map (fun e => e.seq) = (List.range tr.events.length).map (fun j => t.eventSeq + 1 + j) := by
  obtain ⟨t₁, -, -, hev⟩ := transition_existing h ht
  rw [hev, numberEvents_map_seq, numberEvents_length]

/-- An accepted step publishes each of its events, in order, with one `Publish` effect per event. -/
theorem transition_publish {w : World} {i : Intent} {c : Ctx} {tr : Transition}
    (h : transition w i c = .ok tr) :
    tr.effects = tr.events.map (fun e => .publish e.seq) := by
  obtain ⟨-, -, -, -, he⟩ := transition_ok h
  exact he

/-- An accepted step against a tenant that exists raises its revision by exactly one. -/
theorem transition_revision {w : World} {i : Intent} {c : Ctx} {tr : Transition} {t : Tenant}
    (h : transition w i c = .ok tr) (ht : w c.tenant = some t) :
    ∃ t', tr.world c.tenant = some t' ∧ t'.revision = t.revision + 1 := by
  obtain ⟨t₁, -, hw, -⟩ := transition_existing h ht
  exact ⟨_, hw, rfl⟩

/-- An accepted step against no tenant creates the tenant at revision 1, with its one `TenantCreated` event as the
    first of its sequence. -/
theorem transition_created {w : World} {owner : PrincipalId} {name : String} {c : Ctx} {tr : Transition}
    (h : transition w (.createTenant owner name) c = .ok tr) (hw : w c.tenant = none) :
    tr.world c.tenant = some (genesisTenant owner) ∧
      tr.events = [{ seq := 1, revision := 1, principal := c.principal, body := .tenantCreated name }] := by
  obtain ⟨w', hs, hwt, hev, -⟩ := transition_ok h
  rcases step_cases hs with ⟨t₂, t₁, ht₂, -, -⟩ | ⟨o, n, hi, -, hw'⟩
  · rw [hw] at ht₂; cases ht₂
  · cases hi
    subst hw'
    have hc : commitWorld w (fun x => if x = c.tenant then some (genesisTenant owner) else w x) c
        (.createTenant owner name) c.tenant = some (genesisTenant owner) := by
      simp [commitWorld, hw]
    refine ⟨?_, ?_⟩
    · rw [hwt]; exact hc
    · rw [hev, hwt]
      simp only [stepEvents, hw, hc]
      simp [genesisTenant]

/-- The object type an accepted record intent names has a definition in the tenant it addresses. -/
theorem typ_of_stepTenant_create {t t₁ : Tenant} {p : Principal} {ty : TypeId} {fs : Fields}
    (h : stepTenant t p (.create ty fs) = .ok t₁) : ∃ o, t.typ? ty = some o := by
  cases hT : t.typ? ty with
  | none => simp [stepTenant, hT] at h
  | some o => exact ⟨o, rfl⟩

/-- An accepted create writes its record under the next id of its type, and emits exactly one `RecordCreated` event
    carrying that record, numbered after the tenant's sequence and committed at the next revision. -/
theorem transition_create {w : World} {ty : TypeId} {fs : Fields} {c : Ctx} {tr : Transition} {t : Tenant}
    (h : transition w (.create ty fs) c = .ok tr) (ht : w c.tenant = some t) :
    ∃ r, tr.events = [{ seq := t.eventSeq + 1, revision := t.revision + 1, principal := c.principal, body := .recordCreated ty t.nextId r }] := by
  obtain ⟨t₁, hst, -, hev⟩ := transition_existing h ht
  obtain ⟨o, hT⟩ := typ_of_stepTenant_create hst
  obtain ⟨r, -, -, -, ht1⟩ := create_ok_parts hT hst
  refine ⟨r, ?_⟩
  have hk : t₁.typ? ty = some { o with records := (t.nextId, r) :: o.records } := by
    rw [ht1]; exact typ?_putType_self t ty o _ hT
  have hr : t₁.recordAt ty t.nextId = some r := by
    unfold Tenant.recordAt; rw [hk]; simp
  rw [hev]
  simp [eventBodies, hr, numberEvents]

/-- An accepted update emits exactly one `RecordUpdated` event: the record as stored before, the record as stored
    after, and the workflow transitions the update fired. -/
theorem transition_update {w : World} {ty : TypeId} {id : RecordId} {fs : Fields} {ev : Option Nat} {c : Ctx}
    {tr : Transition} {t : Tenant}
    (h : transition w (.update ty id fs ev) c = .ok tr) (ht : w c.tenant = some t) :
    ∃ old r, t.recordAt ty id = some old ∧
      tr.events = [{ seq := t.eventSeq + 1, revision := t.revision + 1, principal := c.principal, body := .recordUpdated ty id old r (firedAll c.principal old (patched old fs) (((t.typ? ty).map ObjType.workflows).getD [])) }] := by
  obtain ⟨t₁, hst, -, hev⟩ := transition_existing h ht
  obtain ⟨o, hT⟩ : ∃ o, t.typ? ty = some o := by
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at hst
    | some o => exact ⟨o, rfl⟩
  obtain ⟨old, hl⟩ : ∃ old, o.records.lookup id = some old := by
    cases hl : o.records.lookup id with
    | none => simp [stepTenant, hT, hl] at hst
    | some old => exact ⟨old, rfl⟩
  obtain ⟨r, -, hr, -⟩ := update_writes_fired hT hl hst
  have hold : t.recordAt ty id = some old := by simp [Tenant.recordAt, hT, hl]
  refine ⟨old, r, hold, ?_⟩
  rw [hev]
  simp [eventBodies, hold, hr, hT, numberEvents]

/-- The record a delete removes is stored, so the delete's events are read from it. -/
theorem stored_of_stepTenant_delete {t t₁ : Tenant} {p : Principal} {ty : TypeId} {id : RecordId} {ev : Option Nat}
    (h : stepTenant t p (.delete ty id ev) = .ok t₁) : ∃ old, t.recordAt ty id = some old := by
  cases hT : t.typ? ty with
  | none => simp [stepTenant, hT] at h
  | some o =>
    cases hl : o.records.lookup id with
    | none => simp [stepTenant, hT, hl] at h
    | some old => exact ⟨old, by simp [Tenant.recordAt, hT, hl]⟩

/-- An accepted delete emits one `RecordUpdated` event for each record it rewrites, in the kernel's order, then the
    `RecordDeleted` event of the record itself: the number of events is the number of referrers plus one. -/
theorem transition_delete {w : World} {ty : TypeId} {id : RecordId} {ev : Option Nat} {c : Ctx}
    {tr : Transition} {t : Tenant}
    (h : transition w (.delete ty id ev) c = .ok tr) (ht : w c.tenant = some t) :
    ∃ old, t.recordAt ty id = some old ∧
      tr.events.length = (referrerEntries t ty id).length + 1 ∧
      tr.events = numberEvents (t.eventSeq + 1) (t.revision + 1) c.principal 0
        (referrerEvents t ty id ++ [.recordDeleted ty id old]) := by
  obtain ⟨t₁, hst, -, hev⟩ := transition_existing h ht
  obtain ⟨old, hold⟩ := stored_of_stepTenant_delete hst
  refine ⟨old, hold, ?_, ?_⟩
  · rw [hev, numberEvents_length]
    simp only [eventBodies, hold, referrerEvents, List.length_append, List.length_map, List.length_mergeSort, List.length_singleton]
  · rw [hev]
    simp only [eventBodies, hold]

/-- An accepted schema change of a field's required flag emits one `SchemaChanged` event: the version it made, the
    field it changed, and the number of records of the touched types that violate the schema now (the kernel's
    `grandfathered`: the records a lax change keeps). -/
theorem transition_setRequired {w : World} {ty : TypeId} {f : FieldId} {strict : Bool} {c : Ctx}
    {tr : Transition} {t : Tenant}
    (h : transition w (.setRequired ty f strict) c = .ok tr) (ht : w c.tenant = some t) :
    ∃ t₁, stepTenant t c.principal (.setRequired ty f strict) = .ok t₁ ∧
      tr.events = [{ seq := t.eventSeq + 1, revision := t.revision + 1, principal := c.principal, body := .schemaChanged (t.schemaVersion + 1) [.field ty f] (violatedCount t.limits (scopedTypes t₁.types (Intent.touched t (.setRequired ty f strict))) t₁.env) }] := by
  obtain ⟨t₁, hst, -, hev⟩ := transition_existing h ht
  refine ⟨t₁, hst, ?_⟩
  rw [hev]
  simp [eventBodies, Intent.line, numberEvents]

end Strata
