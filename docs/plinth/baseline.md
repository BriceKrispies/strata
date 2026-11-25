# Strata baseline for the Plinth Suite port

Task: WG-5615. Read-only audit of Strata at `C:\dev\strata`, branch `main`, commit `a3aee42b` (committed 2026-11-25). Audit date 2026-11-25. Nothing was built, tested or run. The recorded Exegra verdicts in `var/exegra-verdicts.json` (dated 2026-11-25) were read, not re-evaluated.

## 0. How to read this

**Citation forms.** Paths are relative to `C:\dev\strata` unless absolute. `spec:<ID>` is an id in `specifications/requirements.jsonc` (for example `spec:ADM-152`). `decision:<ID>` is a decision in `docs/decisions.md` (`D11`) or in `specifications/intent.jsonc` (`D-700`, `D-4915`, `D-6547`). `claim:<m-id>` is an Exegra model claim in `specifications/behaviour.jsonc`. `test:` names a Rust test. `lean:` names a Lean theorem. `Verifies:` is the doc-comment tag a test or theorem carries.

**What the gate checks.** `gate.json` sets `startedMilestones` to `[1]` (`gate.json:2`). A requirement is checked by the gate only when its milestone is started. Of the 1737 requirement records in `specifications/requirements.jsonc`, 63 are in milestone 1. None of the requirements this audit covers (ADM, IDN, TOK, SEC, AGT, TEN, LDG, CMS-012, IDN-023, CNV) is in milestone 1. The README's counts ("37 of 739", `specifications/README.md:186-189`) are stale: the file now holds 1737 requirement records. The gate's stated boundary is in `specifications/README.md:168-211`.

**What a `Verifies:` tag proves.** A tag is a hand-written claim that the test checks the requirement. Nothing checks that the test asserts the statement (`specifications/README.md:190-192`). I counted tags by grep over `crates/` and `lean/`: 615 distinct ids are tagged. Per prefix, tagged ids out of the requirement refs: ADM 10 of 31, IDN 31 of 62, TOK 13 of 16, SEC 2 of 16, AGT 7 of 51, TEN 5 of 95, LDG 40 of 63, OPS 8 of 73, PRV 31 of 98.

**What an Exegra claim proves.** `var/exegra-verdicts.json` records all 507 `m-` claims as `verified`. The 1781 `not-evaluated` entries are requirement-level. A model claim is a link to a requirement, not a proof that the claim says what the requirement says (`specifications/README.md:198-203`).

**Fundamentals.** Each gap is a general capability with an id `F-<kebab-slug>`. Section 4 lists them in the shape of `docs/plinth/inventory.schema.json` (`fundamental` definition). The `neededBy` field names the baseline area; mapping to Plinth items belongs to the inventory step.

**Summary of the top gaps.** Area 1: `F-role-definition-authority` (a schema manager can widen a role it holds) and `F-identity-admin-capabilities` (identity administration is owner-only in code). Area 2: `F-ledger-tamper-evidence` and `F-ledger-posting-audit` (entries are protected by database triggers alone and postings reach no audit log). Area 3: `F-accessible-by-construction` (the accessibility rule is proposed, not decided) and `F-definition-time-a11y-checks` (the checks the design names are not implemented).

---

## 1. Area 1: authorization and admin

### 1.1 Guaranteed

**Roles and capabilities.**
- A tenant defines roles as named permission sets over object actions and capabilities. `public` and `owner` are built in and cannot be defined (`crates/model-core/src/types.rs:94-99` for `public_role` and `owner_role`; `crates/kernel/src/transition.rs:1799-1802`). Spec `spec:ADM-150`.
- A principal holds the permissions of the roles assigned to it directly and of each team's roles, and no other (`crates/kernel/src/rules.rs:150-163` `assigned_roles`; `:167-184` `roles_of`; `:187-192` `effective_permissions`). Spec `spec:ADM-151`. Tagged at `crates/substrate/tests/publication_template.rs:427-428`.
- Capabilities are held only because a role names them: `held_capabilities` (`crates/kernel/src/rules.rs:365`) and `role_capabilities` (`:349`) read only role grants. A role's name confers nothing.

**Policies.**
- A policy is `PolicyDef { id, role, resource, actions, condition }` (`crates/model-core/src/types.rs:5117-5124`). Resources are `Object`, `AnyObject`, `Schema`, `Members` and `Ledger` (`types.rs:5094-5103`). Actions are `Read`, `Create`, `Update`, `Delete` and `Manage` (`types.rs:5075-5081`).
- Decisions are default-deny. The owner is allowed everything. Otherwise a policy must match role, resource and action, and its condition must evaluate to true. A condition that fails to evaluate denies (`crates/kernel/src/rules.rs:385-398` doc; `authorize` at `:398`; owner bypass at `:514`).
- Shape checks at definition: `Manage` fits only `Schema` and `Members`, and a policy on every object cannot read fields (`crates/kernel/src/schema.rs:462-478`).
- Field read rules fail closed. A field whose read condition is false, or errors, is withheld from the record. A condition that follows a reference the snapshot does not hold asks for that reference instead of deciding (`crates/kernel/src/read.rs:45-61`).

**Operation invokers.**
- An operation is invoked by a signed-in person that its invokers name. Every signed-in person holds `tenant-operation-invoke` (`crates/kernel/src/operations.rs:8-11`, decision D-6547, `OPR-1546`).
- Record writes in an operation are authorised by the operation's own powers, which the definer must hold when the definition is made (`crates/kernel/src/transition.rs:2580` `require_operation_powers`, called from `:2519` `require_definition_authority`). They are not checked against the invoker's policies (`crates/kernel/src/operations.rs:17-23`, decision D-6547, `OPR-1547`). A record a step creates names the invoker as creator (`operations.rs:24`, `OPR-1548`).
- Test: `test:crates/kernel/tests/operations.rs` (operation tests; the definer-power refusal is covered by `crates/kernel/tests/approvals.rs:234` `an_operation_that_a_step_names_is_not_removed_while_it_does` for the removal rule only).

**Owner.**
- The owner holds every permission (`crates/kernel/src/rules.rs:514`). Only an owner confers the owner role (`crates/kernel/src/rules.rs:312-318`).
- A change that takes a role away must leave an owner (`crates/kernel/src/transition.rs:1239-1272`, `effective_owners` and `require_an_owner_remains`). Spec `spec:ADM-1081`. Test: `crates/kernel/tests/behaviour.rs::no_sequence_of_revocations_leaves_a_tenant_without_an_owner` (`:1070`, tag `:1065`).

**Tenant and platform roles.**
- Tenant roles are the schema's roles. Platform roles are not among a tenant's roles (`crates/kernel/src/settings.rs:403-404`).
- The platform operator is a single credential, `STRATA_OPERATOR_TOKEN`, read at `crates/backend/src/app.rs:290-293` and `:350`. There is no platform role that a product can name (open task WG-5474).

**Assign-role and conferral (ADM-152).**
- A role may be conferred only by a principal that holds every permission and every capability it grants, counting policy-granted powers (`crates/kernel/src/rules.rs:312-345` `may_confer`; `:279-301` `missing_policy_powers`). Spec `spec:ADM-152`, `spec:ADM-1085`.
- Enforced on every path that hands out or takes away a role:
  - a direct role change, `change_member` (`crates/kernel/src/transition.rs:1162-1184`, conferral at `:1184`), which applies to revocation too (`spec:ADM-1080`);
  - team roles, `team_role` (`:1323-1340`, conferral at `:1337`);
  - team membership, `team_member` (`:1372-1385`, conferral at `:1380`, also on removal);
  - invitations, `create_invitation` (`:1409-1443`, conferral at `:1441`) and `revoke_invitation` (`:1464-1493`, conferral at `:1491`).
- At redemption the issuer's authority is asked again (`crates/kernel/src/transition.rs:1591-1597`, `spec:ADM-1084`). Redemption refuses a wrong verified address, a redeemed or revoked invitation, and an expired one (`:1504-1600`, `spec:ADM-153`, `spec:ADM-1086`, `spec:ADM-1087`). Invitations last fourteen days (`crates/kernel/src/transition.rs:1458`, `spec:ADM-1082`).
- Tests: `crates/kernel/tests/behaviour.rs::assigning_a_role_is_refused_unless_the_assigner_holds_all_of_its_permissions` (`:683`, tag `:677`), `::a_role_empowered_by_a_policy_is_not_conferred_without_that_power` (`:899`), `::revoking_a_role_needs_the_authority_to_confer_it` (`:972`), `::an_invitation_expires` (`:1155`), `::an_invitation_can_be_revoked` (`:1194`). Tags for `spec:ADM-1080` to `spec:ADM-1085` are at `behaviour.rs:893, 966, 1065, 1151, 1189, 1230`.
- Gap in the same mechanism: see `F-role-definition-authority` in section 1.3.

**Teams.**
- Team roles count for each member (`crates/kernel/src/rules.rs:150-163`). Team operations are `define_team` (`crates/kernel/src/transition.rs:1304`), `team_role` (`:1323`) and `team_member` (`:1372`). Team membership and team changes are written to the change audit (`crates/backend/src/change_audit.rs:1-30`).
- The team ops are gated by `Manage` on `Members` (`crates/kernel/src/transition.rs:1305`, `:1329`, `:1378`).
- Team membership and role changes are recorded in the change audit, with the member's name sealed under that person's key (`crates/backend/src/change_audit.rs:28-30`, `spec:PRV-359`).

**Step-up (recent authentication).**
- A sensitive operation is refused when the last authentication is older than the step-up age (15 minutes, `crates/backend/src/app.rs:133`, decision D-700 and `spec:IDN-019`) or when the session has moved to another network (`spec:IDN-260`). Refusal is `crates/backend/src/signin/stepup.rs:56-76` `require_fresh`, which also logs a decision.
- `confirm` moves the authentication time to now, using the password, the authenticator code, or a passkey assertion (`crates/backend/src/signin/stepup.rs:85`; endpoint `crates/backend/src/http.rs:421-430`).
- Call sites in the backend: `crates/backend/src/http.rs:350, 570, 723, 848, 861, 882, 895, 941, 975`; `crates/backend/src/products.rs:336-338` (product operations, owner only, not for dry runs); `crates/backend/src/erasure.rs:587`; `crates/backend/src/dev.rs:66`; `crates/backend/src/signin/oidc/mod.rs:136`; `crates/backend/src/signin/passkey.rs:303`.
- The BFF members route asks for step-up on `assign_role`, `add_team_member`, `invite` and `revoke_invitation` (`crates/bff/src/routes.rs:1856-1871`). The contract `assign-role` states `stepUp: "15 min"` (`specifications/boundary.jsonc:979`), as does `add-team-member`. The contracts for `revoke-role`, `remove-team-member`, `invite` and `revoke-invitation` state no step-up (`specifications/boundary.jsonc`, the entries with those ids).
- The admission branch that skips step-up when there is no session (`crates/backend/src/precheck.rs:210`) cannot be reached with the current `identify` (`crates/backend/src/auth.rs:215-283` returns only session identities or anonymous, and anonymous callers are refused first).
- Exegra: `claim:m-stepup-age-refused`, `claim:m-stepup-network-refused`, `claim:m-stepup-accepts-valid`, all verified. Test: `crates/bff/tests/conformance/fixtures/generated/contracts.rs:503` (`violates_step_up`).

**Sessions and revocation.**
- A session secret is looked up by its hash (`crates/backend/src/auth.rs:215-283`, token hash at `:233`; `spec:IDN-010`).
- Lifetimes: absolute 12 hours by default (`crates/backend/src/app.rs:31`, `DEFAULT_SESSION_TTL_MS`, matching `decision:D-700`), idle 2 hours (`app.rs:35`). The comment at `app.rs:27-30` says two different absolute values appear in the specification (open question, section 5).
- Each request looks the session up in the store (`auth.rs:215-283`), so a revoked session is refused on the next request.
- Revocation is committed in the same transaction as the change that causes it. The store ends the sessions of each principal listed in the commit's `revoke_sessions`: `crates/storage-sqlite/src/lib.rs:2456-2461` and `crates/storage-postgres/src/lib.rs:2132`. The list is built in `crates/storage-api/src/lib.rs:363-370` from `Effect::RevokeSessions` and from every `MemberChanged` event. The kernel emits the effect when a person's roles become empty (`crates/kernel/src/transition.rs:1201-1204` and `:1400-1402`).
- A person ends their own session (`crates/backend/src/auth.rs:518-528`, `revoke_own_session`, which refuses any other person's session). Deactivation through SCIM ends every live session of the person (`crates/backend/src/service/scim.rs:356-368`, `:396-420`).
- Tests: `crates/backend/tests/service.rs::a_session_ends_at_the_shorter_of_its_idle_and_absolute_lifetimes` (`:1228`), `::revoking_a_session_closes_the_sockets_it_has_open` (`:1273`), `crates/backend/tests/scim.rs::deactivating_a_person_revokes_their_sessions` (`:296`).
- Exegra, all verified: `claim:m-session-idle-ends`, `claim:m-session-absolute-ends`, `claim:m-revoked-session-refused`, `claim:m-owner-revokes-own-session`, `claim:m-owner-revokes-only-chosen-session`, `claim:m-deactivation-revokes-sessions`, `claim:m-password-change-revokes-other-sessions`, `claim:m-recovery-revokes-sessions`, `claim:m-session-withheld-until-second-factor`.
- Spec: `spec:IDN-012`, `spec:IDN-016`, `spec:IDN-024`. Spec `spec:IDN-014` sets a five-second bound across gateway instances; the per-request store lookup meets it by construction, and nothing measures it.

**Delegation and agents acting for a person.**
- A person grants an agent a delegation (`crates/backend/src/service/tokens.rs:363` `create_grant`). Each scope must be one the person may use now: `person_may` at `:379` ("You may not delegate"). The grant lasts at most 30 days (`tokens.rs:38` `GRANT_MAX_MS`, checked at `:393-396`).
- The agent exchanges the grant for a token (`crates/backend/src/service/tokens.rs:280-340`, grant token-exchange). Each step rechecks the grant is live (`:290`), that the person still holds each scope (`person_may` at `:299`), and that the token does not outlive the grant (`:320-335`).
- Verification rechecks each scope against the person's present permissions (`crates/backend/src/service/tokens.rs:534-538`, `person_may` at `:538`) and the grant's liveness (`:534`).
- A token names the person as `sub` and the agent in `act.sub` (`crates/backend/src/service/tokens.rs:48-52`, struct `Act`). Its lifetime is five minutes at most (`tokens.rs:36`, `TOKEN_TTL_MS`; `spec:TOK-005`).
- Specs: `spec:TOK-003`, `spec:TOK-004`, `spec:TOK-006`, `spec:TOK-007`, `spec:TOK-250`.
- Tests: `crates/backend/tests/tokens.rs::revoking_a_grant_refuses_the_tokens_made_from_it` (`:314`), plus `Verifies:` tags at `tokens.rs:214, 270, 271, 309, 387, 455, 502, 560, 589, 653`.
- Exegra, all verified: `claim:m-tokact-accepts-valid`, `claim:m-tokact-agent-refused`, `claim:m-tokact-person-refused`, `claim:m-tokiss-grant-refused`, `claim:m-revoked-grant-token-refused`, `claim:m-live-grant-token-accepted`, `claim:m-token-value-not-shared`.
- Delegated scopes are object scopes only, `action:object` (`crates/backend/src/service/tokens.rs:124-143`). No capability can be delegated (section 1.3, `F-capability-scoped-tokens`).

**Acting-as for people.** A person cannot act as another person. The kernel's `Context` carries one principal (open task WG-5499 records this; I did not find a second principal in the kernel `Context`). Only agents act for people, through the grants above.

**SCIM and identity providers.**
- The provider presents a provisioning token the owner made. It is kept as a hash, belongs to one tenant, and opens only the SCIM endpoint (`crates/backend/src/service/scim.rs:1-14`, `spec:FED-259`). Expiry is optional (`crates/backend/src/service/scim.rs:53-58`, `expires_at: Option<i64>`).
- Operations: create, replace, patch, delete, filter users and change group members (`scim.rs:1-14`, `spec:FED-014`). Deactivating or deleting a person ends their sessions and tokens (`scim.rs:356-368`, `spec:FED-015`).
- A group mapped to a role grants that role through `set_group_role` (`crates/backend/src/service/scim.rs:208-226`). The owner role and the public role cannot be mapped (`scim.rs:221-224`, `spec:FED-260`). A user resource carries no role.
- Every SCIM role change runs as `owner_context` (`crates/backend/src/service/scim.rs:284` `give_role` and `:307` `take_role`, using `crates/backend/src/service/mod.rs:113-135`). See `F-provider-change-attribution`.
- Scenario: `specifications/behaviour.jsonc:9253` (`ui-scim-provisioning`).

**Service accounts and tokens.**
- Only the owner creates, rotates, lists or revokes a service account (`crates/backend/src/service/clients.rs:57-58`, `:105-106`, `:113-114`, `:146-147`, `require_owner` at `crates/backend/src/service/mod.rs:70-84`).
- A service account has at most 32 scopes, each a valid tenant scope (`crates/backend/src/service/clients.rs:24-25`, `:69-76`).
- The client-credentials grant issues only scopes the client holds (`crates/backend/src/service/tokens.rs:255-276`, `spec:TOK-001`, `spec:TOK-002`). The audience must be this site (`tokens.rs:257-262`).
- The client secret is stored only as a hash and shown once (`spec:TOK-252`, test `crates/backend/tests/tokens.rs:502`). The token endpoint limits credential requests to thirty a minute per client (`spec:TOK-253`, test `tokens.rs:560`; decision D-700).
- Tokens live five minutes at most (`spec:TOK-005`). A signing key that is compromised is retired at once (`spec:TOK-251`, test `tokens.rs:455`).

**Separation of duties (approvals).**
- The person whose action started a workflow run may not approve its request step (`crates/kernel/src/approvals.rs:14-15` and `:116`; `spec:WFL-2681`). The check applies to the owner too: the owner bypass at `:109` precedes it, and the separation check still refuses (`:116`).
- An automation principal never decides an approval (`crates/kernel/src/approvals.rs:99-101`). Deciding needs `workflow-approve` and a role the policy names, unless the holder is the owner (`approvals.rs:102-112`).
- Test: `crates/kernel/tests/approvals.rs:121` (`a_run_that_no_person_started_has_no_one_to_separate`); `crates/backend/tests/approvals.rs:792` (`a_failed_step_undoes_the_completed_steps_in_reverse_order`).
- Exegra, all verified: `claim:m-unpermitted-approval-continues-nothing`, `claim:m-permitted-approval-continues`, `claim:m-continued-run-has-permitted-approval`.

**Approvals (workflows).** The approval decision is pure: it takes the tenant, the approver, the step's roles, the starter and the deadline, and returns the approver or a refusal (`crates/kernel/src/approvals.rs:90-125`). It refuses a non-person, a missing capability, an unnamed role, the starter, and a passed deadline (`approvals.rs:40-46`, `:62-70`).

**Authorization changes in the audit.**
- `crates/backend/src/change_audit.rs:1-30` lists what `ops::run_intent_at` records for each committed change: member roles (`member.roles_changed`), team membership (`team.membership_changed`), team (`team.changed`), invitation (`invitation.changed`, without the address), setting, and schema (`schema.changed`). Each entry has the actor, the previous and new values, the revision and a correlation id.
- The audit log is a hash chain. Each line carries the hash of the line before it and of itself (`crates/storage-api/src/audit_chain.rs:1-12`, `:30` `audit_link`, `:58` `verify_audit_chain`). The table is `crates/storage-sqlite/src/lib.rs:447`. A verify endpoint needs `audit-read` (`crates/backend/src/audit_log.rs:1-30`, `AUDIT_READ`).
- Entries about a person are sealed under that person's key, so erasure keeps the chain whole (`crates/backend/src/change_audit.rs:28-30`, `spec:PRV-359`; `crates/storage-api/src/audit_chain.rs:1-8`).
- Tests: `crates/backend/tests/audit_log.rs:182-183` (tags `OPS-329`, `OPS-3120`); `crates/backend/tests/sealing.rs::the_audit_chain_verifies_after_an_erasure_and_the_subjects_lines_are_sealed` (`:389`); `crates/spec-compiler/tests/audit_refusals.rs:106` (tag `ADM-1080`).
- Exegra, all verified: `claim:m-audit-chain-survives-erasure`, `claim:m-platform-audit-store-apart`.
- What is not in the chain: session ends (section 1.3, `F-session-end-audit`) and the removal of the newest lines (`F-audit-chain-head-anchor`).

**What an administrator can and cannot do to themselves.**
- Cannot confer a role whose permissions or policy powers they lack (`spec:ADM-152`, `crates/kernel/src/rules.rs:312-345`).
- Cannot confer the owner role unless they are an owner (`crates/kernel/src/rules.rs:312-318`).
- Cannot revoke a role or leave a tenant without an owner (`spec:ADM-1080`, `spec:ADM-1081`).
- Cannot approve a workflow run they started (`spec:WFL-2681`, `crates/kernel/src/approvals.rs:116`).
- Cannot make an operation grant more than its definer holds (`spec:D-6547`, `crates/kernel/src/transition.rs:2580`).
- Cannot make a schema change without `Manage` on `Schema` (`crates/kernel/src/transition.rs:2233`).
- The gaps are in section 1.3, mainly `F-role-definition-authority`.

**Lean.** The Lean authorization theorems cover the abstract record model, in which authorization is ownership, with one owner (`lean/Strata/Model.lean:439-446` `authorized`; decision D20).
- `lean:step_ok_authorized` (`lean/Strata/Proofs.lean:65`): every accepted step was made by an authorized principal.
- `lean:unauthorized_cannot_update` (`:106`), `lean:unauthorized_cannot_delete` (`:113`), `lean:unauthorized_cannot_mutate` (`:141`), `lean:missing_record_hidden_on_update` and `missing_record_hidden_on_delete`.
- No Lean theorem covers roles, teams, conferral, policies beyond ownership, step-up, sessions, tokens, SCIM, separation of duties or approvals.
- UI visibility model (screen visibility, not server authorization): `lean:admin_needs_role` (`lean/Strata/UI/Visibility.lean:406`), `lean:admin_never_to_default_member` (`:414`), `lean:admin_action_for_members_refused` (`:438`), `lean:admin_default_role_refused` (`:444`), `lean:signed_in_decides` (`:892`).
- Client cache model (browser): `lean:signout_erases`, `lean:revoke_device_erases`, `lean:revoke_all_erases`, `lean:unlock_needs_own_session` (`lean/Strata/Client/Privacy.lean`).

### 1.2 Proof and test coverage, by topic

| Topic | Tests (file::name) | Exegra claims (verified) | Lean |
|---|---|---|---|
| Roles, effective permissions | `publication_template.rs` (tags ADM-150, 151) | none | none |
| Assign-role, conferral | `kernel/tests/behaviour.rs::assigning_a_role...` and `::a_role_empowered_by_a_policy...` | none (no `m-` claim models ADM ids) | none |
| Revoke, owner invariant | `behaviour.rs::revoking_a_role_needs_the_authority...`, `::no_sequence_of_revocations...` | none | none |
| Invitations | `behaviour.rs::an_invitation_expires`, `::an_invitation_can_be_revoked` | none | none |
| Step-up | `bff/tests/conformance/...::violates_step_up`; `backend/tests/factors.rs:228`; `oidc.rs:280` | `m-stepup-*` (3) | none |
| Sessions | `backend/tests/service.rs:1228, 1273`; `scim.rs:296` | `m-session-*`, `m-revoked-session-refused`, `m-owner-revokes-*`, `m-deactivation-revokes-sessions` | `signout_erases` (client) |
| Delegation and tokens | `backend/tests/tokens.rs` (Verifies tags, `:314`) | `m-tokact-*`, `m-tokiss-grant-refused`, `m-revoked-grant-token-refused`, `m-live-grant-token-accepted` | none |
| Approvals, SoD | `kernel/tests/approvals.rs:121`; `backend/tests/approvals.rs:792` | `m-*-approval-*` (3) | none |
| Audit chain | `backend/tests/audit_log.rs`, `sealing.rs:389` | `m-audit-chain-survives-erasure`, `m-platform-audit-store-apart` | none |
| Ownership model | none (Lean only) | none | `step_ok_authorized` and the `unauthorized_cannot_*` family |

Notes: the SCIM last-owner path has no test (WG-5486). The identity-admin refusal for a visitor has a browser journey (`specifications/behaviour.jsonc:9252`, `ui-identity-admin-refuses-visitor`), which the gate does not run (section 3).

### 1.3 Not guaranteed (gaps, Area 1)

**F-role-definition-authority** (security). Editing a role's permissions, defining a role, or adding or removing a policy requires only `Manage` on `Schema` (`crates/kernel/src/transition.rs:2233`). The definition-authority check covers automations, settings, ledger, products and operations (`transition.rs:2519-2556`), but not `DefineRole` (`:1799`), `DefineRolePermissions` (`:1806`), `DefinePolicy` or `RemovePolicy` (`:1813-1819`). Conferral (`may_confer`, `rules.rs:312`) is checked only when a role is handed out. So a principal holding `Manage` on `Schema` who also holds role `Y` can add a policy or permissions to `Y` and gain them at once, without holding them first. Spec: `spec:ADM-152`, `spec:ADM-1085`, `spec:AGT-253`. Test: none refuses a non-owner's role redefinition. Existing task: none (WG-5499's scoped admin is adjacent and does not cover this).

**F-identity-admin-capabilities** (security). Creating, rotating or revoking service accounts, provisioning tokens, group-to-role mappings, signing keys and other people's sessions is gated by `require_owner` (`crates/backend/src/service/mod.rs:70-84`; `clients.rs:58, 106, 114, 147`; `scim.rs:98, 174, 209, 248, 272`; `tokens.rs:429`, `:599`). Owner status is not a capability, so a tenant cannot give identity administration to anyone but the owner. Spec: `spec:ADM-150`, `spec:AGT-253`, `spec:SEC-1208` (capability session-end, not wired). Existing task: WG-5499 (scoped admin), WG-5490 (administrator ends others' sessions).

**F-service-account-role-bounds** (security). A service account's scopes are any valid tenant scopes the owner chooses (`crates/backend/src/service/clients.rs:69-76`). They are not bounded by a role or by the creator's own powers, and a service account is not a principal in the role model. Spec: `spec:TOK-001`, `spec:TOK-002`. Existing task: none.

**F-last-owner-on-every-path** (security, defect). SCIM `set_active` disables a person and ends their sessions without the owner-count check (`crates/backend/src/service/scim.rs:396-420`). The owner-count check is only in `crates/kernel/src/transition.rs:1258` (`require_an_owner_remains`). SCIM delete also runs before its refusal (WG-5486). Spec: `spec:ADM-1081`. Existing task: WG-5486.

**F-local-account-suspension** (security). Local persons cannot be suspended. `PrincipalState` is in the model and is not enforced (WG-5490). `revoke_own_session` ends only the caller's own sessions (`crates/backend/src/auth.rs:518-528`). A person whose roles are all revoked still holds `public` (`crates/kernel/src/rules.rs:167-184`, `roles.insert(public_role())`) and can still sign in. Spec: `spec:IDN-012`, `spec:SEC-1208`. Existing task: WG-5490.

**F-session-end-audit** (security). A session ended by its owner, by a revocation commit, or by a grant revocation is written to telemetry only (`crates/backend/src/auth.rs:449-468`, `log_end` calls `telemetry::session_ended`). It is not an audit line, since `app.audit` persists only when the fields name a tenant (`crates/backend/src/app.rs:368-380`), and the session revocation itself is not in `change_audit.rs`. Spec: `spec:AGT-015`, `spec:SEC-1207`. Existing task: WG-5490 (each administrator action in the change audit).

**F-audit-chain-head-anchor** (security). The head of the audit chain is not anchored outside the tenant database, so removal of the newest lines leaves a shorter chain that still verifies (`crates/backend/src/audit_log.rs:1-30`, `crates/storage-api/src/audit_chain.rs:10-12`, decision D-6208). Spec: `spec:CT-128`, `spec:PRV-359`. Existing task: WG-5506 (anchored audit).

**F-provider-change-attribution** (security). SCIM and realm role changes are committed as the first owner found in the member table (`crates/backend/src/service/mod.rs:113-135`, `owner_context`, which picks the first owner from `state.members`). The change audit therefore names that owner as the actor. The provider and its provisioning token are not recorded as the actor. The kernel's conferral check also passes, because the acting owner holds every permission. Spec: `spec:SEC-1207` (the audit log derives the actor from the calling credential). Existing task: none.

**F-declared-sensitive-operations** (security). Step-up is hand-placed in about nine backend handlers (`crates/backend/src/http.rs:350-975`) and in the BFF members route (`crates/bff/src/routes.rs:1856-1871`). It is not declared per operation in the contract format, which decision D-4915 says it should be once the format states it. Revoking a role and removing a team member carry no step-up in the BFF or the contract (`specifications/boundary.jsonc`). Spec: `spec:IDN-019`, `spec:IDN-260`, `decision:D-4915`. Existing task: none.

**F-separation-of-duties-policy** (security). Separation of duties exists only for workflow approvals (`crates/kernel/src/approvals.rs:116`). There is no declared rule for other operations or role changes, such as an assigner who may not assign to themselves beyond what they already hold. Spec: `spec:WFL-2681`, `spec:AGS-409` (named in `approvals.rs:15`, not implemented as a proposal rule). Existing task: none.

**F-conditional-approval-decision** (security). The decision on a waiting approval reads the run and then writes it (WG-5488). Whether the store makes the write conditional on the read state was not verified. Spec: `spec:WFL-2683`. Existing task: WG-5488 (and WG-5427).

**F-proposal-gated-changes** (security). Changes classed as risky by a tenant policy may be applied only from an approved proposal (`spec:AGT-011`), with the proposal carrying the projection against the tenant's recorded history (`spec:AGT-012`) and refused when the revision has moved (`spec:AGT-013`). No proposal mechanism exists in the kernel or the backend. The Impact Lab applies a projection only at its revision (`crates/backend/src/lab_apply.rs`, test `crates/backend/tests/lab_revision.rs:240`, tag `AGT-2940`), which is a partial match. Existing task: none.

**F-time-bound-access** (security). Role grants cannot carry an end, there are no assign-role or revoke-role steps as automation, and there is no access-request flow, person-to-person delegation with scope and window, break-glass or support grant (`spec:ADM-163` to `spec:ADM-165`, `spec:PRV-1210`). Support grants are specified and not built. Existing task: WG-5499.

**F-role-composition** (security). There is no role implication, no conflicting-role rule, no runtime team creation or deletion, no principal attributes, and members, teams and invitations live in the schema document (`crates/kernel/src/rules.rs:150-163`; `transition.rs:1304` `define_team` only). Existing task: WG-5500.

**F-platform-admin-role** (security). A platform tenant-admin role that every product can name does not exist (`crates/kernel/src/settings.rs:403-404`). Products name per-product admin roles. Existing task: WG-5474.

**F-sandbox-tenant-lifecycle** (security). A person cannot start a sandbox tenant from a template and keep it, sandboxes do not expire, and staging promotion through proposals is not built (`spec:ADM-154` to `spec:ADM-160`, `spec:ADM-158`). The only staging concept is a reserved product role suffix (`crates/kernel/src/products.rs:331-333`). Existing task: none.

**F-tenant-export-archive** (security). A tenant owner cannot export the tenant's records, files, configuration and audit entries as a manifest-verified archive (`spec:ADM-161`, `spec:ADM-162`). Only a per-person export exists (`crates/backend/src/subject_export.rs`). Existing task: none.

**F-capability-scoped-tokens** (security). Delegated and client tokens carry object scopes only, `action:object` (`crates/backend/src/service/tokens.rs:124-143`). A capability such as `members-manage` cannot be delegated to an agent. Spec: `spec:TOK-004`, `spec:AGT-014`. Existing task: none.

---

## 2. Area 2: the ledger

### 2.1 Guaranteed

**Units, pools, accounts.**
- An account is identified by one owner (a principal of the tenant, or a named pool), one declared unit and one scope. The account id is derived from tenant, owner, unit and scope, so a decision names the account before it loads anything (`crates/kernel/src/ledger.rs:78-103` `account_id`; decision D-5001, D-6551). Spec `spec:LDG-001`.
- A posting that names an undeclared unit, owner or pool is refused and changes nothing (`crates/kernel/src/ledger.rs:503-534`, rules `ledger:unknown-unit` and `ledger:unknown-owner`; `spec:LDG-002`, `spec:LDG-004`).
- A zero or non-whole amount is refused (`crates/kernel/src/ledger.rs:536-542`, rule `ledger:invalid-amount`; `spec:LDG-003`).
- A unit or pool that an account names cannot be removed (`crates/kernel/src/ledger.rs:1525-1558`, `ledger:in-use`; `spec:LDG-1549`). A tenant holds at most 64 units, 64 pools and 10,000 account limits (`crates/kernel/src/limits.rs:91-95`, enforced at `crates/kernel/src/ledger.rs:1570-1606`; `spec:LDG-1550`, `spec:LDG-2893`).

**PostLedger.**
- `post_after` (`crates/kernel/src/ledger.rs:487`, `post` at `:476`) decides a posting from the snapshot, the request, the context and the time the context gives (`spec:LDG-008`, decision D-5010). It is called by the direct intent `PostLedger` after `ledger-post` is checked (`crates/kernel/src/ledger.rs:902-930` `decide_intent`; the capability check at `:915-921`), and by tenant operations and automation steps with the authority of their definition (`spec:LDG-008`, decision D-6547).
- The backend route requires `ledger-post` (`crates/backend/src/precheck.rs:52`).
- Each entry records amount, idempotency key, reason, causing principal and the time given (`spec:LDG-005`). An entry carries one event, `ledger-entry-posted` (`spec:LDG-010`; `specifications/behaviour.jsonc:12877`, the event body).

**ReverseLedger (reversal by reference).**
- A reversal is a new entry that names the entry it corrects, posts the negated amount, and is keyed by the reference of the posting (`spec:LDG-023`, `spec:LDG-2900`). `reverse_after` is at `crates/kernel/src/ledger.rs:360-447`.
- A reversal of an absent posting is refused as `ledger:no-such-posting` (`ledger.rs:404-412`, `spec:LDG-2901`). A posting already reversed is refused as `ledger:already-reversed` (`ledger.rs:417-437`).
- A reversal under a cap that counts additions only frees no capacity and is not counted (`crates/kernel/src/ledger.rs:741`, `counts = amount > 0 && !(additions_only && corrects.is_some())`; `spec:LDG-2902`).

**Idempotency keys.**
- An idempotency key is per account (`crates/kernel/src/ledger.rs:611-636`, `e.account == account && e.idempotency_key == key`). A repeat with the same amount, reason and correction answers with the first entry and changes the balance once (`ledger.rs:611-636`, `spec:LDG-006`). A repeat with another amount, reason or correction is refused as `ledger:idempotency-mismatch` (`ledger.rs:654`, `VersionConflict`; `spec:LDG-007`).
- A key that is too long, empty, or begins with `expiry:` on an ordinary posting is refused (`ledger.rs:547-563`, `spec:LDG-003`; `spec:LDG-2897`).
- The worker answers a retried step with the first entry (`crates/backend/src/worker/steps.rs:1014`, `LDG-006`).

**Caps and limits.**
- Floor and cap per account and per unit, with the account limit replacing the unit limit field by field (`crates/kernel/src/ledger.rs:232-310` `Limits`, `limits_of`; `spec:LDG-011`, `spec:LDG-012`, `spec:LDG-1542`). A posting that leaves a balance below its floor or above its cap is refused, unless it moves a balance already outside the limit back toward it (`ledger.rs:714-735`, rules `ledger:below-floor`, `ledger:above-cap`; `spec:LDG-013`).
- Cap per period in the UTC hour, day, week or month containing the entry (`ledger.rs:197-215` `period_start`; `ledger.rs:740-765`, rule `ledger:above-period-cap`; `spec:LDG-014`, `spec:LDG-015`).
- A limit change applies to later postings and changes no entry or balance (`spec:LDG-016`, `spec:LDG-2892`).
- `additions-only` is accepted only with a cap per period (`crates/kernel/src/ledger.rs:1461-1463`, `WG-5598`).

**Balance.**
- The balance is a stored header value, `LedgerAccount.balance`, updated with each entry (`crates/kernel/src/ledger.rs:802-850` sets `balance: LedgerAmount(after)`; `apply` at `:982` writes the delta). Spec `spec:LDG-020` requires it to equal the sum of entries.
- Nothing sets a balance directly. Operations and the platform cannot set it (`spec:LDG-021`); the balance column is written only by the kernel's delta (`crates/storage-sqlite/src/ledger.rs:433-470`, the entry insert, and `:378` the account insert).
- The balance read counts only entries the reader's policies permit (`crates/kernel/src/ledger.rs:1761-1800` `read_balance`, `may_read` at `:1682`; `spec:LDG-025`).
- Replay verification is available: `replayed_balance` (`crates/kernel/src/ledger.rs:2141-2158`) compares the stored balance with the sum of entries and names the first missing sequence number (`crates/backend/src/ledger.rs:469-476`, operation `verify`, decision `CT-287`). It is an operator operation, not a schedule (section 2.3).

**Append-only storage.**
- SQLite: `ledger_entries` refuses DELETE always, and UPDATE unless the only change is `caused_by` set to NULL (`crates/storage-sqlite/src/ledger.rs:63-76`, triggers `ledger_entries_append_only_delete` and `ledger_entries_append_only_update`). Spec `spec:LDG-022`.
- PostgreSQL: the same rule as a plpgsql trigger function (`crates/storage-postgres/src/ledger.rs:51-71`, `ledger_entries_append_only`).
- The only sanctioned UPDATE is erasure: `caused_by` set to NULL, and an account's owner set to none (`crates/storage-sqlite/src/ledger.rs:489-520` `anonymise`; PostgreSQL mirror `crates/storage-postgres/src/ledger.rs:519-527`). Erasure keeps every amount, key, reason, number, time and balance (`spec:LDG-027`, `spec:LDG-028`).
- Ledger limit rows can be deleted when their owner is erased (`crates/storage-sqlite/src/ledger.rs:517`, `DELETE FROM ledger_limits`).

**Atomicity (one commit).**
- A posting is committed with the change that causes it. Both stores apply `DeltaOp::Ledger` inside the commit transaction: SQLite `crates/storage-sqlite/src/lib.rs:2407`, PostgreSQL `crates/storage-postgres/src/lib.rs:2070`. A rolled-back change leaves no entry (`spec:LDG-009`, `spec:LDG-029`).
- A transfer is two postings committed together or not at all (decision D-5013; `crates/kernel/src/ledger.rs:23`). Test `crates/kernel/tests/ledger.rs::a_transfer_is_two_postings_committed_together_or_not_at_all`.
- The sequence number must continue the account. The store refuses an entry that does not follow the header's last sequence in the same transaction (`crates/storage-sqlite/src/ledger.rs:433-453`).
- A tenant restore is refused when the ledger holds entries newer than the snapshot, because entries are never removed (`crates/storage-sqlite/src/ledger.rs:472-487` `check_restorable`, called from `crates/storage-sqlite/src/lib.rs:5143`).

**Reads and visibility.**
- `read_balance`, `read_entries`, `read_ranked` and `read_definitions` answer from a snapshot their `*_needs` function names (`crates/kernel/src/ledger.rs:1747` `balance_needs`, `:1828` `entries_needs`, `:1921` `ranked_needs`).
- `ledger-read` lists and reads any account; `ledger-read-own` reads only accounts the person owns (`crates/kernel/src/ledger.rs:1701-1719`; `spec:LDG-026`, `spec:LDG-1541`, `spec:LDG-2889`). A read the reader may not make is answered as an account that does not exist (`spec:LDG-2881`).
- A tenant ledger policy bounds reads by the owner, pool, unit, scope, amount, reason and caused-by of each account (`crates/kernel/src/ledger_policy.rs:45`, `:80` `policy_problems`; `spec:LDG-1540`, `spec:LDG-2888`).
- Ranked listings return at most 100 accounts a page and examine at most 10,000 accounts, refused with `scan-bound-exceeded` beyond that (`crates/kernel/src/ledger.rs:1807-1808`, `spec:LDG-1543`, `spec:LDG-2890`, `spec:LDG-2891`).

**Expiry of credits.**
- A unit may declare an expiry of one to thirty-six periods (`crates/kernel/src/ledger_expiry.rs:1-25`). A debit consumes the oldest bucket first. The expiry posts one entry per account with the key `expiry:<bucket start>` (`crates/kernel/src/ledger_expiry.rs:216` `decide`; `spec:LDG-1545`, `spec:LDG-1546`). At most 100 accounts per transaction (`ledger_expiry.rs:42` `EXPIRY_BATCH`; `spec:LDG-1547`). At most 36 buckets per account, the oldest merged into the newer (`ledger_expiry.rs:44` `MAX_BUCKETS`; `spec:LDG-2883`, `spec:LDG-2898`).
- The job is created when a unit with an expiry is declared and creates the next one in the transaction (`spec:LDG-2884`; `crates/backend/src/ledger.rs:68`, `:577`).

**Concurrency.**
- SQLite runs one connection, so commits are serialised by construction (decision D11, `docs/decisions.md`).
- PostgreSQL serialises the writers of a tenant with a transaction-level advisory lock (`crates/storage-postgres/src/lib.rs:1292`) and locks the tenant row (`lib.rs:2629`). Isolation is READ COMMITTED (`lib.rs:2152`).
- A posting that loses a commit race is decided again on the new state (`crates/backend/src/ledger.rs:6-8`, decision D-5009). A posting that still fits is applied (`spec:LDG-018`); of two concurrent postings that together pass a cap, exactly one is refused (`spec:LDG-017`).
- Test `crates/kernel/tests/ledger.rs::two_concurrent_postings_cannot_both_pass_a_cap` (tags `LDG-017, 018`, `ledger.rs:445`).
- Exegra, all verified: `claim:m-ledger-race-never-passes-cap`, `claim:m-ledger-race-one-is-refused`, `claim:m-ledger-race-exactly-one-posted`, `claim:m-ledger-race-loser-that-fits-is-applied`.

**Observability.**
- Each entry emits one domain event after commit, through the outbox (`spec:LDG-010`, `spec:RUN-005`, decision D12). Socket notices reach only subscribers whom the read decision lets read the account, and carry only the account, unit, scope and entry number (`crates/kernel/src/ledger.rs:2242` `may_hear`; `spec:LDG-2887`, `spec:LDG-2895`).
- Operator read APIs: `balance`, `entries`, `accounts`, `definitions`, `verify`, `expire` (`crates/backend/src/ledger.rs:1-30` table).

**Tests and claims.**
- Kernel tests carry the tags `LDG-005` to `LDG-020`, `LDG-1540` to `LDG-1550` and `LDG-2900` to `2902` (`crates/kernel/tests/ledger.rs:157, 203, 227, 365, 416, 445, 500, 769`, and others). Test names include `the_balance_always_equals_the_sum_of_the_entries`, `erasure_clears_the_principal_and_keeps_every_amount_and_balance`, `a_reversal_under_a_cap_that_counts_additions_only_neither_counts_nor_frees`, `one_transaction_of_expiry_handles_at_most_one_hundred_accounts`, `credits_debits_and_expiries_keep_the_balance_the_sum_of_the_entries`.
- Exegra, all verified (16): `claim:m-ledger-posting-once-per-key`, `m-ledger-answers-repeated-key`, `m-ledger-below-floor-refused`, `m-ledger-over-cap-refused`, `m-ledger-balance-within-limits`, `m-ledger-balance-not-writable`, `m-ledger-replay-equals-balance`, `m-ledger-posting-with-its-change`, `m-ledger-rolled-back-change-leaves-no-entry`, `m-ledger-committed-change-posts`, `m-ledger-one-event-per-entry`, `m-ledger-race-*` (4 listed above).
- Lean: no theorem about the ledger exists in `lean/Strata` (grep for ledger, balance and posting returns none).

### 2.2 Ledger coverage by requirement

Of the 63 `LDG` requirements, 40 carry a `Verifies:` tag. Untagged: `LDG-021`, `LDG-025`, `LDG-029`, and `LDG-2880` to `LDG-2899`. The untagged ids are checked by Exegra claims only where a claim models them (`claim:m-ledger-balance-not-writable` models `LDG-021`). None of the `LDG` requirements is in the started milestone (milestone 11 for all 63).

### 2.3 Not guaranteed (gaps, Area 2)

**F-ledger-tamper-evidence** (security). Ledger entries have no hash chain. They are protected by database triggers only (`crates/storage-sqlite/src/ledger.rs:63-76`, `crates/storage-postgres/src/ledger.rs:51-71`), and a database administrator can drop or alter them. The commit journal is unchained (`crates/storage-sqlite/src/lib.rs:230-233`, columns `revision` and `data`). A point-in-time restore deletes journal rows past a revision (`crates/storage-sqlite/src/lib.rs:5177`, `DELETE FROM journal`, guarded for ledger entries only by `check_restorable`, `ledger.rs:472-487`). Spec: `spec:LDG-022`, `spec:OPS-329`, `spec:PRV-359`, `spec:CT-287`. Existing task: WG-5506 covers the audit chain only.

**F-ledger-posting-audit** (security). A posting is not written to the audit log. `change_audit.rs` records roles, teams, invitations, settings and schema, and has no ledger case (`crates/backend/src/change_audit.rs:1-30`). A posting's only record is its entry, its outbox event, and the commit journal. Spec: `spec:LDG-005`, `spec:LDG-010`. Existing task: none.

**F-scheduled-reconciliation** (security). Balance replay is an operator endpoint (`crates/backend/src/ledger.rs:469-476`). Nothing runs it on a schedule or raises an alarm when the stored balance and the replayed sum differ. Spec: `spec:LDG-019`, `spec:LDG-020`, `spec:CT-287`. Existing task: none.

**F-balanced-journal-entries** (security). There is no journal of N postings that commits only if debits equal credits per currency. A transfer is two postings committed together (`crates/kernel/src/ledger.rs:23`), and no invariant checks the pair's balance (grep for `balanced` and `double-entry` finds nothing in the ledger code). Existing task: WG-5504 (blocked by WG-5494).

**F-account-hierarchy** (security). Accounts cannot form a chart with parents whose balance is the sum of children (`crates/kernel/src/ledger.rs`, accounts are flat per owner, unit and scope). Existing task: WG-5504.

**F-external-reconciliation** (security). No statement, import or comparison against an external ledger exists. Spec: none. Existing task: none.

**F-ledger-read-access-audit** (security). Reads of account balances and entries are authorised and filtered but not audited, so no access report exists for a record (`crates/backend/src/ledger.rs` read operations write no audit line). Existing task: WG-5506 (per-read audit, object types).

**F-reversal-idempotent-replay** (security, unsure). A repeat of a reversal with the same reversal key is answered `ledger:already-reversed` (`crates/kernel/src/ledger.rs:417-437`), not with the first reversal's entry, unlike a posting (`ledger.rs:611-636`). Whether the store's intent-level idempotency answers a retried reversal first was not traced. Spec: `spec:LDG-006`, `spec:AGT-403`.

**F-ledger-machine-checked-invariants** (security). No Lean theorem covers the ledger (`lean/Strata`). The invariants (balance equals the sum of entries, cap never passed, one event per entry) are checked by Rust tests and by Exegra models, which are links rather than proofs (`specifications/README.md:198-203`).

**Tiers.** The ledger has no tier concept. Plan tiers exist for quotas and entitlements (`crates/backend/src/quota.rs:874` `entitles`), not for ledger limits. Open question, section 5.

---

## 3. Area 3: accessibility of generated UI

### 3.1 What the specification requires

**Decided and gate-checked: nothing.** The design-system accessibility rule, decision D-DS-18 ("International, accessible and responsive by rule"), is marked proposed and not yet decided by the owner (`specifications/design-system.jsonc:23`). The design doc names it as the basis for the system (`docs/design-system.md:61`, `:195`) and states the primitives and their semantics as the target (`docs/design-system.md:200-230`).

**Requirements that exist, all outside the started milestone:**
- `spec:IDN-023`: sign-in pages meet WCAG 2.2 AA (milestone 3; `specifications/requirements.jsonc:94`).
- `spec:CMS-012`: the authoring application meets WCAG 2.2 AA (milestone 11; `:209`).
- `spec:CNV-238`: every control of the conversation operable by keyboard alone (milestone 9; `:406`). `spec:CNV-239`: focus moves to a preview answering the person's message; an unasked preview is announced without moving focus (`:407`). `spec:CNV-241`: motion stops under reduced motion (`:409`). `spec:CNV-248`: text from outside the tenant is labelled with its source (`:416`).
- Intent statements: `specifications/intent.jsonc:1237` (sign-in pages and authoring app meet WCAG 2.2 AA) and `:1267` (the conversation works with keyboard, screen reader, reduced motion, small screen, poor network).
- Design-system requirements DS-001 to DS-023 (`specifications/design-system.jsonc:40-62`) cover composition, contrast (DS-005, DS-006, proved in Lean), visibility and the build loop. None states an accessibility rule for generated screens.
- The design doc's definition-time checks, `ui.a11y.unnamed-control`, `ui.a11y.image-alt` and `ui.a11y.heading-order` (`docs/design-system.md:583-588`), are not in any requirement.

### 3.2 What the generators emit

- Generated form data carries each field's accessible name (`label`), control type and submit label. The generated `web/src/generated/forms.js:20-36` is the example.
- The label defaults to the field's identifier when the product declares none (`crates/spec-compiler/src/web_kernel.rs:1670`, `unwrap_or(&f.name)`). An unlabelled field is named by its identifier, not refused.
- The `specc` attribute check allows only `aria-*`, `data-*`, `value`, `href`, `lang` and `dir` in scenario steps (`crates/spec-compiler/src/e2e.rs:1877`, `:2067`). This is a check on test scripts, not on generated screens.
- The compiler emits Playwright journeys that find controls by role and name (`crates/spec-compiler/src/e2e.rs:4166-4168`, `getByRole`, `getByLabel`), and the axe step `axeScan` (`crates/spec-compiler/src/e2e.rs:330`).
- No `ui.a11y.*` check exists in the Rust crates, the web code, the scripts or the specification (repository-wide grep finds the names only in `docs/design-system.md`).

### 3.3 What the renderer does

**Labels and descriptions.**
- Form fields: `<label for>` (`web/src/views/patterns/form.js:15`), checkbox and radio inside labels (`form.js:41`, `:61`), field description and error linked with `aria-describedby` and `aria-invalid` (`form.js:20-25`, `:32`; `web/src/components/parts.js:110`, `:114`).
- Hand-written pages label every control (`web/src/views/pages.js:184`, `:240`, `:266`, `:307`, `:313`, `:352`), and the shell's search has a visually hidden label (`web/src/views/shell.js:57`).

**Errors and focus.**
- A refused form moves focus to its first failing field (`web/src/kernel/update.js:519-525` and `:908-918`). Focus targets are ids; whether every generic field id exists was not checked in a browser.
- After navigation the kernel focuses `#main` (`web/src/main.js:76-84`; `web/src/views/shell.js:77`, `tabindex="-1"`). Escape closes the masthead menu and returns focus to its toggle (`web/src/main.js:193-197`).
- Step-up asks in a `role="alertdialog"` labelled by its title (`web/src/views/desk.js:94`), and focus moves to the secret field (`web/src/kernel/factors.js:89`). No focus trap was found in `web/src` (grep for `showModal` and `keydown` finds only the masthead handler, `main.js:193`).
- Focus styles: `:focus-visible` in `web/src/styles/base.css:122`, `ds.css:171, 407-409, 538`, `components.css:687`.

**Landmarks and headings.**
- Shell regions: skip link (`web/src/views/shell.js:29`; `base.css:169-183`), header (`shell.js:31`), navigation (`shell.js:46`), main labelled "Content" (`shell.js:76-77`), footer (`shell.js:81`).
- Page header `<header>` with the page `<h1>` (`web/src/views/tenant.js:33`); aside labelled "More on this page" (`tenant.js:38`).
- Hand-written pages have one `h1` each, asserted by the browser suite (`tests/e2e/site.spec.js:698-699`).
- Pattern headings are `h2` (`web/src/views/patterns/form.js:84`, `web/src/views/patterns/stat-tile.js:27`). Heading order is not checked.

**Tables.** Data table has a visually hidden caption and `th scope="col"` and `scope="row"` (`web/src/views/patterns/data-table.js:37-55`).

**Status and live updates.**
- A single polite announcer region (`web/src/views/shell.js:87`, `role="status"`, `aria-live="polite"`) carries the kernel's `announce` text (`web/src/kernel/update.js:228`, `:871`).
- Errors are `role="alert"` (`web/src/views/desk.js:18, 58, 97, 121, 139, 280`; `web/src/views/lab.js:118`). Progress and confirmations are `role="status"` (`desk.js:126, 157`; `lab.js:161`).
- The editor preview is a polite live region (`web/src/views/desk.js:329`).
- Toasts are a `role="region"` labelled "Messages" with no live role (`web/src/views/shell.js:88-100`). A toast's text is not announced by itself.

**Controls and state.** Segmented control with `aria-pressed` (`web/src/views/desk.js:310-311`), toolbar with `aria-controls` (`desk.js:316`), masthead menu with `aria-expanded` and `aria-controls` (`web/src/views/shell.js:39`), `aria-current` on the active navigation link (`web/src/styles/layout.css:57` and the navigation markup), `aria-busy` on regions while loading (`shell.js:77`; `web/src/components/ds.js:141`, `:197`).

**Images.**
- Decorative images carry an empty alt: avatars (`web/src/components/ds.js:55`), covers (`web/src/components/parts.js:98`, `web/src/views/desk.js:365`).
- Tenant images take their alt from the node and default to empty (`web/src/components/ds.js:103`, `node.alt ?? ''`). The part for a block passes `block.alt` through without a check (`web/src/components/parts.js:58`).
- The QR code is `role="img"` with a label (`web/src/components/parts.js:144`).

**Motion and contrast in CSS.**
- Reduced motion: the token layer sets durations to zero under `prefers-reduced-motion: reduce` (`web/src/styles/generated/tokens-fieldnotes.css:277`, `tokens-neutral.css:160`). The skeleton animates only under `no-preference` (`web/src/styles/ds.css:619-622`).
- Forced colours: `web/src/styles/ds.css:1189`.
- Document language is hard-coded: `<html lang="en">` (`crates/bff/src/routes.rs:2407-2409`). No `dir` is set. Strings are literal in the views (for example `web/src/views/pages.js`).

**Primitives in the design doc that are not in the code.** The design names `ds-dialog`, `ds-menu`, `ds-record-picker` (combobox), `ds-tooltip`, `ds-toast-region`, `ds-sortable`, `ds-lanes` and `ds-selection` (`docs/design-system.md:207-230`). No such element exists in `web/src` (grep finds the names only in the design doc). The renderer uses native elements and hand-written views (`web/src/views/desk.js`, `lab.js`, `pages.js`, `patterns/*.js`, `tenant.js`).

**Conversation.** The conversation surface that `spec:CNV-238` to `CNV-248` describe is not in `web/src` (grep for `conversation` finds nothing).

### 3.4 Automated checks: which exist, and where they run

| Check | Where | Runs in the gate? |
|---|---|---|
| axe (WCAG 2.0 A/AA, 2.1 AA, 2.2 AA) on reader, desk, Impact Lab pages | `tests/e2e/site.spec.js:124-128` helper; pages at `:696-700`, `:704-713`, `:810-899` | No. Only with `spec-gate --browser` (`scripts/spec-gate.mjs:42-43`, `:84`, `:563`); `npm run e2e` is not a gate step (`specifications/README.md:156-159`) |
| axe in generated journeys | `tests/e2e/generated/journey.js:457-461`; `ui-desk-pages-accessible.spec.js` (verifies `CMS-012`), `ui-reader-pages-accessible.spec.js` | No. Same as above (`spec-gate.mjs:553-554`) |
| axe step fixtures (a page that passes, one that violates) | `tests/e2e/step-fixtures/spec.jsonc:493-497` (`pass-axeScan`, `violates-axeScan`); `tests/e2e/step-checks/violates-axeScan-...` | Generator check runs in the gate (`spec-gate.mjs:329-332`); the browser run does not |
| Keyboard paths | Playwright `keyboard.press` in `tests/e2e/site.spec.js:184, 259, 361, 587-600`; generated `press` step (`crates/spec-compiler/src/e2e.rs:1703-1706`) | No (browser only) |
| Contrast of catalogue pairs (presets refused below minimum) | `scripts/ui-tokens.mjs:1-14` (refuses a preset with a pair under its ratio); unit tests `scripts/ui-tokens.test.mjs` | The unit test is not matched by the gate's script glob (`scripts/spec-gate.mjs:401`, `^(gate|test)-.*\.test\.mjs$`); `package.json:12` `test:scripts` lists it but the gate does not call that script |
| Style literal check (colours, fonts, pills outside tokens) | `scripts/check-style-literals.mjs:1-10`; `package.json:10` `check:styles` | No. Not referenced in `spec-gate.mjs` |
| Lean contrast | `lean/Strata/UI/Contrast.lean:45` `onColour_meets_aa`, `:59` `search_finds`, `:80` `checkTheme_iff` | Lean builds in the gate. The theorems are about an abstract luminance model, not the CSS values (`specifications/README.md:198-203`) |
| Gallery and screenshot regression, axe at WCAG 2.2 AA over every example, keyboard scripts for every primitive | Open task WG-5473 | Not built |

### 3.5 Not guaranteed (gaps, Area 3)

**F-accessible-by-construction** (accessibility). The accessibility rule for generated screens is proposed, not decided (`specifications/design-system.jsonc:23`), and no requirement states it. Existing tasks: WG-5457 (the design-system epic), WG-5473 (gallery and axe).

**F-definition-time-a11y-checks** (accessibility). The checks the design names (`ui.a11y.unnamed-control`, `ui.a11y.image-alt`, `ui.a11y.heading-order`, `docs/design-system.md:583-588`) are not implemented. A field without a declared label is labelled by its identifier (`crates/spec-compiler/src/web_kernel.rs:1670`). A tenant image without alt is marked decorative (`web/src/components/ds.js:103`). Heading order is not checked. Existing task: none exact; WG-5473 covers runtime axe only.

**F-browser-accessibility-gate** (accessibility). Axe and keyboard checks run only under `spec-gate --browser` (`scripts/spec-gate.mjs:42-43`). The default gate reports them as NOT RUN (`spec-gate.mjs:563`). A change that breaks a label or a focus path passes the default gate. Existing task: WG-5473.

**F-contrast-verification-in-gate** (accessibility). The token contrast tests (`scripts/ui-tokens.test.mjs`) and the style literal check (`scripts/check-style-literals.mjs`) are not run by the gate (`scripts/spec-gate.mjs:401`, `package.json:10`). The Lean contrast proofs concern an abstract model (`lean/Strata/UI/Contrast.lean:45`), so the CSS values are not proved. Existing task: none exact.

**F-accessible-primitive-library** (accessibility). The primitives the design names (dialog with focus trap, menu with roving focus, combobox, tooltip, live toast region, gesture hosts) are not built (`docs/design-system.md:207-230`; no matches in `web/src`). Existing tasks: WG-5457, WG-5473.

**F-request-language-and-direction** (accessibility, internationalisation). `<html lang="en">` is hard-coded (`crates/bff/src/routes.rs:2407-2409`). No direction is set, and strings are literal in the views. D-DS-18 would require logical properties, the request language on the root, and localisable strings (`specifications/design-system.jsonc:23`). Existing task: none.

**F-image-text-alternative** (accessibility). A tenant image's alt defaults to empty (`web/src/components/ds.js:103`), so the image is silently decorative. Existing task: none.

**F-live-status-announcements** (accessibility). One announcer region exists (`web/src/views/shell.js:87`). Toasts are not live (`shell.js:88-100`). A success message shown as a toast (for example, a generated form's `show: 'toast'`, `web/src/generated/forms.js:20-24`) is announced only if the kernel also sets `announce`. Existing task: none.

**F-conversational-interface** (accessibility, feature). The conversation surface is not built, so `spec:CNV-238`, `CNV-239`, `CNV-241` and `CNV-248` have no code (milestone 9). Existing task: none found in the tasks read.

---

## 4. Fundamentals (inventory schema shape)

The array below follows the `fundamental` definition in `docs/plinth/inventory.schema.json`: `id`, `capability`, `why`, `neededBy`, `existingTask`, `security`, `touches`. `neededBy` names the baseline area until the inventory step maps Plinth items.

```json
{
  "fundamentals": [
    {
      "id": "F-role-definition-authority",
      "capability": "Changing what a role grants, or the policies of a tenant, is authorised only for a principal that already holds every power the change grants.",
      "why": "Only Manage on Schema is checked for schema changes (transition.rs:2233). DefineRolePermissions, DefinePolicy and RemovePolicy are applied without a definer check (transition.rs:1799-1819, require_definition_authority at 2519-2556). may_confer (rules.rs:312) runs only when a role is handed out.",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["kernel", "spec"]
    },
    {
      "id": "F-identity-admin-capabilities",
      "capability": "Administration of identity (service accounts, provisioning tokens, group-to-role mapping, signing keys, other people's sessions) can be granted as capabilities to any role, not only to the owner.",
      "why": "All such operations call require_owner (service/mod.rs:70-84; clients.rs:58; scim.rs:98).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5499",
      "security": true,
      "touches": ["backend", "bff", "spec"]
    },
    {
      "id": "F-service-account-role-bounds",
      "capability": "A service account's permissions are bounded by a role and by the creator's own powers.",
      "why": "Client scopes are any valid tenant scopes, up to 32 (clients.rs:69-76); token scopes are object-level only (tokens.rs:124-143).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "kernel"]
    },
    {
      "id": "F-last-owner-on-every-path",
      "capability": "The invariant that a tenant keeps an owner holds on every path that changes membership, including identity-provider paths.",
      "why": "SCIM set_active (scim.rs:396-420) and SCIM delete skip the owner count that the kernel applies (transition.rs:1258).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5486",
      "security": true,
      "touches": ["backend"]
    },
    {
      "id": "F-local-account-suspension",
      "capability": "A person can be suspended: sessions and tokens end and sign-in is refused, reversibly.",
      "why": "PrincipalState is not enforced (WG-5490); revoke_own_session ends only the caller's sessions (auth.rs:518-528).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5490",
      "security": true,
      "touches": ["backend", "bff", "kernel"]
    },
    {
      "id": "F-session-end-audit",
      "capability": "Every end of a session by an administrator, a revocation commit or a grant revocation is an audit line with the actor.",
      "why": "log_end writes telemetry only (auth.rs:459-468); app.audit persists only with a tenant field (app.rs:368-380); change_audit.rs has no session case.",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5490",
      "security": true,
      "touches": ["backend"]
    },
    {
      "id": "F-audit-chain-head-anchor",
      "capability": "The audit chain's head is anchored outside the tenant database, so removal of the newest lines is detected.",
      "why": "audit_chain.rs:10-12 and audit_log.rs:1-30 state the newest lines can be removed unseen (decision D-6208).",
      "neededBy": ["baseline.area1.authorization", "baseline.area2.ledger"],
      "existingTask": "WG-5506",
      "security": true,
      "touches": ["storage-api", "backend", "storage-sqlite", "storage-postgres"]
    },
    {
      "id": "F-provider-change-attribution",
      "capability": "A change made on behalf of an identity provider or a realm rule is audited with the provider or the rule as actor, not with an arbitrary owner.",
      "why": "owner_context (service/mod.rs:113-135) acts as the first owner in the member table, and change_audit records that owner as the actor.",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["backend"]
    },
    {
      "id": "F-declared-sensitive-operations",
      "capability": "Operations marked sensitive in the contract are refused without fresh authentication, enforced from the declaration.",
      "why": "Step-up is placed by hand in about nine handlers (http.rs:350-975) and in the BFF members route (routes.rs:1856-1871); revoke-role and remove-team-member carry none (boundary.jsonc).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["spec", "bff", "backend"]
    },
    {
      "id": "F-separation-of-duties-policy",
      "capability": "A tenant declares separation-of-duties rules for operations and role changes, enforced by the kernel.",
      "why": "Separation exists only for workflow approvals (approvals.rs:116).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["kernel", "spec"]
    },
    {
      "id": "F-conditional-approval-decision",
      "capability": "A decision on a waiting approval is applied only if the state read for it is still current, so exactly one decision succeeds.",
      "why": "Whether the store makes the write conditional on the read state was not verified (WG-5488).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5488",
      "security": true,
      "touches": ["backend", "storage-sqlite", "storage-postgres"]
    },
    {
      "id": "F-proposal-gated-changes",
      "capability": "A change classed as risky by tenant policy is applied only from an approved proposal, checked against the revision it was projected at.",
      "why": "Specified as AGT-011 to AGT-013 and AGS-409; no proposal mechanism exists (the Impact Lab applies projections only at their revision).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["kernel", "backend", "bff"]
    },
    {
      "id": "F-time-bound-access",
      "capability": "Access grants carry an end, can be requested and approved, can be delegated person to person with scope and window, and support access is granted by an owner and audited before first read.",
      "why": "No expiry on roles or team membership; no access-request flow; support grants (ADM-163 to ADM-165) not built.",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5499",
      "security": true,
      "touches": ["kernel", "backend", "spec"]
    },
    {
      "id": "F-role-composition",
      "capability": "Roles can imply other roles, conflicting roles are refused on every path, and teams and principal attributes are runtime data.",
      "why": "No role implication (rules.rs:150-163), no conflict rule, define_team only (transition.rs:1304), members in the schema document.",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5500",
      "security": true,
      "touches": ["kernel", "backend"]
    },
    {
      "id": "F-platform-admin-role",
      "capability": "A platform tenant-admin role that every product can name in policies, invokers and audiences.",
      "why": "Platform roles are not among tenant roles (settings.rs:403-404); the operator is one credential (app.rs:290-293).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "WG-5474",
      "security": true,
      "touches": ["spec", "kernel", "backend"]
    },
    {
      "id": "F-sandbox-tenant-lifecycle",
      "capability": "A person starts a sandbox tenant from a template and becomes its owner, sandboxes expire unless kept, and staging changes reach production only through approved proposals.",
      "why": "Not built (spec ADM-154 to ADM-160); only a reserved staging role suffix exists (products.rs:331-333).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["spec", "backend", "kernel"]
    },
    {
      "id": "F-tenant-export-archive",
      "capability": "A tenant owner exports the tenant's records, files, configuration and audit entries as an archive with a manifest of hashes.",
      "why": "Not built (spec ADM-161, ADM-162); only a per-person export exists (subject_export.rs).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "spec"]
    },
    {
      "id": "F-capability-scoped-tokens",
      "capability": "A delegated or client token can carry a capability as well as object scopes, bounded by the person's or client's own capabilities.",
      "why": "Token scopes are action:object only (tokens.rs:124-143).",
      "neededBy": ["baseline.area1.authorization"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "spec"]
    },
    {
      "id": "F-ledger-tamper-evidence",
      "capability": "Ledger entries and the commit journal are hash-chained and anchored, so an alteration or deletion is detected without trusting the database.",
      "why": "Entries are protected by triggers only (storage-sqlite/src/ledger.rs:63-76; storage-postgres/src/ledger.rs:51-71); the journal is unchained (storage-sqlite/src/lib.rs:230-233); restore deletes journal rows (lib.rs:5177).",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "WG-5506",
      "security": true,
      "touches": ["storage-api", "storage-sqlite", "storage-postgres", "kernel"]
    },
    {
      "id": "F-ledger-posting-audit",
      "capability": "Every posting is an audit line with its actor, in the same chain as configuration changes.",
      "why": "change_audit.rs covers roles, teams, invitations, settings and schema, and has no ledger case.",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "kernel"]
    },
    {
      "id": "F-scheduled-reconciliation",
      "capability": "Stored balances are compared with journal replay on a schedule, and a difference is raised.",
      "why": "Replay is an operator endpoint only (backend/src/ledger.rs:469-476).",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "kernel"]
    },
    {
      "id": "F-balanced-journal-entries",
      "capability": "A journal of several postings commits only when debits equal credits per currency.",
      "why": "Transfers are two postings committed together (kernel/src/ledger.rs:23); no balance invariant across N postings.",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "WG-5504",
      "security": true,
      "touches": ["kernel", "spec"]
    },
    {
      "id": "F-account-hierarchy",
      "capability": "Accounts form a chart whose parent balance is the sum of its children.",
      "why": "Accounts are flat per owner, unit and scope (kernel/src/ledger.rs:78-103).",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "WG-5504",
      "security": true,
      "touches": ["kernel", "spec"]
    },
    {
      "id": "F-external-reconciliation",
      "capability": "The ledger's balances can be compared with an external statement, with differences recorded.",
      "why": "No import, statement or comparison exists in the ledger code.",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "",
      "security": true,
      "touches": ["backend", "spec"]
    },
    {
      "id": "F-ledger-read-access-audit",
      "capability": "Reads of ledger balances and entries are audited and reportable per record.",
      "why": "Reads are authorised and filtered (kernel/src/ledger.rs:1682-1719) and not audited.",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "WG-5506",
      "security": true,
      "touches": ["backend", "kernel", "spec"]
    },
    {
      "id": "F-reversal-idempotent-replay",
      "capability": "A retried reversal with the same key returns the first reversal's entry, as a retried posting does.",
      "why": "kernel/src/ledger.rs:417-437 refuses a repeat as already-reversed; postings replay (ledger.rs:611-636). Whether intent-level idempotency answers first was not traced.",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "",
      "security": true,
      "touches": ["kernel", "backend"]
    },
    {
      "id": "F-ledger-machine-checked-invariants",
      "capability": "Ledger invariants (balance equals the sum of entries, cap never passed, one event per entry) are proved against the implementation, not only tested and modelled.",
      "why": "No Lean theorem covers the ledger; Exegra claims are links, not proofs (specifications/README.md:198-203).",
      "neededBy": ["baseline.area2.ledger"],
      "existingTask": "",
      "security": true,
      "touches": ["lean", "kernel"]
    },
    {
      "id": "F-accessible-by-construction",
      "capability": "Every generated screen meets a decided accessibility baseline (WCAG 2.2 AA) by the rules of its primitives, not by review.",
      "why": "D-DS-18 is proposed and not decided (specifications/design-system.jsonc:23); no requirement states it.",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "WG-5457",
      "security": false,
      "touches": ["spec", "web", "specc"]
    },
    {
      "id": "F-definition-time-a11y-checks",
      "capability": "A product's screens are refused at definition when a control has no name, an image has no alternative text or marking, or headings skip levels.",
      "why": "ui.a11y.* checks named in docs/design-system.md:583-588 are not implemented; an unlabelled field is labelled by its identifier (spec-compiler/src/web_kernel.rs:1670).",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["specc", "spec", "web"]
    },
    {
      "id": "F-browser-accessibility-gate",
      "capability": "The default gate runs axe and keyboard checks on every change to generated screens.",
      "why": "Browser suites run only under spec-gate --browser (scripts/spec-gate.mjs:42-43, 84, 563).",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "WG-5473",
      "security": false,
      "touches": ["scripts", "tests", "specc"]
    },
    {
      "id": "F-contrast-verification-in-gate",
      "capability": "Colour contrast of every theme's token pairs is verified by the gate on the generated stylesheets.",
      "why": "ui-tokens.test.mjs and check-style-literals.mjs are not run by the gate (scripts/spec-gate.mjs:401; package.json:10); Lean contrast proofs are of an abstract model (lean/Strata/UI/Contrast.lean:45).",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["scripts", "lean", "web"]
    },
    {
      "id": "F-accessible-primitive-library",
      "capability": "A library of primitives (dialog with focus trap, menu with roving focus, combobox, tooltip, live toast region) that each pass keyboard and axe checks.",
      "why": "The primitives in docs/design-system.md:207-230 are not built; no matches in web/src.",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "WG-5457",
      "security": false,
      "touches": ["web", "spec"]
    },
    {
      "id": "F-request-language-and-direction",
      "capability": "The document carries the request language and direction, and every string is localisable.",
      "why": "html lang is hard-coded to en (bff/src/routes.rs:2407-2409); no dir; literal strings in views.",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["bff", "web", "spec"]
    },
    {
      "id": "F-image-text-alternative",
      "capability": "A tenant image without alternative text is refused or explicitly marked decorative, never silently decorative.",
      "why": "Tenant image alt defaults to empty (web/src/components/ds.js:103).",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["web", "specc", "spec"]
    },
    {
      "id": "F-live-status-announcements",
      "capability": "Every user-visible outcome (toast or message) is announced once, politely or assertively, without moving focus.",
      "why": "Toasts are a plain region (web/src/views/shell.js:88-100); only kernel announce text is announced (shell.js:87).",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["web", "kernel"]
    },
    {
      "id": "F-conversational-interface",
      "capability": "A conversation surface with keyboard, screen-reader, reduced-motion and source-labelling behaviour.",
      "why": "Not built in web/src; spec CNV-238, CNV-239, CNV-241, CNV-248 (milestone 9) have no code.",
      "neededBy": ["baseline.area3.accessibility"],
      "existingTask": "",
      "security": false,
      "touches": ["web", "spec"]
    }
  ]
}
```

---

## 5. Unsure

- **Session lifetimes.** `crates/backend/src/app.rs:27-30` says the specification states two different absolute session lifetimes (D-700 and the machine `session-authority-flow-impl`). The default (12 hours) matches D-700 (`intent.jsonc:1546`). I did not read the machine's value.
- **Realm password-off and bootstrap credential.** Exegra claims `m-realm-owner-refused` and `m-bootstrap-credential-expires` are verified. I did not trace the backend code path that enforces them; the generated model in `crates/model/target-machines/src/target_machines.rs` is the only place I found the realm rule.
- **Token-authenticated operation calls.** `identify` returns session or anonymous identities only (`auth.rs:215-283`). How an agent's JWT is checked for an operation call sits in the external resource server, which is outside this repository. I did not trace it.
- **Reversal replay.** Whether the store's intent-level idempotency answers a retried reversal with the first reversal's entry was not traced (`F-reversal-idempotent-replay`).
- **Toast announcements.** Whether each toast's text is also set as `announce` in the kernel was not checked for every form path.
- **Focus targets.** The first-failing-field focus uses ids such as `editor-<field>` (`web/src/kernel/update.js:525`). Whether every generic field has that id was not checked in a browser.
- **Tiers.** The brief asked about ledger tiers. The ledger has none. Plan tiers exist for quotas (`crates/backend/src/quota.rs:874`). If Plinth means something else by tiers, the baseline does not cover it.
- **SCIM role mapping and last owner.** WG-5486 describes the defect; I confirmed the code path (`scim.rs:396-420`) but did not run it.
- **Exegra verdicts.** The verdict file is a recorded result (dated 2026-11-25). I did not re-run Exegra. The verdicts are as recorded.
- **Gate counts.** The README's 37-of-739 figure is stale. The current counts I used are 1737 requirement records and 63 in milestone 1, counted by grep over `specifications/requirements.jsonc`.
- **Tests I did not open.** Test names listed here come from grep over file names and `fn` lines. I did not read every body, and I did not verify that each test asserts its tagged requirement.

## 6. Open tasks that cover gaps (read with `workgraph task show`)

- **WG-5474** (backlog): a platform tenant-admin role that products may name. Covers `F-platform-admin-role`.
- **WG-5486** (backlog, high): SCIM must not disable or delete the last owner. Covers `F-last-owner-on-every-path`.
- **WG-5488** (backlog): concurrent approval decisions must be conditional on the state read. Covers `F-conditional-approval-decision`.
- **WG-5490** (backlog): local account suspension and administrator session revocation. Covers `F-local-account-suspension` and `F-session-end-audit`.
- **WG-5499** (backlog): time-bound grants, role steps, delegation, scoped admin, break-glass. Covers `F-time-bound-access` and part of `F-identity-admin-capabilities`.
- **WG-5500** (backlog): runtime teams, role implication, principal attributes, conflicting roles. Covers `F-role-composition`.
- **WG-5506** (backlog, low): read-access audit, retention range and holds, anchored audit. Covers `F-audit-chain-head-anchor`, `F-ledger-tamper-evidence` (in part), `F-ledger-read-access-audit`.
- **WG-5504** (backlog, low): double-entry journals and chart of accounts on the ledger. Covers `F-balanced-journal-entries` and `F-account-hierarchy`; blocked by WG-5494.
- **WG-5473** (backlog): gallery, screenshot regression, axe and keyboard checks. Covers `F-browser-accessibility-gate` and part of `F-accessible-by-construction`.
- **WG-5457** (the design-system epic, referenced by WG-5473 and WG-5474): covers `F-accessible-by-construction` and `F-accessible-primitive-library`.

Not covered by any task I read: `F-role-definition-authority`, `F-service-account-role-bounds`, `F-provider-change-attribution`, `F-declared-sensitive-operations`, `F-separation-of-duties-policy`, `F-proposal-gated-changes`, `F-sandbox-tenant-lifecycle`, `F-tenant-export-archive`, `F-capability-scoped-tokens`, `F-ledger-posting-audit`, `F-scheduled-reconciliation`, `F-external-reconciliation`, `F-reversal-idempotent-replay`, `F-ledger-machine-checked-invariants`, `F-definition-time-a11y-checks`, `F-contrast-verification-in-gate`, `F-request-language-and-direction`, `F-image-text-alternative`, `F-live-status-announcements`, `F-conversational-interface`. Of these, `F-role-definition-authority` is the one with a live escalation path and is the first to file.
