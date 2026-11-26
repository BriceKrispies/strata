/-
Prints, for each theorem the project claims, the axioms its proof depends on.
Run with `lake env lean Audit.lean`. A theorem proved with `sorry` would list `sorryAx`.
-/
import Strata
import Strata.SchemaChangeProofs

#print axioms Strata.step_preserves_wf
#print axioms Strata.stepTenant_preserves_wf
#print axioms Strata.unauthorized_cannot_mutate
#print axioms Strata.unauthorized_cannot_update
#print axioms Strata.unauthorized_cannot_delete
#print axioms Strata.missing_record_hidden_on_update
#print axioms Strata.missing_record_hidden_on_delete
#print axioms Strata.step_ok_authorized
#print axioms Strata.step_isolated
#print axioms Strata.step_local
#print axioms Strata.step_deterministic
#print axioms Strata.lax_change_can_break_wf
#print axioms Strata.problem_none
#print axioms Strata.stepTenant_preserves_refs
#print axioms Strata.step_preserves_refs
#print axioms Strata.delete_refused_while_referenced
#print axioms Strata.delete_leaves_no_reference
#print axioms Strata.conforms_afterDelete
#print axioms Strata.problem_none_parts
#print axioms Strata.patch_keeps_unnamed
#print axioms Strata.patch_last_wins
#print axioms Strata.update_writes_patched
#print axioms Strata.update_keeps_unnamed
#print axioms Strata.update_raises_version
#print axioms Strata.delete_raises_referrer_version
#print axioms Strata.version_conflict_on_update
#print axioms Strata.version_conflict_on_delete
#print axioms Strata.create_fills_default
#print axioms Strata.required_list_never_blocks
#print axioms Strata.problem_none_within
#print axioms Strata.create_refused_over_record_bound
#print axioms Strata.create_within_limits
#print axioms Strata.update_within_limits

-- Slice 3: uniqueness
#print axioms Strata.valueProblem_conds
#print axioms Strata.valueProblem_uniq_dichotomy
#print axioms Strata.valueProblem_clash
#print axioms Strata.findSome_dichotomy
#print axioms Strata.findSome_hit
#print axioms Strata.fieldProblem_uniq_dichotomy
#print axioms Strata.fieldProblem_clash
#print axioms Strata.problem_unique_none
#print axioms Strata.problem_uniq_dichotomy
#print axioms Strata.problem_uniq_refusal
#print axioms Strata.agreeOn_comm
#print axioms Strata.dupFields_false
#print axioms Strata.dropRef_getD_keyed
#print axioms Strata.get_cons_self
#print axioms Strata.get_cons_ne
#print axioms Strata.get_clearFields_eq
#print axioms Strata.clearRef_keyed
#print axioms Strata.agreeOn_clearRef
#print axioms Strata.nodup_filterMap_fst
#print axioms Strata.mem_set_keys
#print axioms Strata.set_nodup
#print axioms Strata.patch_nodup
#print axioms Strata.normalise_keys_sub
#print axioms Strata.normalise_nodup
#print axioms Strata.newRecord_nodup
#print axioms Strata.clearRef_nodup
#print axioms Strata.keys_makeRequired
#print axioms Strata.keys_makeUnique
#print axioms Strata.mem_makeRequired
#print axioms Strata.mem_makeUnique
#print axioms Strata.mem_replace
#print axioms Strata.mem_afterDelete
#print axioms Strata.stepTenant_preserves_distinct
#print axioms Strata.stepTenant_preserves_unique
#print axioms Strata.create_duplicate_refused
#print axioms Strata.update_duplicate_refused
#print axioms Strata.define_unique_refused_over_duplicates
#print axioms Strata.set_unique_refused_over_duplicates
#print axioms Strata.lax_unique_change_can_break_wf
#print axioms Strata.strict_schema_change_conforms
#print axioms Strata.schemaViolated_false
#print axioms Strata.setRequired_spec
#print axioms Strata.defineUnique_spec
#print axioms Strata.lookup_map_ty
#print axioms Strata.lookup_makeUnique_ty
#print axioms Strata.fieldOk_makeUnique
#print axioms Strata.conforms_makeUnique
#print axioms Strata.setUnique_records
#print axioms Strata.defineUnique_records
#print axioms Strata.stepTenant_conform_ids
#print axioms Strata.step_preserves_unique
#print axioms Strata.setUnique_spec

#print axioms Strata.Evolution.closure
#print axioms Strata.Evolution.locality
#print axioms Strata.Evolution.compatibility
#print axioms Strata.Evolution.breaking_change_can_change_value
#print axioms Strata.Evolution.breaking_change_can_invalidate
#print axioms Strata.Evolution.rewrite_correct
#print axioms Strata.Evolution.rewrite_correct_nb
#print axioms Strata.Evolution.project_migrate
#print axioms Strata.Evolution.left_inverse_on_footprint
#print axioms Strata.Evolution.pinned_eval
#print axioms Strata.Evolution.pinned_eval_live
#print axioms Strata.Evolution.removeField_irreversible
#print axioms Strata.Evolution.retargetRef_irreversible
#print axioms Strata.Evolution.removeObject_irreversible
#print axioms Strata.Evolution.narrow_nat_irreversible
#print axioms Strata.Evolution.commutation
#print axioms Strata.Evolution.rewrite_compose
#print axioms Strata.Evolution.rewrite_chain
#print axioms Strata.Evolution.pinned_breaking_change_accepted

#print axioms Strata.WorkingSet.problem_eq_via
#print axioms Strata.WorkingSet.decideTenant_faithful
#print axioms Strata.WorkingSet.decideOn_fetch
#print axioms Strata.WorkingSet.decideOn_congr
#print axioms Strata.WorkingSet.touchedIn_sound
#print axioms Strata.WorkingSet.hit_stamp
#print axioms Strata.WorkingSet.serve_eq_fetch
#print axioms Strata.WorkingSet.sync_safe_self
#print axioms Strata.WorkingSet.readWS_value
#print axioms Strata.WorkingSet.checkedRead_linearizes
#print axioms Strata.WorkingSet.decideWS_refusal
#print axioms Strata.WorkingSet.decideWS_pendingOk
#print axioms Strata.WorkingSet.submitWS_world
#print axioms Strata.WorkingSet.submitWS_inv
#print axioms Strata.WorkingSet.stepWS_inv
#print axioms Strata.WorkingSet.stepWS_safe
#print axioms Strata.WorkingSet.stepWS_faithful
#print axioms Strata.WorkingSet.stepWS_within
#print axioms Strata.WorkingSet.stepWS_pins
#print axioms Strata.WorkingSet.runWS_faithful
#print axioms Strata.WorkingSet.runWS_reads
#print axioms Strata.WorkingSet.runWS_within
#print axioms Strata.WorkingSet.stepWS_reqOk
#print axioms Strata.WorkingSet.request_snapshot
#print axioms Strata.WorkingSet.runWS_reqOk
#print axioms Strata.WorkingSet.unfenced_breaks_reads
#print axioms Strata.WorkingSet.w4_violation_breaks_checked_read
#print axioms Strata.WorkingSet.admit_keeps_pinned
#print axioms Strata.WorkingSet.evict_keeps_pinned
#print axioms Strata.WorkingSet.resize_keeps_pinned
#print axioms Strata.WorkingSet.pin_resident
#print axioms Strata.WorkingSet.resize_budget_exact
#print axioms Strata.WorkingSet.resize_budget_le
#print axioms Strata.WorkingSet.machine_resize
#print axioms Strata.WorkingSet.below_sealing_erasure_safe
#print axioms Strata.WorkingSet.above_sealing_serves_erased
#print axioms Strata.WorkingSet.submit_one_generation
#print axioms Strata.WorkingSet.decideReq_refusal
#print axioms Strata.WorkingSet.decideReq_expected
#print axioms Strata.WorkingSet.reqGet_eq_fetch
#print axioms Strata.WorkingSet.decideReqWS_pendingOk
#print axioms Strata.WorkingSet.stepWS_invR
#print axioms Strata.WorkingSet.conflict_refreshes
#print axioms Strata.WorkingSet.quiet_run
#print axioms Strata.WorkingSet.resubmit_no_conflict
#print axioms Strata.WorkingSet.Leases.write_refused_unless_holder
#print axioms Strata.WorkingSet.Leases.acquire_refused_while_serving
#print axioms Strata.WorkingSet.Leases.cache_read_before_term
#print axioms Strata.WorkingSet.Leases.read_correct
#print axioms Strata.WorkingSet.Leases.stepL_inv
#print axioms Strata.WorkingSet.Leases.leased_reads_correct
#print axioms Strata.WorkingSet.Leases.linv_init
#print axioms Strata.WorkingSet.Leases.raw_breaks_leased_read
#print axioms Strata.WorkingSet.Arbiter.arbiter_alarm
#print axioms Strata.WorkingSet.Arbiter.arbiter_within
#print axioms Strata.WorkingSet.Arbiter.arbiter_floors
#print axioms Strata.WorkingSet.Arbiter.arbiter_length
#print axioms Strata.WorkingSet.Arbiter.arbiter_overplaced_le_floors
#print axioms Strata.WorkingSet.bytes_fitTo_le
#print axioms Strata.WorkingSet.resizeHard_within
#print axioms Strata.WorkingSet.resizeHard_eq_resize
#print axioms Strata.WorkingSet.machine_hard
#print axioms Strata.WorkingSet.machine_arbiter

#print axioms Strata.Client.Cache.run_base
#print axioms Strata.Client.Cache.run_coh
#print axioms Strata.Client.Cache.current_recent
#print axioms Strata.Client.Cache.current_value
#print axioms Strata.Client.Cache.current_answer
#print axioms Strata.Client.Cache.read_stale_older
#print axioms Strata.Client.Cache.rest_one_epoch
#print axioms Strata.Client.Cache.rest_above_floor
#print axioms Strata.Client.Cache.rest_same_tenant_epoch
#print axioms Strata.Client.Cache.contact_floor
#print axioms Strata.Client.Cache.tenant_epoch_purges
#print axioms Strata.Client.Cache.older_epoch_ignored
#print axioms Strata.Client.Cache.contact_reaches_current_epoch
#print axioms Strata.Client.Cache.out_of_order_epochs_recover
#print axioms Strata.Client.Cache.later_request_is_authoritative
#print axioms Strata.Client.Cache.lower_ordinal_recovers
#print axioms Strata.Client.Cache.restarted_counter_dropped_without_reset
#print axioms Strata.Client.Cache.reset_accepts
#print axioms Strata.Client.Cache.hear_purges
#print axioms Strata.Client.Cache.hear_access_purges
#print axioms Strata.Client.Cache.receipt_read
#print axioms Strata.Client.Cache.read_after_commit
#print axioms Strata.Client.Cache.incomplete_notice_serves_stale
#print axioms Strata.Client.Cache.fresh_contact_still_stale
#print axioms Strata.Client.Cache.stamp_after_open_serves_erased
#print axioms Strata.Client.Cache.unguarded_put_resurrects
#print axioms Strata.Client.Cache.guarded_put_drops
#print axioms Strata.Client.Cache.unfloored_put_resurrects
#print axioms Strata.Client.Cache.floored_put_drops
#print axioms Strata.Client.Cache.current_fetch
#print axioms Strata.Client.Cache.browser_generation_only_serves_erased
#print axioms Strata.Client.Privacy.reachable_inv
#print axioms Strata.Client.Privacy.no_cross_principal
#print axioms Strata.Client.Privacy.served_own
#print axioms Strata.Client.Privacy.signout_erases
#print axioms Strata.Client.Privacy.offline_signout_erases
#print axioms Strata.Client.Privacy.expire_locks
#print axioms Strata.Client.Privacy.unlock_needs_own_session
#print axioms Strata.Client.Privacy.expire_then_same_principal_unlocks
#print axioms Strata.Client.Privacy.revoke_device_erases
#print axioms Strata.Client.Privacy.revoke_all_erases
#print axioms Strata.Client.Privacy.offline_key_survives_reload
#print axioms Strata.Client.Privacy.rot_no_loss
#print axioms Strata.Client.Privacy.rot_no_lost_update
#print axioms Strata.Client.Privacy.rot_finish_retires
#print axioms Strata.Client.Privacy.rot_stale_append_breaks
#print axioms Strata.Client.Privacy.rot_stale_append_refused
#print axioms Strata.Client.Privacy.rot_blind_write_loses_update
#print axioms Strata.Client.Privacy.rot_cas_keeps_update
#print axioms Strata.Client.Queue.submit_ok_guard
#print axioms Strata.Client.Queue.submit_not_ok_unchanged
#print axioms Strata.Client.Queue.submit_classes
#print axioms Strata.Client.Queue.at_most_once
#print axioms Strata.Client.Queue.stamped_at_most_once
#print axioms Strata.Client.Queue.q2_sent
#print axioms Strata.Client.Queue.fence_holds
#print axioms Strata.Client.Queue.unfenced_lookup_applies_twice
#print axioms Strata.Client.Queue.fenced_lookup_applies_once
#print axioms Strata.Client.Queue.create_twice_after_forget
#print axioms Strata.Client.Queue.rekeyed_create_applies_twice
#print axioms Strata.Client.Queue.restore_without_epoch_applies_twice
#print axioms Strata.Client.Queue.restore_with_epoch_applies_once
#print axioms Strata.Client.Drafts.orphan_drops_server_values
#print axioms Strata.Client.Drafts.survives_orphaning_erasure
#print axioms Strata.Client.Drafts.gone_after_next_erasure
#print axioms Strata.Client.Drafts.gone_after_idle
#print axioms Strata.Client.Drafts.kept_until
#print axioms Strata.UI.Composition.check_iff
#print axioms Strata.UI.Composition.checked_bindings_resolve
#print axioms Strata.UI.Composition.publishable_no_placeholder
#print axioms Strata.UI.Composition.publishable_bound
#print axioms Strata.UI.Composition.bindAt_valid
#print axioms Strata.UI.Composition.bindAt_placeholders
#print axioms Strata.UI.Composition.refine_valid
#print axioms Strata.UI.Composition.refine_publishable
#print axioms Strata.UI.Composition.incremental_valid
#print axioms Strata.UI.Composition.unguarded_removal_breaks
#print axioms Strata.UI.Composition.orphan_valid
#print axioms Strata.UI.Composition.orphan_removes_only_failing
#print axioms Strata.UI.Composition.orphan_keeps_valid
#print axioms Strata.UI.Contrast.onColour_meets_aa
#print axioms Strata.UI.Contrast.search_finds
#print axioms Strata.UI.Contrast.checkTheme_iff
#print axioms Strata.UI.Visibility.render_sound
#print axioms Strata.UI.Visibility.served_has_element
#print axioms Strata.UI.Visibility.served_needs_content
#print axioms Strata.UI.Visibility.hidden_leaves_no_trace
#print axioms Strata.UI.Visibility.hidden_leaves_no_trace_in
#print axioms Strata.UI.Visibility.admin_needs_role
#print axioms Strata.UI.Visibility.admin_never_to_default_member
#print axioms Strata.UI.Visibility.admin_action_for_members_refused
#print axioms Strata.UI.Visibility.admin_default_role_refused
#print axioms Strata.UI.Visibility.select_sees
#print axioms Strata.UI.Visibility.select_first
#print axioms Strata.UI.Visibility.selectServed_sound
#print axioms Strata.UI.Visibility.selectServed_first
#print axioms Strata.UI.Visibility.variant_falls_through
#print axioms Strata.UI.Visibility.audOf_sees
#print axioms Strata.UI.Visibility.dataAud_sees
#print axioms Strata.UI.Visibility.public_grant_anon_sees
#print axioms Strata.UI.Visibility.contentAud_sub
#print axioms Strata.UI.Visibility.checked_screen_has_content
#print axioms Strata.UI.Visibility.empty_region_pruned
#print axioms Strata.UI.Visibility.static_does_not_keep_region
#print axioms Strata.UI.Visibility.narrow_class_eq_render
#print axioms Strata.UI.Visibility.renderClass_shared
#print axioms Strata.UI.Visibility.signed_in_decides
#print axioms Strata.UI.Loop.draft_steps_frame
#print axioms Strata.UI.Loop.snapshot_readable
#print axioms Strata.UI.Loop.snapshot_ignores_unreadable
#print axioms Strata.UI.Loop.preview_depends_on_readable_only
#print axioms Strata.UI.Loop.preview_reads_within_viewer
#print axioms Strata.UI.Loop.followed_within_viewer
#print axioms Strata.UI.Loop.no_stale_draft_write
#print axioms Strata.UI.Loop.revisions_increase
#print axioms Strata.UI.Loop.approve_applies_held
#print axioms Strata.UI.Loop.approve_class_matches
#print axioms Strata.UI.Loop.approve_refused_when_moved
#print axioms Strata.UI.Loop.approve_refused_on_hash_mismatch
#print axioms Strata.UI.Loop.staged_invisible_until_last
#print axioms Strata.UI.Visibility.selectNarrowed_eq_selectServed
#print axioms Strata.UI.Loop.hidden_reference_not_followed
#print axioms Strata.UI.Loop.early_change_to_reachable_visible

#print axioms Strata.Ledger.ledger_conserves_units
#print axioms Strata.Ledger.ledger_append_only
#print axioms Strata.Ledger.ledger_replay_idempotent
#print axioms Strata.Ledger.ledger_no_overdraft

-- Temporal logic library (Strata.Temporal)
#print axioms Strata.Temporal.suffix_zero
#print axioms Strata.Temporal.suffix_suffix
#print axioms Strata.Temporal.lift_suffix
#print axioms Strata.Temporal.act_suffix
#print axioms Strata.Temporal.eventually_of
#print axioms Strata.Temporal.always_elim
#print axioms Strata.Temporal.always_always
#print axioms Strata.Temporal.always_mono
#print axioms Strata.Temporal.always_imp
#print axioms Strata.Temporal.always_conj
#print axioms Strata.Temporal.eventually_disj
#print axioms Strata.Temporal.eventually_eventually
#print axioms Strata.Temporal.not_always_iff
#print axioms Strata.Temporal.not_eventually_iff
#print axioms Strata.Temporal.strong_implies_weak
#print axioms Strata.Temporal.leadsTo_refl
#print axioms Strata.Temporal.leadsTo_trans
#print axioms Strata.Temporal.leadsTo_disj
#print axioms Strata.Temporal.leadsTo_of_always_imp
#print axioms Strata.Temporal.leadsTo_apply
#print axioms Strata.Temporal.leadsTo_wf
#print axioms Strata.Temporal.invariant_of
#print axioms Strata.Temporal.invariant_of_spec
#print axioms Strata.Temporal.wf1
#print axioms Strata.Temporal.always_enabled_leadsTo
#print axioms Strata.Temporal.always_enabled_infinitely_often
#print axioms Strata.Temporal.Lock.lock_enabled
#print axioms Strata.Temporal.Lock.lock_deliver_leadsTo
#print axioms Strata.Temporal.Lock.lock_pass_leadsTo
#print axioms Strata.Temporal.Lock.lock_liveness
#print axioms Strata.Temporal.Lock.cyc_step
#print axioms Strata.Temporal.Lock.recur_of_period4
#print axioms Strata.Temporal.Lock.cycBeh_deliver_one
#print axioms Strata.Temporal.Lock.cycBeh_host_zero
#print axioms Strata.Temporal.Lock.cycBeh_host_one
#print axioms Strata.Temporal.Lock.lock_nonvacuous

#print axioms Strata.Browser.held_is_own
#print axioms Strata.Browser.held_guarded_signed_in
#print axioms Strata.Browser.page_shows_only_own
#print axioms Strata.Browser.visitor_shows_no_guarded_page
#print axioms Strata.Browser.session_change_drops_model
#print axioms Strata.Browser.requests_are_pending
#print axioms Strata.Browser.signed_in_view_reachable
#print axioms Strata.Browser.held_only_when_known
#print axioms Strata.Browser.cold_answer_changes_no_model
#print axioms Strata.Browser.cold_start_awaits_session
#print axioms Strata.Browser.login_sent_only_complete
#print axioms Strata.Browser.failure_keeps_session
#print axioms Strata.Browser.input_login_sends_nothing
#print axioms Strata.Browser.theme_cycle_returns
#print axioms Strata.Browser.theme_choice_stored
#print axioms Strata.Browser.session_change_keeps_theme
#print axioms Strata.Browser.dismissed_toast_gone
#print axioms Strata.Browser.toast_expiry_removes
#print axioms Strata.Browser.notice_advance_moves_both
#print axioms Strata.Browser.stopped_never_reconnects
#print axioms Strata.Browser.refusal_counts_and_stops
#print axioms Strata.Browser.invite_accepted_signs_nobody_in
#print axioms Strata.Browser.oidc_redirect_leaves_page
#print axioms Strata.Browser.link_sent_only_complete
#print axioms Strata.Browser.reconnect_waits_backoff
#print axioms Strata.Browser.second_factor_sent_only_when_waiting
#print axioms Strata.Browser.sign_in_ends_prompt
#print axioms Strata.Browser.refused_second_keeps_prompt
#print axioms Strata.Browser.passkey_answer_for_ceremony
#print axioms Strata.Browser.changed_list_is_read_again
#print axioms Strata.Browser.save_sent_only_checked
#print axioms Strata.Browser.publish_sent_only_checked
#print axioms Strata.Browser.editor_answer_reads_view
#print axioms Strata.Browser.manifest_version_never_drops
#print axioms Strata.Browser.tenant_action_sent_only_allowed
#print axioms Strata.Browser.tenant_answer_reads_view
#print axioms Strata.Browser.lab_run_sent_only_complete
#print axioms Strata.Browser.poll_only_while_running
#print axioms Strata.Browser.apply_only_when_ready
#print axioms Strata.Browser.page_data_is_own
#print axioms Strata.Browser.visitor_sees_no_guarded_data
#print axioms Strata.Browser.person_change_shows_loading


-- System spec and protocol (WG-5655): the trusted spec, the protocol, the refinement.
#print axioms Strata.System.serve_log_sub
#print axioms Strata.System.frames_ok_step
#print axioms Strata.System.spec_frames_readable
#print axioms Strata.System.serve_log_mem
#print axioms Strata.System.spec_answered_unique_step
#print axioms Strata.System.spec_log_unique
#print axioms Strata.System.updTxn_self
#print axioms Strata.System.updTxn_ne
#print axioms Strata.System.good_inert
#print axioms Strata.System.good_begin
#print axioms Strata.System.good_noSession
#print axioms Strata.System.upd_fields
#print axioms Strata.System.commit_transfer
#print axioms Strata.System.good_snap
#print axioms Strata.System.good_commit
#print axioms Strata.System.good_conflict
#print axioms Strata.System.good_done
#print axioms Strata.System.good_load
#print axioms Strata.System.good_storeDecided
#print axioms Strata.System.good_storeAccept
#print axioms Strata.System.good_storeRefuse
#print axioms Strata.System.good_step
#print axioms Strata.System.good_init
#print axioms Strata.System.steps_star
#print axioms Strata.System.star_one
#print axioms Strata.System.notFrame_ne
#print axioms Strata.System.frame_iff_send
#print axioms Strata.System.frame_eq_iff
#print axioms Strata.System.SysState.ext'
#print axioms Strata.System.abs_eq_of
#print axioms Strata.System.abs_init
#print axioms Strata.System.net_mono
#print axioms Strata.System.frames_eq_of
#print axioms Strata.System.star_eq_abs
#print axioms Strata.System.star_noSession
#print axioms Strata.System.star_accept
#print axioms Strata.System.star_refuse
#print axioms Strata.System.star_frame
#print axioms Strata.System.refine_star
#print axioms Strata.System.relayed_step
#print axioms Strata.System.protocol_refines
#print axioms Strata.System.observed_reply_is_decision
#print axioms Strata.System.observed_matches_spec
#print axioms Strata.System.frames_readable
#print axioms Strata.System.commit_once
#print axioms Strata.System.commit_is_verdict
#print axioms Strata.System.rev_step
#print axioms Strata.System.duplicate_commit_inert
#print axioms Strata.System.wInit_pinit
#print axioms Strata.System.refusal_reachable

-- Backend liveness (WG-5656): the fair spec, the reply, publish and decision theorems, the witness.
#print axioms Strata.System.fair_pers
#print axioms Strata.System.sfair_inf
#print axioms Strata.System.net_seq
#print axioms Strata.System.dcount_le
#print axioms Strata.System.dcount_mono
#print axioms Strata.System.dcount_upd
#print axioms Strata.System.bnd_upd
#print axioms Strata.System.txn_keep_step
#print axioms Strata.System.rev_mono_step
#print axioms Strata.System.updAtt_ne
#print axioms Strata.System.linv_bndP
#print axioms Strata.System.linv_bndT
#print axioms Strata.System.linv_rev
#print axioms Strata.System.linv_doneDec
#print axioms Strata.System.linv_nsRep
#print axioms Strata.System.linv_evsCom
#print axioms Strata.System.linv_txnFin
#print axioms Strata.System.linv_txnLoad
#print axioms Strata.System.linv_txnCom
#print axioms Strata.System.linv_loadRev
#print axioms Strata.System.linv_snapAtt
#print axioms Strata.System.linv_attLe
#print axioms Strata.System.linv_zeroNone
#print axioms Strata.System.linv_attBound
#print axioms Strata.System.linv_conflictLt
#print axioms Strata.System.linv_step
#print axioms Strata.System.linv_init
#print axioms Strata.System.seq_inv
#print axioms Strata.System.txn_chain
#print axioms Strata.System.txn_tenant_const
#print axioms Strata.System.store_enabled
#print axioms Strata.System.store_conflict_only
#print axioms Strata.System.ready_keep
#print axioms Strata.System.committing_keep
#print axioms Strata.System.storeStep_net
#print axioms Strata.System.store_step_undecided
#print axioms Strata.System.eventually_const_aux
#print axioms Strata.System.eventually_const
#print axioms Strata.System.txn_some_seq
#print axioms Strata.System.relay_of_reply
#print axioms Strata.System.relay_of_done
#print axioms Strata.System.nosession_false
#print axioms Strata.System.decided_none_of
#print axioms Strata.System.txn_exists
#print axioms Strata.System.attempt_mono_step
#print axioms Strata.System.conflict_inc
#print axioms Strata.System.protoSpec_parts
#print axioms Strata.System.reply_core
#print axioms Strata.System.reply_leadsTo
#print axioms Strata.System.publish_leadsTo
#print axioms Strata.System.decided_relayed_leadsTo
#print axioms Strata.System.sendOne_self
#print axioms Strata.System.updTxn_same
#print axioms Strata.System.tail_always
#print axioms Strata.System.tail_weak
#print axioms Strata.System.aT1_spec
#print axioms Strata.System.aT1_owner
#print axioms Strata.System.aT1_can
#print axioms Strata.System.a1_step
#print axioms Strata.System.a2_step
#print axioms Strata.System.a3_step
#print axioms Strata.System.a4_step
#print axioms Strata.System.a5_step
#print axioms Strata.System.a6_step
#print axioms Strata.System.a7_step
#print axioms Strata.System.a8_step
#print axioms Strata.System.a9_step
#print axioms Strata.System.a9_load
#print axioms Strata.System.a10_step
#print axioms Strata.System.a10_committed
#print axioms Strata.System.a11_step
#print axioms Strata.System.atr_step
#print axioms Strata.System.atr_ge
#print axioms Strata.System.a11_net
#print axioms Strata.System.a11_txn1
#print axioms Strata.System.a11_txne
#print axioms Strata.System.a11_decided
#print axioms Strata.System.a11_decided_rest
#print axioms Strata.System.a11_hist
#print axioms Strata.System.a11_rev
#print axioms Strata.System.a11_attempt
#print axioms Strata.System.a11_done
#print axioms Strata.System.a11_reply
#print axioms Strata.System.a11_clientReply
#print axioms Strata.System.a11_commit
#print axioms Strata.System.a11_fwd
#print axioms Strata.System.a11_load
#print axioms Strata.System.a11_snap
#print axioms Strata.System.a11_frame
#print axioms Strata.System.fwd_self
#print axioms Strata.System.begin_none
#print axioms Strata.System.relay_self
#print axioms Strata.System.done_self
#print axioms Strata.System.load_self
#print axioms Strata.System.commit_none
#print axioms Strata.System.conflict_none
#print axioms Strata.System.snap_none
#print axioms Strata.System.a11_committed
#print axioms Strata.System.publish_self
#print axioms Strata.System.store_self
#print axioms Strata.System.protoSpec_nonvacuous

-- Reduction (WG-5660): the concurrent transactions (a), the generic distributed reduction (b),
-- and their instantiation for the backend protocol.
#print axioms Strata.Reduction.upd_self
#print axioms Strata.Reduction.upd_ne
#print axioms Strata.Reduction.serialRun_append
#print axioms Strata.Reduction.committedReqs_length
#print axioms Strata.Reduction.filter_snoc_acc
#print axioms Strata.Reduction.filter_snoc_rej
#print axioms Strata.Reduction.tinv_init
#print axioms Strata.Reduction.accepted_of_ok
#print axioms Strata.Reduction.mem_accepted_base
#print axioms Strata.Reduction.tinv_step
#print axioms Strata.Reduction.tinv_of_reach
#print axioms Strata.Reduction.serial_equivalence
#print axioms Strata.Reduction.conflict_no_effect
#print axioms Strata.Reduction.refusal_no_effect
#print axioms Strata.Reduction.storeState_eq'
#print axioms Strata.Reduction.find_snoc
#print axioms Strata.Reduction.decideOk_refines_store
#print axioms Strata.Reduction.decideErr_refines_store
#print axioms Strata.Reduction.conflict_refines_store
#print axioms Strata.Reduction.step_refines_store
#print axioms Strata.Reduction.allRecvIn_netOk
#print axioms Strata.Reduction.netOk_mono
#print axioms Strata.Reduction.netOk_congr
#print axioms Strata.Reduction.set_cons
#print axioms Strata.Reduction.netOk_append
#print axioms Strata.Reduction.netOk_prefix
#print axioms Strata.Reduction.recv_of_nosend
#print axioms Strata.Reduction.split_first
#print axioms Strata.Reduction.seg_eq_hostEvs
#print axioms Strata.Reduction.tagIs_unique
#print axioms Strata.Reduction.netOk_remove
#print axioms Strata.Reduction.hostEvs_append
#print axioms Strata.Reduction.hostEvs_filter
#print axioms Strata.Reduction.hostEvs_of_all
#print axioms Strata.Reduction.hostEvs_of_none
#print axioms Strata.Reduction.sorted_split
#print axioms Strata.Reduction.hostEvs_split
#print axioms Strata.Reduction.seg_remove_ne
#print axioms Strata.Reduction.seg_remove_self
#print axioms Strata.Reduction.seg_netOk
#print axioms Strata.Reduction.ne_or_of_not_and
#print axioms Strata.Reduction.seg_remove_of_nonempty
#print axioms Strata.Reduction.reordered_nil
#print axioms Strata.Reduction.reorder_core
#print axioms Strata.Reduction.reduction
#print axioms Strata.Reduction.rstep_tk
#print axioms Strata.Reduction.rstep_empty
#print axioms Strata.Reduction.bridge_backend
#print axioms Strata.Reduction.bridge_store
#print axioms Strata.Reduction.bridge_other
#print axioms Strata.Reduction.sendIff
#print axioms Strata.Reduction.recvIff
#print axioms Strata.Reduction.mem_sendsOf
#print axioms Strata.Reduction.mem_recvsOf
#print axioms Strata.Reduction.recv_of_obl
#print axioms Strata.Reduction.eq_of_gap
#print axioms Strata.Reduction.star_trans
#print axioms Strata.Reduction.initState_pinit
#print axioms Strata.Reduction.step_role
#print axioms Strata.Reduction.bridge_run
#print axioms Strata.Reduction.backend_fine_refines
#print axioms Strata.Reduction.run_congr

-- Boundary codec (WG-5653)
#print axioms Strata.Codec.lawful_nat
#print axioms Strata.Codec.lawful_int
#print axioms Strata.Codec.lawful_bool
#print axioms Strata.Codec.lawful_str
#print axioms Strata.Codec.lawful_pair
#print axioms Strata.Codec.lawful_iso
#print axioms Strata.Codec.lawful_opt
#print axioms Strata.Codec.lawful_list
#print axioms Strata.Codec.lawful_onDelete
#print axioms Strata.Codec.lawful_err
#print axioms Strata.Codec.lawful_ty
#print axioms Strata.Codec.lawful_elem
#print axioms Strata.Codec.lawful_val
#print axioms Strata.Codec.lawful_fieldDef
#print axioms Strata.Codec.lawful_limits
#print axioms Strata.Codec.lawful_record
#print axioms Strata.Codec.lawful_tenant
#print axioms Strata.Codec.lawful_intent
#print axioms Strata.Codec.lawful_ctx
#print axioms Strata.Codec.lawful_result
#print axioms Strata.Codec.decodeBytes_encodeBytes
#print axioms Strata.Codec.encodeBytes_of_decodeBytes
#print axioms Strata.Codec.fix_val_str
#print axioms Strata.Codec.fix_tenant
#print axioms Strata.Codec.fix_intent_update
#print axioms Strata.Codec.fix_result_ok
#print axioms Strata.Codec.boundary_codecs


-- Slice 4: several object types
#print axioms Strata.unknown_type_refused
#print axioms Strata.unknown_type_owner
#print axioms Strata.unknown_type_other
#print axioms Strata.unknown_type_hidden
#print axioms Strata.create_refs_resolve
#print axioms Strata.update_refs_resolve
#print axioms Strata.write_refused_dangling_across_types
#print axioms Strata.update_refused_dangling_across_types
#print axioms Strata.unknown_type_schema_refused

-- Slice 4 correction: a strict schema change is checked over the object types it touches
#print axioms Strata.schemaViolated_of_strict
#print axioms Strata.schema_step_putType
#print axioms Strata.strict_schema_change_scoped
#print axioms Strata.strict_schema_change_conforms
#print axioms Strata.strict_schema_change_unique

-- Executable kernel (Strata/Impl/*.lean, docs/formal-verification.md): every theorem
#print axioms Strata.initialWorld_records
#print axioms Strata.Impl.ascend_append
#print axioms Strata.Impl.lookup_append_nat
#print axioms Strata.Impl.lookup_cell
#print axioms Strata.Impl.lookup_ascend
#print axioms Strata.Impl.lookup_absSlots
#print axioms Strata.Impl.has_absSlots
#print axioms Strata.Impl.mem_ascend
#print axioms Strata.Impl.mem_absSlots
#print axioms Strata.Impl.anyFrom_iff
#print axioms Strata.Impl.any_absSlots
#print axioms Strata.Impl.ids_ge
#print axioms Strata.Impl.ascend_replicate
#print axioms Strata.Impl.absSlots_replicate
#print axioms Strata.Impl.absSlots_push
#print axioms Strata.Impl.beq_false_of_ne'
#print axioms Strata.Impl.ascend_set
#print axioms Strata.Impl.absSlots_setIfInBounds
#print axioms Strata.Impl.ascend_map
#print axioms Strata.Impl.ascend_clear
#print axioms Strata.Impl.absSlots_clear
#print axioms Strata.Impl.absSlots_map
#print axioms Strata.Impl.absSlots_padTo
#print axioms Strata.Impl.size_padTo
#print axioms Strata.Impl.IObj.abs_schema
#print axioms Strata.Impl.IObj.abs_uniques
#print axioms Strata.Impl.lookup_map_abs
#print axioms Strata.Impl.typ?_abs
#print axioms Strata.Impl.putTypeI_abs
#print axioms Strata.Impl.putType_abs
#print axioms Strata.Impl.env_abs
#print axioms Strata.Impl.lookup_cons_ne
#print axioms Strata.Impl.lookup_cons_eq
#print axioms Strata.Impl.lookup_putTypeI
#print axioms Strata.Impl.inv_putType
#print axioms Strata.Impl.lookup_map_entry
#print axioms Strata.Impl.getRec_setSlot
#print axioms Strata.Impl.getRec_push_padTo
#print axioms Strata.Impl.size_setSlot
#print axioms Strata.Impl.getRec_some
#print axioms Strata.Impl.refHas_eq
#print axioms Strata.Impl.elem_resolves_eq
#print axioms Strata.Impl.val_resolves_eq
#print axioms Strata.Impl.clashAt_eq
#print axioms Strata.Impl.valueProblem_eq
#print axioms Strata.Impl.fieldProblem_eq
#print axioms Strata.Impl.compositeProblem_eq
#print axioms Strata.Impl.problem_eq
#print axioms Strata.Impl.canCreateI_eq
#print axioms Strata.Impl.canAttemptCreateI_eq
#print axioms Strata.Impl.canUpdateI_eq
#print axioms Strata.Impl.canDeleteI_eq
#print axioms Strata.Impl.missingRecordI_eq
#print axioms Strata.Impl.unknownTypeI_eq
#print axioms Strata.Impl.problemI_abs
#print axioms Strata.Impl.blocked_eq
#print axioms Strata.Impl.abs_members
#print axioms Strata.Impl.abs_limits
#print axioms Strata.Impl.abs_nextId
#print axioms Strata.Impl.abs_types
#print axioms Strata.Impl.afterDelete_abs
#print axioms Strata.Impl.except_map_ite
#print axioms Strata.Impl.stepTenantI_refines
#print axioms Strata.Impl.stepTenantI_inv
#print axioms Strata.Impl.stepI_refines
#print axioms Strata.Impl.stepI_inv
#print axioms Strata.Impl.ofEmpty_abs
#print axioms Strata.Impl.ofEmpty_inv
#print axioms Strata.Impl.implInitial_inv
#print axioms Strata.Impl.absW_implInitial
#print axioms Strata.Impl.clearRef_idem
#print axioms Strata.Impl.blockedX_eq
#print axioms Strata.Impl.typ?_putType_same
#print axioms Strata.Impl.typ?_putType_other
#print axioms Strata.Impl.ClearedFrom.refl
#print axioms Strata.Impl.clearOne_cleared
#print axioms Strata.Impl.ClearedFrom.congr
#print axioms Strata.Impl.clearAll_cleared
#print axioms Strata.Impl.getRec_map
#print axioms Strata.Impl.array_ext_getRec
#print axioms Strata.Impl.list_ext_lookup
#print axioms Strata.Impl.IObj.ext'
#print axioms Strata.Impl.ITenant.ext'
#print axioms Strata.Impl.getRec_delObjI
#print axioms Strata.Impl.deleteX_good
#print axioms Strata.Impl.delete_good
#print axioms Strata.Impl.MM.get_add
#print axioms Strata.Impl.MM.get_remove
#print axioms Strata.Impl.MM.mem_addAll
#print axioms Strata.Impl.MM.mem_removeAll
#print axioms Strata.Impl.MM.mem_addCells
#print axioms Strata.Impl.MM.inv_change
#print axioms Strata.Impl.MM.inv_addCells
#print axioms Strata.Impl.modifyTypeI_eq
#print axioms Strata.Impl.ITenant.modifyType_eq
#print axioms Strata.Impl.putTypeI_keys
#print axioms Strata.Impl.typ?_putType
#print axioms Strata.Impl.mem_of_lookup
#print axioms Strata.Impl.lookup_of_mem_nodup
#print axioms Strata.Impl.getRec_of_ge
#print axioms Strata.Impl.getRec_of_lt
#print axioms Strata.Impl.cc_putType
#print axioms Strata.Impl.CellChange.withNextId
#print axioms Strata.Impl.lt_of_getRec
#print axioms Strata.Impl.findSome_congr
#print axioms Strata.Impl.any_congr
#print axioms Strata.Impl.clashX_eq
#print axioms Strata.Impl.compClashX_eq
#print axioms Strata.Impl.valueProblemX_eq
#print axioms Strata.Impl.fieldProblemX_eq
#print axioms Strata.Impl.compositeProblemX_eq
#print axioms Strata.Impl.problemX_eq
#print axioms Strata.Impl.good_error
#print axioms Strata.Impl.create_good
#print axioms Strata.Impl.update_good
#print axioms Strata.Impl.keysBy_putSlots
#print axioms Strata.Impl.lookup_ty_map
#print axioms Strata.Impl.lookup_ty_makeRequired
#print axioms Strata.Impl.lookup_ty_makeUnique
#print axioms Strata.Impl.rkeysOf_congr
#print axioms Strata.Impl.ukeysOf_makeRequired
#print axioms Strata.Impl.ukeysOf_makeUnique_sub
#print axioms Strata.Impl.mem_cellsOf
#print axioms Strata.Impl.keysBy_ty
#print axioms Strata.Impl.MM.inv_putSlots
#print axioms Strata.Impl.setRequired_good
#print axioms Strata.Impl.setUnique_good
#print axioms Strata.Impl.defineUnique_good
#print axioms Strata.Impl.mem_refIds
#print axioms Strata.Impl.mem_ukeysOf
#print axioms Strata.Impl.mem_ckeysOf
#print axioms Strata.Impl.mem_rkeysOf
#print axioms Strata.Impl.CellChange.keysBy
#print axioms Strata.Impl.ITenant.keysBy_at
#print axioms Strata.Impl.ukf_congr
#print axioms Strata.Impl.ckf_congr
#print axioms Strata.Impl.rkf_congr
#print axioms Strata.Impl.XTenant.ok_reindex
#print axioms Strata.Impl.stepTenantXL_good
#print axioms Strata.Impl.stepTenantX_good
#print axioms Strata.Impl.stepTenantX_refines
#print axioms Strata.Impl.stepTenantX_ok
#print axioms Strata.Impl.stepX_refines
#print axioms Strata.Impl.stepX_ok
#print axioms Strata.Impl.ofCore_ok
#print axioms Strata.Impl.xInitial_ok
#print axioms Strata.Impl.absX_xInitial

-- Slice 5: expressions and constraints
#print axioms Strata.and_false_shortcircuit
#print axioms Strata.or_true_shortcircuit
#print axioms Strata.and_null_false
#print axioms Strata.and_null_true
#print axioms Strata.and_null_null
#print axioms Strata.and_true_null
#print axioms Strata.and_null_error
#print axioms Strata.and_int_error
#print axioms Strata.or_null_true
#print axioms Strata.or_null_false
#print axioms Strata.or_false_error
#print axioms Strata.not_null
#print axioms Strata.isNull_null
#print axioms Strata.cmp_null_operand
#print axioms Strata.cmp_kind_error
#print axioms Strata.cmp_bool_order_error
#print axioms Strata.and_false_left
#print axioms Strata.or_true_left
#print axioms Strata.and_null_left_nonfalse
#print axioms Strata.and_error_left
#print axioms Strata.constraint_violated_refused
#print axioms Strata.constraint_evaluation_refused
#print axioms Strata.null_constraint_passes
#print axioms Strata.constraint_first_failure
#print axioms Strata.create_constraint_refused
#print axioms Strata.update_constraint_refused
#print axioms Strata.create_default_error_when_permitted
#print axioms Strata.create_unauthorized_regardless_of_defaults
#print axioms Strata.create_checks_defaulted_record
#print axioms Strata.default_refused_by_constraint
#print axioms Strata.named_value_accepted
#print axioms Strata.unauthorized_regardless_of_defaults
#print axioms Strata.canCreate_attempt
#print axioms Strata.canCreate_false_of_attempt
#print axioms Strata.evalExpr_congr
#print axioms Strata.constraintProblem_congr
#print axioms Strata.satisfied_of_putType
#print axioms Strata.stepTenant_preserves_satisfied
#print axioms Strata.step_preserves_satisfied
#print axioms Strata.brkTenant_satisfied
#print axioms Strata.brk_delete_refused
#print axioms Strata.clearBreaks_constraints
#print axioms Strata.Impl.clearBreaks_eq
#print axioms Strata.create_fills_default
#print axioms Strata.create_within_limits
#print axioms Strata.update_within_limits
#print axioms Strata.create_refused_over_record_bound
#print axioms Strata.newRecord_nodup
#print axioms Strata.defaultsOf_nodup
#print axioms Strata.create_refs_resolve
#print axioms Strata.write_refused_dangling_across_types
#print axioms Strata.unknown_type_hidden
#print axioms Strata.Codec.lawful_expr
#print axioms Strata.Codec.lawful_err
#print axioms Strata.Codec.lawful_objType
#print axioms Strata.Codec.lawful_fieldDef
#print axioms Strata.Codec.fix_fieldDef
#print axioms Strata.Codec.fix_tenant

-- Slice 6: roles and policies as data (Strata/Model.lean, Strata/Roles.lean).
#print axioms Strata.Tenant.allows_iff
#print axioms Strata.default_deny
#print axioms Strata.default_deny_create
#print axioms Strata.default_deny_update
#print axioms Strata.default_deny_delete
#print axioms Strata.create_ok_permitted
#print axioms Strata.update_ok_permitted
#print axioms Strata.delete_ok_permitted
#print axioms Strata.setRequired_ok_permitted
#print axioms Strata.unknown_type_owner
#print axioms Strata.unknown_type_other
#print axioms Strata.unknown_type_hidden
#print axioms Strata.unknown_type_schema_refused
#print axioms Strata.rolesOf_system
#print axioms Strata.rolesOf_anonymous
#print axioms Strata.anonymous_not_owner
#print axioms Strata.system_never_permitted
#print axioms Strata.anonymous_permitted_only_public
#print axioms Strata.system_refused_schema
#print axioms Strata.system_refused_create
#print axioms Strata.system_refused_update
#print axioms Strata.system_refused_delete
#print axioms Strata.owner_holds_every_capability
#print axioms Strata.capability_held_by_grant
#print axioms Strata.fixture_owner_permits
#print axioms Strata.fixture_editor_permits
#print axioms Strata.fixture_stranger_refused_item
#print axioms Strata.fixture_anonymous_refused_item
#print axioms Strata.fixture_anonymous_creates_part
#print axioms Strata.fixture_stranger_refused_part
#print axioms Strata.fixture_editor_creates_item
#print axioms Strata.fixture_writer_updates_own
#print axioms Strata.fixture_writer_refused_other
#print axioms Strata.fixture_editor_updates_other
#print axioms Strata.fixture_writer_missing_record_hidden
#print axioms Strata.fixture_editor_missing_record
#print axioms Strata.fixture_system_refused_item

-- Slice 7: membership, teams, invitations, tenant creation (Strata/MembersProofs.lean)
#print axioms Strata.effective_roles
#print axioms Strata.system_holds_no_role
#print axioms Strata.anonymous_holds_public
#print axioms Strata.system_cannot_manage_members
#print axioms Strata.hasOwner_iff
#print axioms Strata.genesis_hasOwner
#print axioms Strata.changeRole_owner
#print axioms Strata.defineTeam_owner
#print axioms Strata.teamRole_owner
#print axioms Strata.teamMember_owner
#print axioms Strata.redeemInvitation_owner
#print axioms Strata.stepMember_owner_kept
#print axioms Strata.owner_kept_world
#print axioms Strata.stepMember_teamsNodup
#print axioms Strata.step_nodup
#print axioms Strata.assign_only_conferrable
#print axioms Strata.teamRole_only_conferrable
#print axioms Strata.teamMember_only_conferrable
#print axioms Strata.redeem_only_conferrable
#print axioms Strata.create_invitation_expiry
#print axioms Strata.redeem_sets_redeemed
#print axioms Strata.redeem_twice_refused
#print axioms Strata.revoked_not_redeemable
#print axioms Strata.redeem_not_expired
#print axioms Strata.redeem_not_closed
#print axioms Strata.redeem_issuer_authority
#print axioms Strata.reserved_refused
#print axioms Strata.assign_reserved_refused
#print axioms Strata.add_member_reserved_refused
#print axioms Strata.create_reserved_refused
#print axioms Strata.create_genesis
#print axioms Strata.create_existing_refused
#print axioms Strata.create_not_system_refused
#print axioms Strata.genesis_owner_and_nodup
#print axioms Strata.directOf_setDirect
#print axioms Strata.rolesOf_mono_setDirect

-- Slice 7 integration: the membership intents in `Intent` and `step`, the kernels, the working set and the codec
#print axioms Strata.stepTenant_member_eq
#print axioms Strata.step_cases
#print axioms Strata.stepMember_manage
#print axioms Strata.member_authorized
#print axioms Strata.genesisTenant_wf
#print axioms Strata.rolesOf_user
#print axioms Strata.rolesOf_user_no_teams
#print axioms Strata.reserved_caller_refused
#print axioms Strata.create_blank_refused
#print axioms Strata.create_world_form
#print axioms Strata.step_member_form
#print axioms Strata.stepTenant_data_mt
#print axioms Strata.step_data_mt
#print axioms Strata.step_other
#print axioms Strata.step_exists
#print axioms Strata.Impl.createdTenantI_abs
#print axioms Strata.Impl.genesis_inv
#print axioms Strata.Impl.createTenant_good
#print axioms Strata.Impl.member_good
#print axioms Strata.Impl.genesis_ok
#print axioms Strata.Impl.createdTenantX_abs
#print axioms Strata.Impl.createdTenantX_ok
#print axioms Strata.WorkingSet.sound_members
#print axioms Strata.WorkingSet.sound_genesis
#print axioms Strata.WorkingSet.applyWorld_other
#print axioms Strata.Codec.lawful_teamDef
#print axioms Strata.Codec.lawful_invitationDef
#print axioms Strata.Codec.lawful_memberIntent
#print axioms Strata.Codec.fix_tenant_members
#print axioms Strata.Codec.fix_policy_members
#print axioms Strata.Codec.fix_intent_createTenant
#print axioms Strata.Codec.fix_intent_assignRole
#print axioms Strata.Codec.fix_intent_defineTeam
#print axioms Strata.Codec.fix_intent_createInvitation
#print axioms Strata.Codec.fix_intent_revokeInvitation
#print axioms Strata.Codec.fix_intent_redeemInvitation
#print axioms Strata.Codec.fix_intent_redeemInvitation_unverified
#print axioms Strata.Codec.fix_err_invalidIntent
#print axioms Strata.Codec.fix_err_unknownRole
#print axioms Strata.Codec.fix_err_lastOwner
#print axioms Strata.Codec.fix_err_tenantExists

-- Slice 8: workflows (`Strata/Workflows.lean`)
#print axioms Strata.create_ok_initials
#print axioms Strata.create_workflow_initial
#print axioms Strata.create_non_initial_refused
#print axioms Strata.fireOne_declared
#print axioms Strata.fireOne_keeps
#print axioms Strata.fireAll_final
#print axioms Strata.fireAll_keeps
#print axioms Strata.fireAll_present
#print axioms Strata.clearRef_present
#print axioms Strata.get_clearFields_str
#print axioms Strata.update_changed_fired
#print axioms Strata.update_undeclared_refused
#print axioms Strata.update_writes_fired
#print axioms Strata.str_of_clearFields
#print axioms Strata.clearRef_statesOk
#print axioms Strata.stepTenant_preserves_states
#print axioms Strata.step_preserves_states
#print axioms Strata.update_raises_version
#print axioms Strata.update_writes_patched
#print axioms Strata.update_keeps_unnamed
#print axioms Strata.update_ok_parts
#print axioms Strata.fireAll_plain
#print axioms Strata.fireOne_version
#print axioms Strata.fireAll_nodup
#print axioms Strata.evalSets_keys
#print axioms Strata.applySets_get_mem
#print axioms Strata.wf_lookup_filter_none
#print axioms Strata.lookup_filter_of_ne

-- Slice 9: events, revisions and effects (`Strata/EventProofs.lean`).
#print axioms Strata.transition_ok
#print axioms Strata.transition_refused
#print axioms Strata.eventBodies_commit
#print axioms Strata.transition_existing
#print axioms Strata.numberEvents_map_seq
#print axioms Strata.numberEvents_length
#print axioms Strata.transition_seqs
#print axioms Strata.transition_publish
#print axioms Strata.transition_revision
#print axioms Strata.transition_created
#print axioms Strata.typ_of_stepTenant_create
#print axioms Strata.transition_create
#print axioms Strata.transition_update
#print axioms Strata.stored_of_stepTenant_delete
#print axioms Strata.transition_delete
#print axioms Strata.transition_setRequired
#print axioms Strata.Impl.absW_commitWorldI
#print axioms Strata.Impl.transitionI_refines
#print axioms Strata.Impl.absX_commitWorldX
#print axioms Strata.Impl.transitionX_refines
#print axioms Strata.Impl.commitI_abs

/-! ## Slice 10: reads (`Strata/Reads.lean`): records and named queries

The statements are about the model's `read`. The theorems are checked by `lake env lean Audit.lean`,
which is not a module of the `lake build` target. -/

namespace Strata

/-! ### Visibility and the system -/

theorem system_never_visible (t : Tenant) (ty : TypeId) (r : Record) :
    t.canRead .system ty r = false := by
  simp [Tenant.canRead, Tenant.allows, Tenant.rolesOf, Tenant.mt, MTenant.rolesOf]

theorem redact_system (rs : FieldRules) (ty : TypeId) (r : Record) :
    redact rs .system ty r = r := by
  have hkeep : r.fields.filter (fun fv => shownField rs .system ty r fv.1) = r.fields :=
    List.filter_eq_self.mpr (fun a _ => by simp [shownField])
  simp only [redact, hkeep]

/-! ### Record reads -/

theorem rowOf_visible {t : Tenant} {rs : FieldRules} {p : Principal} {ty : TypeId}
    {id : RecordId} {row : RecordId × Record} (h : rowOf t rs p ty id = some row) :
    ∃ r, t.recordAt ty id = some r ∧ t.canRead p ty r = true ∧ row = (id, redact rs p ty r) := by
  unfold rowOf at h
  cases hr : t.recordAt ty id with
  | none => simp [hr] at h
  | some r =>
    cases hv : t.canRead p ty r with
    | false => simp [hr, hv] at h
    | true =>
      simp only [hr, hv, ite_true] at h
      exact ⟨r, rfl, hv, (Option.some.inj h).symm⟩

/-- A missing record and an invisible one are left out the same way: neither yields a row. -/
theorem missing_and_invisible_same (t : Tenant) (rs : FieldRules) (p : Principal) (ty : TypeId)
    (id : RecordId) :
    (t.recordAt ty id = none → rowOf t rs p ty id = none) ∧
    (∀ r, t.recordAt ty id = some r → t.canRead p ty r = false → rowOf t rs p ty id = none) := by
  constructor
  · intro h
    simp [rowOf, h]
  · intro r hr hv
    simp [rowOf, hr, hv]

theorem readRecords_limit_first (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (h : maxReadIds < ids.length) :
    readRecords t spec p ty ids = .error .limitExceeded := by
  simp [readRecords, h]

theorem readRecords_unknown_type (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (h : ids.length ≤ maxReadIds) (hty : t.typ? ty = none) :
    readRecords t spec p ty ids = .error .unknownObject := by
  have h' : ¬ maxReadIds < ids.length := by omega
  simp [readRecords, h', hty]

/-- The rows of a record read are the visible records, redacted, in request order. -/
theorem readRecords_rows (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (rows : List (RecordId × Record))
    (h : readRecords t spec p ty ids = .ok rows) :
    rows = ids.filterMap (rowOf t spec.rules p ty) ∧
    ∀ x ∈ rows, ∃ id r, t.recordAt ty id = some r ∧ t.canRead p ty r = true ∧
      x = (id, redact spec.rules p ty r) := by
  unfold readRecords at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · have hrows : rows = ids.filterMap (rowOf t spec.rules p ty) := by
        simpa using (Except.ok.inj h).symm
      refine ⟨hrows, ?_⟩
      intro x hx
      rw [hrows] at hx
      obtain ⟨id, _, hid⟩ := List.mem_filterMap.mp hx
      obtain ⟨r, hr, hv, hx'⟩ := rowOf_visible hid
      exact ⟨id, r, hr, hv, hx'⟩

/-- A read rule that hides a field of a stored record keeps that field out of every row, for every
    reader but the system (which the rule does not restrict). -/
theorem hidden_field_absent (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record)
    (f : FieldId) (e : Expr) (hp : p ≠ .system) (hr : rs.rule ty f = some e)
    (hh : condHolds (readEnv p r) (some e) = false) :
    ∀ v, (f, v) ∉ (redact rs p ty r).fields := by
  intro v hv
  simp only [redact, List.mem_filter] at hv
  obtain ⟨_, hs⟩ := hv
  cases p with
  | system => exact hp rfl
  | user n => simp_all [shownField]
  | anonymous => simp_all [shownField]

/-- The system is refused every record, so it reads none: the answer is the error or the empty list. -/
theorem system_reads_no_record (t : Tenant) (spec : ReadSpec) (ty : TypeId)
    (ids : List RecordId) (h : ids.length ≤ maxReadIds) :
    readRecords t spec .system ty ids =
      if (t.typ? ty).isNone then .error .unknownObject else .ok [] := by
  have h' : ¬ maxReadIds < ids.length := by omega
  unfold readRecords
  simp only [h', ite_false]
  split
  · rfl
  · have : ids.filterMap (rowOf t spec.rules .system ty) = [] := by
      apply List.filterMap_eq_nil_iff.mpr
      intro id _
      unfold rowOf
      cases t.recordAt ty id with
      | none => rfl
      | some r => simp [system_never_visible]
    simp [this]

end Strata

namespace Strata

/-! ### Named queries -/

theorem scanRows_mem (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) :
    x ∈ scanRows t spec p q ↔ ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ t.canRead p q.ty r = true ∧
      condHolds (readEnv p (redact spec.rules p q.ty r)) q.filter = true ∧
      x = (id, redact spec.rules p q.ty r) := by
  unfold scanRows
  rw [List.mem_filterMap]
  constructor
  · rintro ⟨⟨id, r⟩, hmem, hx⟩
    dsimp only at hx
    split at hx
    · rename_i hv
      split at hx
      · rename_i hf
        simp only [Option.some.injEq] at hx
        exact ⟨id, r, hmem, hv, hf, hx.symm⟩
      · simp at hx
    · simp at hx
  · rintro ⟨id, r, hmem, hv, hf, rfl⟩
    refine ⟨(id, r), hmem, ?_⟩
    simp [hv, hf]

theorem queryAll_mem_iff (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) : x ∈ queryAll t spec p q ↔ x ∈ scanRows t spec p q := by
  unfold queryAll
  exact List.mem_mergeSort

/-- The rows of a named query are exactly the records the principal may read whose redacted copy
    makes the filter exactly `true`, each redacted. -/
theorem query_mem_iff (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) :
    x ∈ queryAll t spec p q ↔ ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ t.canRead p q.ty r = true ∧
      condHolds (readEnv p (redact spec.rules p q.ty r)) q.filter = true ∧
      x = (id, redact spec.rules p q.ty r) := by
  rw [queryAll_mem_iff, scanRows_mem]

/-- A row of a named query hides no field a read rule hides on the record it was read from. -/
theorem query_rows_hide (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (x : RecordId × Record) (hx : x ∈ queryAll t spec p q) :
    ∃ id r, (id, r) ∈ t.recordsOf q.ty ∧ x = (id, redact spec.rules p q.ty r) ∧
      ∀ f e, spec.rules.rule q.ty f = some e →
        condHolds (readEnv p r) (some e) = false →
        ∀ v, (f, v) ∉ x.2.fields := by
  obtain ⟨id, r, hmem, hv, _, hxr⟩ := (query_mem_iff t spec p q x).mp hx
  refine ⟨id, r, hmem, hxr, ?_⟩
  intro f e hr hh v hvm
  have hp : p ≠ .system := by
    rintro rfl
    simp [system_never_visible] at hv
  rw [hxr] at hvm
  exact hidden_field_absent spec.rules p q.ty r f e hp hr hh v hvm

theorem orderKey_length (q : QueryDef) (row : RecordId × Record) :
    (orderKey q row).length = (orderDirs q).length := by
  simp [orderKey, orderDirs]

/-- The rows of a named query are in order: each row's keys, then its id, are at most the next row's. -/
theorem queryAll_sorted (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef) :
    (queryAll t spec p q).Pairwise
      (fun a b => keysLe (orderDirs q) (orderKey q a) (orderKey q b) = true) := by
  unfold queryAll
  apply List.pairwise_mergeSort
  · intro a b c h1 h2
    exact keysLe_trans _ _ _ _ (orderKey_length q a) (orderKey_length q b)
      (orderKey_length q c) h1 h2
  · intro a b
    exact keysLe_total _ _ _ (orderKey_length q a) (orderKey_length q b)

theorem walk_eq_drop {α : Type} (L : List α) (size : Nat) (hs : 0 < size) :
    ∀ n off, L.length - off ≤ n → walk L size off = L.drop off := by
  intro n
  induction n with
  | zero =>
    intro off h
    have hlen : L.length ≤ off := by omega
    unfold walk
    split
    · omega
    · simp [List.drop_eq_nil_of_le hlen]
  | succ n ih =>
    intro off h
    unfold walk
    split
    · rename_i hc
      rw [ih (off + size) (by omega), ← List.drop_drop, List.take_append_drop]
    · rename_i hc
      rw [List.take_of_length_le (by simp; omega)]

/-- The pages of a named query, concatenated, are the whole of its result, in order. Since the result
    lists each record once, no record appears in two pages. -/
theorem query_pages_concat (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (size : Nat) (hs : 0 < size) :
    walk (queryAll t spec p q) size 0 = queryAll t spec p q := by
  rw [walk_eq_drop _ size hs (queryAll t spec p q).length 0 (by simp), List.drop_zero]

/-! ### The limits and refusals of a named query -/

theorem query_unknown (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor)
    (h : spec.queries.lookup qid = none) :
    queryPage t spec p qid params page cursor = .error .unknownQuery := by
  simp [queryPage, h]

theorem query_undeclared_param (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = true) :
    queryPage t spec p qid params page cursor = .error .invalidIntent := by
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_pos hp]

theorem query_page_zero (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : (page.or q.limit).getD defaultPage = 0) :
    queryPage t spec p qid params page cursor = .error .invalidIntent := by
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_pos hs]

theorem query_page_over (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : maxPage < (page.or q.limit).getD defaultPage) :
    queryPage t spec p qid params page cursor = .error .limitExceeded := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_pos hs]

theorem query_wrong_cursor (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (q : QueryDef) (id : RecordId)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : 0 < (page.or q.limit).getD defaultPage)
    (hb : (page.or q.limit).getD defaultPage ≤ maxPage) :
    queryPage t spec p qid params page (some (.after id)) = .error .invalidIntent := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  have hb' : ¬ maxPage < (page.or q.limit).getD defaultPage := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_neg hb']

theorem query_scan_limit (t : Tenant) (spec : ReadSpec) (p : Principal) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (off : Nat) (q : QueryDef)
    (hq : spec.queries.lookup qid = some q)
    (hp : params.any (fun kv => !(q.params.contains kv.1)) = false)
    (hs : 0 < (page.or q.limit).getD defaultPage)
    (hb : (page.or q.limit).getD defaultPage ≤ maxPage)
    (hn : maxScan < ((t.typ? q.ty).map (fun o => o.records.length)).getD 0) :
    queryPage t spec p qid params page (some (.offset off)) = .error .limitExceeded := by
  have hz : (page.or q.limit).getD defaultPage ≠ 0 := by omega
  have hb' : ¬ maxPage < (page.or q.limit).getD defaultPage := by omega
  unfold queryPage
  rw [hq]
  dsimp only
  rw [if_neg (by rw [hp]; decide), if_neg hz, if_neg hb', if_pos hn]

/-! ### The system -/

/-- The system is refused every record of every type, so a query's result for it is empty. -/
theorem queryAll_system (t : Tenant) (spec : ReadSpec) (q : QueryDef) :
    queryAll t spec .system q = [] := by
  unfold queryAll scanRows
  have h0 : (t.recordsOf q.ty).filterMap (fun e =>
      if t.canRead .system q.ty e.2 then
        (if condHolds (readEnv .system (redact spec.rules .system q.ty e.2)) q.filter = true then
          some (e.1, redact spec.rules .system q.ty e.2) else none)
      else none) = [] := by
    apply List.filterMap_eq_nil_iff.mpr
    intro e _
    simp [system_never_visible]
  rw [h0]
  simp

end Strata

namespace Strata

/-! ### Distinct rows, hidden fields and the system, for both reads -/

theorem filterMap_fst_sublist {γ α β : Type} (l : List (γ × α)) (f : γ × α → Option (γ × β))
    (hf : ∀ e b, f e = some b → b.1 = e.1) :
    ((l.filterMap f).map Prod.fst).Sublist (l.map Prod.fst) := by
  induction l with
  | nil => simp
  | cons e l ih =>
    rw [List.filterMap_cons]
    cases h : f e with
    | none =>
      simp only [h, List.map_cons]
      exact ih.trans (List.sublist_cons_self _ _)
    | some b =>
      have hb := hf e b h
      simp only [h, List.map_cons, hb]
      exact List.Sublist.cons₂ _ ih

theorem scanRows_ids_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((scanRows t spec p q).map Prod.fst).Nodup := by
  unfold scanRows
  refine List.Sublist.nodup (filterMap_fst_sublist _ _ ?_) h
  intro e b hb
  dsimp only at hb
  split at hb
  · split at hb
    · simp only [Option.some.injEq] at hb
      rw [← hb]
    · simp at hb
  · simp at hb

/-- The rows of a named query name each record once, when the records of its type are distinct. -/
theorem queryAll_ids_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((queryAll t spec p q).map Prod.fst).Nodup := by
  unfold queryAll
  exact ((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr (scanRows_ids_nodup t spec p q h)

/-- The pages of a named query name no record twice: concatenated, they are the result. -/
theorem query_pages_nodup (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (size : Nat) (hs : 0 < size) (h : ((t.recordsOf q.ty).map Prod.fst).Nodup) :
    ((walk (queryAll t spec p q) size 0).map Prod.fst).Nodup := by
  rw [query_pages_concat t spec p q size hs]
  exact queryAll_ids_nodup t spec p q h

/-- A row of a record read hides no field a read rule hides on the record it was read from; the
    system reads no row at all, since no record is visible to it. -/
theorem readRecords_hide (t : Tenant) (spec : ReadSpec) (p : Principal) (ty : TypeId)
    (ids : List RecordId) (rows : List (RecordId × Record))
    (h : readRecords t spec p ty ids = .ok rows) :
    ∀ x ∈ rows, ∃ id r, t.recordAt ty id = some r ∧ x = (id, redact spec.rules p ty r) ∧
      ∀ f e, spec.rules.rule ty f = some e →
        condHolds (readEnv p r) (some e) = false →
        ∀ v, (f, v) ∉ x.2.fields := by
  intro x hx
  obtain ⟨_, hrows⟩ := readRecords_rows t spec p ty ids rows h
  obtain ⟨id, r, hr, hv, hxr⟩ := hrows x hx
  refine ⟨id, r, hr, hxr, ?_⟩
  intro f e hf hh v hvm
  have hp : p ≠ .system := by
    rintro rfl
    simp [system_never_visible] at hv
  rw [hxr] at hvm
  exact hidden_field_absent spec.rules p ty r f e hp hf hh v hvm

/-- The system's page of a named query is empty, whatever the query and the cursor. -/
theorem queryPage_system_rows (t : Tenant) (spec : ReadSpec) (qid : QueryId)
    (params : List (String × Val)) (page : Option Nat) (cursor : Option Cursor)
    (r : ReadResult) (h : queryPage t spec .system qid params page cursor = .ok r) :
    r.rows = [] := by
  unfold queryPage at h
  rcases hq : spec.queries.lookup qid with _ | q
  · rw [hq] at h
    simp at h
  · rw [hq] at h
    dsimp only at h
    repeat' split at h
    all_goals first
      | (cases h <;> simp [queryAll_system])
      | cases h

end Strata

/-! ## Slice 10 axioms -/
#print axioms Strata.Val.leb_total
#print axioms Strata.Val.leb_trans
#print axioms Strata.dirLe_total
#print axioms Strata.dirLe_trans
#print axioms Strata.keysLe_total
#print axioms Strata.keysLe_trans
#print axioms Strata.system_never_visible
#print axioms Strata.redact_system
#print axioms Strata.rowOf_visible
#print axioms Strata.missing_and_invisible_same
#print axioms Strata.readRecords_limit_first
#print axioms Strata.readRecords_unknown_type
#print axioms Strata.readRecords_rows
#print axioms Strata.hidden_field_absent
#print axioms Strata.system_reads_no_record
#print axioms Strata.scanRows_mem
#print axioms Strata.queryAll_mem_iff
#print axioms Strata.query_mem_iff
#print axioms Strata.query_rows_hide
#print axioms Strata.orderKey_length
#print axioms Strata.queryAll_sorted
#print axioms Strata.walk_eq_drop
#print axioms Strata.query_pages_concat
#print axioms Strata.query_unknown
#print axioms Strata.query_undeclared_param
#print axioms Strata.query_page_zero
#print axioms Strata.query_page_over
#print axioms Strata.query_wrong_cursor
#print axioms Strata.query_scan_limit
#print axioms Strata.queryAll_system
#print axioms Strata.filterMap_fst_sublist
#print axioms Strata.scanRows_ids_nodup
#print axioms Strata.queryAll_ids_nodup
#print axioms Strata.query_pages_nodup
#print axioms Strata.readRecords_hide
#print axioms Strata.queryPage_system_rows

/-! ## Slice 10: the order on values as the kernel's, and the executable read -/
#print axioms Strata.Elem.leb_total
#print axioms Strata.Elem.leb_trans
#print axioms Strata.lexLe_total
#print axioms Strata.lexLe_trans
#print axioms Strata.Impl.slotRows_eq
#print axioms Strata.Impl.ITenant.recordsOf_abs
#print axioms Strata.Impl.ITenant.canRead_abs
#print axioms Strata.Impl.ITenant.rowOfI_abs
#print axioms Strata.Impl.readRecordsI_abs
#print axioms Strata.Impl.ITenant.scanRowsI_abs
#print axioms Strata.Impl.ITenant.queryAllI_abs
#print axioms Strata.Impl.ITenant.countI_abs
#print axioms Strata.Impl.queryPageI_abs
#print axioms Strata.Impl.readI_refines
#print axioms Strata.Impl.readX_refines

/-! ## Slice 10, second half: search and aggregates (`Strata/Search.lean`, `Strata/Aggregates.lean`) -/

namespace Strata

/-- A hit is sound for a query and a reader: it is a stored record of its type that the reader may read, given
    as the reader receives it (redacted), and its matched fields are shown ones that match every query word. -/
def SoundHit (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (h : SearchHit) : Prop :=
  ∃ rec, (h.id, rec) ∈ src.recs h.ty ∧ src.canRead p h.ty rec = true ∧
    h.record = redact rs p h.ty rec ∧ h.matched ≠ [] ∧
    ∀ f ∈ h.matched, (∃ v, (f, v) ∈ h.record.fields) ∧
      allMatch q (textTokens (Fields.get h.record.fields f)) = true

theorem matchedIn_mem (q : List String) (fs : List FieldId) (r : Record) (f : FieldId)
    (h : f ∈ matchedIn q fs r) : f ∈ fs := by
  unfold matchedIn at h
  exact (List.mem_filter.mp h).1

theorem matchedIn_match (q : List String) (fs : List FieldId) (r : Record) (f : FieldId)
    (h : f ∈ matchedIn q fs r) : allMatch q (textTokens (Fields.get r.fields f)) = true := by
  unfold matchedIn at h
  have := (List.mem_filter.mp h).2
  simpa using this

theorem candidatesOf_mem (src : Src) (ty : TypeId) (q : List String) (probes : List FieldId) (c : Cand)
    (hc : c ∈ candidatesOf src ty q probes) : (c.id, c.record) ∈ src.recs ty := by
  unfold candidatesOf at hc
  obtain ⟨e, he, hf⟩ := List.mem_filterMap.mp hc
  have he' : e ∈ src.recs ty := List.mem_mergeSort.mp he
  by_cases hm : (matchedIn q probes e.2).isEmpty = true
  · simp [hm] at hf
  · simp only [hm, Bool.false_eq_true, if_false, Option.some.injEq] at hf
    subst hf
    exact he'

theorem decideCand_sound (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hc : (c.id, c.record) ∈ src.recs ty)
    (hd : decideCand src rs p ty q probes c = some h) : SoundHit src rs p q h := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · rename_i hcan
    have hv : src.canRead p ty c.record = true := by simpa using hcan
    split at hd
    · cases hd
    · rename_i hne
      have hh := Option.some.inj hd
      subst hh
      refine ⟨c.record, ?_, hv, rfl, ?_, ?_⟩
      · exact hc
      · intro h0
        apply hne
        rw [List.isEmpty_iff]
        exact h0
      · intro f hf
        refine ⟨?_, matchedIn_match _ _ _ _ hf⟩
        have hmem := matchedIn_mem _ _ _ _ hf
        have hfilt := (List.mem_filter.mp hmem).2
        simp only [Bool.and_eq_true] at hfilt
        obtain ⟨fv, hfv, hfe⟩ := List.any_eq_true.mp hfilt.2
        have hfe' : fv.1 = f := beq_iff_eq.mp hfe
        subst hfe'
        exact ⟨fv.2, by simpa using hfv⟩

theorem decideRun_sound (S : SearchHit → Prop) (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), (∀ c ∈ cs, ∀ h, ev c = some h → S h) →
      ∀ h ∈ (decideRun ev room cs).1, S h
  | [], _, _, h, hh => by simp [decideRun] at hh
  | c :: cs, room, hev, h, hh => by
    unfold decideRun at hh
    rcases hc : ev c with _ | h0
    · simp only [hc] at hh
      exact decideRun_sound S ev cs room (fun c' hc' h' he' => hev c' (List.mem_cons_of_mem _ hc') h' he') h hh
    · simp only [hc] at hh
      have hS := hev c List.mem_cons_self h0 hc
      by_cases hr : room ≤ 1
      · simp only [hr, ite_true] at hh
        simp at hh
        subst hh
        exact hS
      · simp only [hr, ite_false] at hh
        rcases List.mem_cons.mp hh with h1 | h2
        · subst h1; exact hS
        · exact decideRun_sound S ev cs (room - 1)
            (fun c' hc' h' he' => hev c' (List.mem_cons_of_mem _ hc') h' he') h h2

theorem hits_append_sound (S : SearchHit → Prop) (acc hs : List SearchHit)
    (h1 : ∀ h ∈ acc, S h) (h2 : ∀ h ∈ hs, S h) : ∀ h ∈ acc ++ hs, S h := by
  intro h hh
  rcases List.mem_append.mp hh with h' | h'
  · exact h1 h h'
  · exact h2 h h'

theorem probeType_sound (S : SearchHit → Prop) (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand),
      (∀ h ∈ acc.hits, S h) → (∀ c ∈ rest, ∀ h, ev c = some h → S h) →
      ∀ h ∈ (probeType size ty ev fuel acc rest).hits, S h
  | 0, acc, rest, hacc, _, h, hh => by
    simp only [probeType] at hh
    exact hacc h hh
  | fuel + 1, acc, rest, hacc, hrest, h, hh => by
    unfold probeType at hh
    dsimp only at hh
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true] at hh
      exact hacc h hh
    · simp only [hs0, Bool.false_eq_true, ite_false] at hh
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true] at hh
        exact hacc h hh
      · simp only [hb, ite_false] at hh
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true] at hh
          exact hacc h hh
        · simp only [hf, Bool.false_eq_true, ite_false] at hh
          have hfound : ∀ c ∈ rest.take (min candidatePage (maxCandidates - acc.examined)),
              ∀ h', ev c = some h' → S h' :=
            fun c hc h' he => hrest c (List.mem_of_mem_take hc) h' he
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          have hsound : ∀ h' ∈ hs, S h' := by
            intro h' hh'
            have hm : h' ∈ (decideRun ev (size - acc.hits.length)
                (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
              rw [hdr]; exact hh'
            exact decideRun_sound S ev _ _ hfound h' hm
          rw [hdr] at hh
          dsimp only at hh
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · simp only [hfull, ite_true] at hh
            exact hits_append_sound S acc.hits hs hacc hsound h hh
          · simp only [hfull, ite_false] at hh
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true] at hh
              exact hits_append_sound S acc.hits hs hacc hsound h hh
            · simp only [hl, ite_false] at hh
              exact probeType_sound S size ty ev fuel _ _
                (hits_append_sound S acc.hits hs hacc hsound)
                (fun c hc => hrest c (List.mem_of_mem_drop hc)) h hh

theorem searchTypes_sound (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc),
      (∀ h ∈ acc.hits, SoundHit src rs p q h) →
      ∀ h ∈ (searchTypes src rs p q size plan acc).hits, SoundHit src rs p q h
  | [], acc, hacc, h, hh => by
    simp only [searchTypes] at hh
    exact hacc h hh
  | (ty, probes) :: more, acc, hacc, h, hh => by
    unfold searchTypes at hh
    dsimp only at hh
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true] at hh
      exact hacc h hh
    · simp only [hs0, Bool.false_eq_true, ite_false] at hh
      have hacc' := probeType_sound (SoundHit src rs p q) size ty
        (decideCand src rs p ty q probes) ((candidatesOf src ty q probes).length + 1) acc
        (candidatesOf src ty q probes) hacc
        (fun c hc h' hd => decideCand_sound src rs p ty q probes c h'
          (candidatesOf_mem src ty q probes c hc) hd)
      exact searchTypes_sound src rs p q size more _ hacc' h hh

theorem searchOn_sound (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (plan : List (TypeId × List FieldId)) :
    ∀ h ∈ (searchOn src rs p q size plan).hits, SoundHit src rs p q h := by
  intro h hh
  unfold searchOn at hh
  exact searchTypes_sound src rs p q size plan {} (fun h' hh' => by simp at hh') h hh

theorem searchPlan_query (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (q : List String) (size : Nat) (plan : List (TypeId × List FieldId))
    (h : searchPlan t spec p req = .ok (q, size, plan)) : q = tokensOf req.query := by
  unfold searchPlan at h
  dsimp only at h
  repeat' (split at h)
  all_goals (try simp_all)

/-- Every hit of a search is sound: a stored record the reader may read, given redacted, whose matched fields
    are shown and match the query. -/
theorem search_sound (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hr : search t spec p req = .ok r) :
    ∀ h ∈ r.hits, SoundHit (Src.ofTenant t) spec.rules p (tokensOf req.query) h := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e => rw [hpl] at hr; cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hq := searchPlan_query t spec p req q size plan hpl
    have hr' := Except.ok.inj hr
    subst hr'
    subst hq
    exact searchOn_sound _ _ _ _ _ _

/-! Refusals of a search, in the kernel's order -/

theorem search_query_too_long (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h : maxQueryBytes < req.query.utf8ByteSize) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h]

theorem search_query_no_word (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : tokensOf req.query = []) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2]

theorem search_query_too_many_words (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : maxQueryTokens < (tokensOf req.query).length) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2, h3]

theorem search_page_out_of_range (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : ¬ maxQueryTokens < (tokensOf req.query).length)
    (h4 : req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage) :
    search t spec p req = .error .invalidIntent := by
  unfold search searchPlan
  simp [h1, h2, h3, h4]

theorem search_unknown_object (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (h1 : ¬ maxQueryBytes < req.query.utf8ByteSize) (h2 : ¬ tokensOf req.query = [])
    (h3 : ¬ maxQueryTokens < (tokensOf req.query).length)
    (h4 : ¬ (req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage))
    (h5 : req.objects.any (fun ty => (t.typ? ty).isNone) = true) :
    search t spec p req = .error .unknownObject := by
  unfold search searchPlan
  simp [h1, h2, h3, h4, h5]

/-! The candidates a search examines -/

theorem decideRun_consumed_le (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), (decideRun ev room cs).2 ≤ cs.length
  | [], _ => by simp [decideRun]
  | c :: cs, room => by
    have ih := decideRun_consumed_le ev cs
    unfold decideRun
    rcases hc : ev c with _ | h0
    · simp only [hc]
      have := ih room
      simp only [List.length_cons]
      omega
    · simp only [hc]
      by_cases hr : room ≤ 1
      · simp [hr]
      · simp only [hr, ite_false]
        have := ih (room - 1)
        simp only [List.length_cons]
        omega

theorem probeType_examined_le (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand), acc.examined ≤ maxCandidates →
      (probeType size ty ev fuel acc rest).examined ≤ maxCandidates
  | 0, acc, rest, hacc => by
    simp only [probeType]
    exact hacc
  | fuel + 1, acc, rest, hacc => by
    unfold probeType
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true]
        exact hacc
      · simp only [hb, ite_false]
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true]
          exact hacc
        · simp only [hf, Bool.false_eq_true, ite_false]
          have hlen := List.length_take_le (min candidatePage (maxCandidates - acc.examined)) rest
          have hcons := decideRun_consumed_le ev (rest.take (min candidatePage (maxCandidates - acc.examined)))
            (size - acc.hits.length)
          have hmin := Nat.min_le_right candidatePage (maxCandidates - acc.examined)
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          rw [hdr] at hcons
          dsimp only at hcons ⊢
          have hcon : consumed ≤ maxCandidates - acc.examined := by
            have := hcons
            omega
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · simp only [hfull, ite_true]
            omega
          · simp only [hfull, ite_false]
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true]
              omega
            · simp only [hl, ite_false]
              exact probeType_examined_le size ty ev fuel _ _ (by
                show acc.examined + consumed ≤ maxCandidates
                omega)

theorem searchTypes_examined_le (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc), acc.examined ≤ maxCandidates →
      (searchTypes src rs p q size plan acc).examined ≤ maxCandidates
  | [], acc, hacc => by
    simp only [searchTypes]
    exact hacc
  | (ty, probes) :: more, acc, hacc => by
    unfold searchTypes
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      exact searchTypes_examined_le src rs p q size more _
        (probeType_examined_le size ty (decideCand src rs p ty q probes)
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes) hacc)

/-- A search examines at most `maxCandidates` (1,000) candidates, whatever the reader may read of them. -/
theorem search_examined_le (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hr : search t spec p req = .ok r) : r.examined ≤ maxCandidates := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e => rw [hpl] at hr; cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hr' := Except.ok.inj hr
    subst hr'
    unfold searchOn
    exact searchTypes_examined_le _ _ _ _ _ _ _ (by simp)

/-- Hits in type order, then record id order. -/
def hitLt (a b : SearchHit) : Prop := a.ty < b.ty ∨ (a.ty = b.ty ∧ a.id < b.id)

theorem decideRun_none_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h : ev c = none) :
    decideRun ev room (c :: cs) = ((decideRun ev room cs).1, (decideRun ev room cs).2 + 1) := by
  rw [decideRun, h]

theorem decideRun_some_last_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h0 : SearchHit) (h : ev c = some h0) (hr : room ≤ 1) :
    decideRun ev room (c :: cs) = ([h0], 1) := by
  rw [decideRun, h]
  dsimp only
  rw [if_pos hr]

theorem decideRun_some_more_eq (ev : Cand → Option SearchHit) (c : Cand) (cs : List Cand) (room : Nat)
    (h0 : SearchHit) (h : ev c = some h0) (hr : ¬ room ≤ 1) :
    decideRun ev room (c :: cs) =
      (h0 :: (decideRun ev (room - 1) cs).1, (decideRun ev (room - 1) cs).2 + 1) := by
  rw [decideRun, h]
  dsimp only
  rw [if_neg hr]

theorem decideRun_prefix (ev : Cand → Option SearchHit) :
    ∀ (cs : List Cand) (room : Nat), ∀ h ∈ (decideRun ev room cs).1,
      ∃ d ∈ cs.take (decideRun ev room cs).2, ev d = some h
  | [], _, h, hh => by simp [decideRun] at hh
  | c :: cs, room, h, hh => by
    rcases hc : ev c with _ | h0
    · rw [decideRun_none_eq ev c cs room hc] at hh ⊢
      obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs room h hh
      exact ⟨d, by simpa [List.take_succ_cons] using List.mem_cons_of_mem _ hd, hed⟩
    · by_cases hr : room ≤ 1
      · rw [decideRun_some_last_eq ev c cs room h0 hc hr] at hh ⊢
        simp at hh
        subst hh
        exact ⟨c, by simp, hc⟩
      · rw [decideRun_some_more_eq ev c cs room h0 hc hr] at hh ⊢
        rcases List.mem_cons.mp hh with h1 | h2
        · subst h1
          exact ⟨c, by simp, hc⟩
        · obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs (room - 1) h h2
          exact ⟨d, by simpa [List.take_succ_cons] using List.mem_cons_of_mem _ hd, hed⟩

theorem decideRun_order (ev : Cand → Option SearchHit) (ty : TypeId)
    (hev : ∀ c h, ev c = some h → h.ty = ty ∧ h.id = c.id) :
    ∀ (cs : List Cand) (room : Nat), cs.Pairwise (fun c d => c.id < d.id) →
      (decideRun ev room cs).1.Pairwise hitLt
  | [], _, _ => by simp [decideRun]
  | c :: cs, room, hp => by
    have hp' := List.pairwise_cons.mp hp
    have ih := decideRun_order ev ty hev cs
    rcases hc : ev c with _ | h0
    · rw [decideRun_none_eq ev c cs room hc]
      exact ih room hp'.2
    · have hh0 := hev c h0 hc
      by_cases hr : room ≤ 1
      · rw [decideRun_some_last_eq ev c cs room h0 hc hr]
        simp
      · rw [decideRun_some_more_eq ev c cs room h0 hc hr]
        apply List.Pairwise.cons
        · intro h' hh'
          obtain ⟨d, hd, hed⟩ := decideRun_prefix ev cs (room - 1) h' hh'
          have hd2 := hev d h' hed
          have hdc : c.id < d.id := hp'.1 d (List.mem_of_mem_take hd)
          right
          exact ⟨by rw [hh0.1, hd2.1], by rw [hh0.2, hd2.2]; exact hdc⟩
        · exact ih (room - 1) hp'.2

theorem decideCand_ty (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hd : decideCand src rs p ty q probes c = some h) :
    h.ty = ty := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · split at hd
    · cases hd
    · have hh := Option.some.inj hd
      subst hh
      rfl

theorem sorted_recs_strict (l : List (RecordId × Record)) (hnd : (l.map Prod.fst).Nodup)
    (hle : l.Pairwise (fun a b => a.1 ≤ b.1)) : l.Pairwise (fun a b => a.1 < b.1) := by
  have hne : l.Pairwise (fun a b => a.1 ≠ b.1) := List.pairwise_map.mp hnd
  exact (List.Pairwise.and hle hne).imp (fun ⟨h1, h2⟩ => Nat.lt_of_le_of_ne h1 h2)

/-- The records of a type, sorted by id, as a list with distinct ids: the order a search reads them in. -/
theorem mergeSort_recs_sorted (l : Records) :
    (l.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≤ b.1) := by
  have key := List.pairwise_mergeSort (le := fun (a b : RecordId × Record) => decide (a.1 ≤ b.1))
    (fun a b c h1 h2 => by
      simp only [decide_eq_true_eq] at h1 h2 ⊢
      exact Nat.le_trans h1 h2)
    (fun a b => by
      cases Nat.le_total a.1 b.1 <;> simp_all)
    l
  exact key.imp (fun h => by simpa using h)

theorem candidatesOf_sorted (src : Src) (ty : TypeId) (q : List String) (probes : List FieldId)
    (hnd : ((src.recs ty).map Prod.fst).Nodup) :
    (candidatesOf src ty q probes).Pairwise (fun c d => c.id < d.id) := by
  unfold candidatesOf
  have hs : ((src.recs ty).mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 < b.1) :=
    sorted_recs_strict _ (((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr hnd)
      (mergeSort_recs_sorted _)
  refine List.Pairwise.filterMap _ (fun a b hab b' hb' b'' hb'' => ?_) hs
  cases hfa : matchedIn q probes a.2 with
  | nil => simp [hfa] at hb'
  | cons _ _ =>
    cases hfb : matchedIn q probes b.2 with
    | nil => simp [hfb] at hb''
    | cons _ _ =>
      simp [hfa] at hb'
      simp [hfb] at hb''
      subst hb' hb''
      exact hab

theorem take_take_le {α : Type} (l : List α) (n m : Nat) (h : m ≤ n) :
    (l.take n).take m = l.take m := by
  rw [List.take_take, Nat.min_eq_left h]

theorem take_lt_drop (rest : List Cand) (hs : rest.Pairwise (fun c d => c.id < d.id)) (n : Nat) :
    ∀ d ∈ rest.take n, ∀ c ∈ rest.drop n, d.id < c.id := by
  have hsplit : (rest.take n ++ rest.drop n).Pairwise (fun c d => c.id < d.id) := by
    rw [List.take_append_drop]
    exact hs
  have hp := List.pairwise_append.mp hsplit
  exact fun d hd c hc => hp.2.2 d hd c hc

theorem probeType_order (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit)
    (hev : ∀ c h, ev c = some h → h.ty = ty ∧ h.id = c.id) :
    ∀ (fuel : Nat) (acc : SearchAcc) (rest : List Cand),
      acc.hits.Pairwise hitLt →
      (∀ h ∈ acc.hits, h.ty < ty ∨ (h.ty = ty ∧ ∀ c ∈ rest, h.id < c.id)) →
      rest.Pairwise (fun c d => c.id < d.id) →
      (probeType size ty ev fuel acc rest).hits.Pairwise hitLt := by
  intro fuel
  induction fuel with
  | zero =>
    intro acc rest hacc _ _
    simp only [probeType]
    exact hacc
  | succ fuel ih =>
    intro acc rest hacc hbound hsorted
    unfold probeType
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      by_cases hb : maxCandidates ≤ acc.examined
      · simp only [hb, ite_true]
        exact hacc
      · simp only [hb, ite_false]
        by_cases hf : (rest.take (min candidatePage (maxCandidates - acc.examined))).isEmpty = true
        · simp only [hf, ite_true]
          exact hacc
        · simp only [hf, Bool.false_eq_true, ite_false]
          have hfsorted : (rest.take (min candidatePage (maxCandidates - acc.examined))).Pairwise
              (fun c d => c.id < d.id) := hsorted.sublist (List.take_sublist _ _)
          rcases hdr : decideRun ev (size - acc.hits.length)
              (rest.take (min candidatePage (maxCandidates - acc.examined))) with ⟨hs, consumed⟩
          have hsp : hs.Pairwise hitLt := by
            have := decideRun_order ev ty hev (rest.take (min candidatePage (maxCandidates - acc.examined)))
              (size - acc.hits.length) hfsorted
            rw [hdr] at this
            exact this
          have hcl : consumed ≤ min candidatePage (maxCandidates - acc.examined) := by
            have := decideRun_consumed_le ev (rest.take (min candidatePage (maxCandidates - acc.examined)))
              (size - acc.hits.length)
            rw [hdr] at this
            have hlen := List.length_take_le (min candidatePage (maxCandidates - acc.examined)) rest
            simp only at this
            omega
          have hcross : ∀ a ∈ acc.hits, ∀ b ∈ hs, hitLt a b := by
            intro a ha b hbm
            have hbmem : b ∈ (decideRun ev (size - acc.hits.length)
                (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
              rw [hdr]
              exact hbm
            obtain ⟨d, hd, hed⟩ := decideRun_prefix ev _ _ b hbmem
            have hdrest : d ∈ rest := List.mem_of_mem_take (List.mem_of_mem_take hd)
            have hbt := hev d b hed
            rcases hbound a ha with h | h
            · left
              rw [hbt.1]
              exact h
            · right
              exact ⟨by rw [hbt.1, h.1], by rw [hbt.2]; exact h.2 d hdrest⟩
          have hacc' : (acc.hits ++ hs).Pairwise hitLt := List.pairwise_append.mpr ⟨hacc, hsp, hcross⟩
          by_cases hfull : size ≤ (acc.hits ++ hs).length
          · dsimp only
            simp only [hfull, ite_true]
            exact hacc'
          · dsimp only
            simp only [hfull, ite_false]
            by_cases hl : (rest.take (min candidatePage (maxCandidates - acc.examined))).length <
                min candidatePage (maxCandidates - acc.examined)
            · simp only [hl, ite_true]
              exact hacc'
            · simp only [hl, ite_false]
              have hbound'' : ∀ h ∈ acc.hits ++ hs, h.ty < ty ∨
                  (h.ty = ty ∧ ∀ c ∈ rest.drop consumed, h.id < c.id) := by
                intro h hh
                rcases List.mem_append.mp hh with ha | hbm
                · rcases hbound h ha with h1 | h1
                  · exact Or.inl h1
                  · exact Or.inr ⟨h1.1, fun c hc => h1.2 c (List.mem_of_mem_drop hc)⟩
                · have hbmem : h ∈ (decideRun ev (size - acc.hits.length)
                      (rest.take (min candidatePage (maxCandidates - acc.examined)))).1 := by
                    rw [hdr]
                    exact hbm
                  obtain ⟨d, hd, hed⟩ := decideRun_prefix ev _ _ h hbmem
                  have hd2 : d ∈ (rest.take (min candidatePage (maxCandidates - acc.examined))).take consumed := by
                    have hcopy := hd
                    rw [hdr] at hcopy
                    exact hcopy
                  rw [take_take_le rest _ consumed hcl] at hd2
                  have hbt := hev d h hed
                  exact Or.inr ⟨hbt.1, fun c hc => by
                    rw [hbt.2]
                    exact take_lt_drop rest hsorted consumed d hd2 c hc⟩
              exact ih _ (rest.drop consumed) hacc' hbound'' (hsorted.sublist (List.drop_sublist _ _))

theorem decideCand_id (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) (h : SearchHit) (hd : decideCand src rs p ty q probes c = some h) :
    h.id = c.id := by
  unfold decideCand at hd
  dsimp only at hd
  split at hd
  · cases hd
  · split at hd
    · cases hd
    · have hh := Option.some.inj hd
      subst hh
      rfl

theorem searchTypes_order (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (hnd : ∀ ty, ((src.recs ty).map Prod.fst).Nodup) :
    ∀ (plan : List (TypeId × List FieldId)) (acc : SearchAcc),
      plan.Pairwise (fun a b => a.1 < b.1) → acc.hits.Pairwise hitLt →
      (∀ h ∈ acc.hits, ∀ e ∈ plan, h.ty < e.1) →
      (searchTypes src rs p q size plan acc).hits.Pairwise hitLt
  | [], acc, _, hacc, _ => by
    simp only [searchTypes]
    exact hacc
  | (ty, probes) :: more, acc, hplan, hacc, hbound => by
    have hpl := List.pairwise_cons.mp hplan
    unfold searchTypes
    dsimp only
    by_cases hs0 : acc.stop = true
    · simp only [hs0, ite_true]
      exact hacc
    · simp only [hs0, Bool.false_eq_true, ite_false]
      have hpt := probeType_order size ty (decideCand src rs p ty q probes)
        (fun c h hd => ⟨decideCand_ty src rs p ty q probes c h hd, decideCand_id src rs p ty q probes c h hd⟩)
        ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes) hacc
        (fun h hh => Or.inl (hbound h hh (ty, probes) List.mem_cons_self))
        (candidatesOf_sorted src ty q probes (hnd ty))
      have hsome : ∀ h ∈ (probeType size ty (decideCand src rs p ty q probes)
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes)).hits,
          h ∈ acc.hits ∨ h.ty = ty := by
        intro h hh
        exact probeType_sound (fun h => h ∈ acc.hits ∨ h.ty = ty) size ty _
          ((candidatesOf src ty q probes).length + 1) acc (candidatesOf src ty q probes)
          (fun h hh => Or.inl hh)
          (fun c _ h hd => Or.inr (decideCand_ty src rs p ty q probes c h hd)) h hh
      exact searchTypes_order src rs p q size hnd more _ hpl.2 hpt (by
        intro h hh e he
        rcases hsome h hh with h1 | h1
        · exact Nat.lt_trans (hbound h h1 (ty, probes) List.mem_cons_self) (hpl.1 e he)
        · rw [h1]
          exact hpl.1 e he)

/-- The object types of a plan are in ascending order, when their ids are distinct. -/
theorem types_sorted_strict (types : List (TypeId × ObjType)) (hnd : (types.map Prod.fst).Nodup) :
    (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 < b.1) := by
  have hle : (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≤ b.1) := by
    have key := List.pairwise_mergeSort (le := fun (a b : TypeId × ObjType) => decide (a.1 ≤ b.1))
      (fun a b c h1 h2 => by
        simp only [decide_eq_true_eq] at h1 h2 ⊢
        exact Nat.le_trans h1 h2)
      (fun a b => by
        cases Nat.le_total a.1 b.1 <;> simp_all)
      types
    exact key.imp (fun h => by simpa using h)
  have hne : (types.mergeSort (fun a b => decide (a.1 ≤ b.1))).Pairwise (fun a b => a.1 ≠ b.1) :=
    List.pairwise_map.mp (((List.mergeSort_perm _ _).map Prod.fst).nodup_iff.mpr hnd)
  exact (List.Pairwise.and hle hne).imp (fun ⟨h1, h2⟩ => Nat.lt_of_le_of_ne h1 h2)

theorem searchPlan_sorted (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq)
    (q : List String) (size : Nat) (plan : List (TypeId × List FieldId))
    (hkeys : (t.types.map Prod.fst).Nodup)
    (h : searchPlan t spec p req = .ok (q, size, plan)) :
    plan.Pairwise (fun a b => a.1 < b.1) := by
  unfold searchPlan at h
  dsimp only at h
  by_cases c1 : maxQueryBytes < req.query.utf8ByteSize
  · simp only [c1, ite_true] at h
    cases h
  simp only [c1, ite_false] at h
  by_cases c2 : tokensOf req.query = []
  · simp only [c2, ite_true] at h
    cases h
  simp only [c2, ite_false] at h
  by_cases c3 : maxQueryTokens < (tokensOf req.query).length
  · simp only [c3, ite_true] at h
    cases h
  simp only [c3, ite_false] at h
  by_cases c4 : req.page.getD defaultPage = 0 ∨ maxPage < req.page.getD defaultPage
  · simp only [c4, ite_true] at h
    cases h
  simp only [c4, ite_false] at h
  by_cases c5 : req.objects.any (fun ty => (t.typ? ty).isNone) = true
  · simp only [c5, ite_true] at h
    cases h
  simp only [c5, ite_false] at h
  have hh := Except.ok.inj h
  simp only [Prod.mk.injEq] at hh
  rw [← hh.2.2]
  refine List.Pairwise.filterMap _ (fun a b hab b' hb' b'' hb'' => ?_) (types_sorted_strict t.types hkeys)
  repeat' (split at hb')
  repeat' (split at hb'')
  all_goals (try simp only [Option.mem_def, Option.some.injEq] at hb' hb'')
  all_goals (try subst hb')
  all_goals (try subst hb'')
  all_goals (try exact hab)
  all_goals (try simp_all)

theorem searchOn_order (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (plan : List (TypeId × List FieldId)) (hplan : plan.Pairwise (fun a b => a.1 < b.1))
    (hnd : ∀ ty, ((src.recs ty).map Prod.fst).Nodup) :
    (searchOn src rs p q size plan).hits.Pairwise hitLt := by
  unfold searchOn
  exact searchTypes_order src rs p q size hnd plan {} hplan List.Pairwise.nil
    (fun h hh => absurd hh (by simp))

/-- The hits of a search are in ascending type, then record id order. -/
theorem search_order (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hkeys : (t.types.map Prod.fst).Nodup) (hids : ∀ ty, ((t.recordsOf ty).map Prod.fst).Nodup)
    (hr : search t spec p req = .ok r) : r.hits.Pairwise hitLt := by
  unfold search at hr
  cases hpl : searchPlan t spec p req with
  | error e =>
    rw [hpl] at hr
    cases hr
  | ok v =>
    rw [hpl] at hr
    obtain ⟨q, size, plan⟩ := v
    have hr' := Except.ok.inj hr
    subst hr'
    exact searchOn_order _ _ _ _ _ _ (searchPlan_sorted t spec p req q size plan hkeys hpl) hids

/-- No record is a hit twice: the hits, as type and id pairs, have no repeats. -/
theorem search_no_repeat (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) (r : SearchResult)
    (hkeys : (t.types.map Prod.fst).Nodup) (hids : ∀ ty, ((t.recordsOf ty).map Prod.fst).Nodup)
    (hr : search t spec p req = .ok r) : (r.hits.map (fun h => (h.ty, h.id))).Nodup := by
  have hp := search_order t spec p req r hkeys hids hr
  unfold List.Nodup
  rw [List.pairwise_map]
  refine hp.imp (fun {a b} hab heq => ?_)
  have h1 := Prod.mk.inj heq
  rcases hab with h2 | h2
  · rw [h1.1] at h2
    exact Nat.lt_irrefl _ h2
  · rw [h1.2] at h2
    exact Nat.lt_irrefl _ h2.2

end Strata

namespace Strata
/-! ## Aggregate refusals -/

theorem aggregate_unknown_query (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (h : spec.queries.lookup req.query = none) :
    aggregate t spec p req = .error .unknownQuery := by
  unfold aggregate
  simp only [h]

theorem aggregate_unknown_aggregate (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = none) :
    aggregate t spec p req = .error .unknownQuery := by
  unfold aggregate
  simp only [hq, ha]

theorem aggregate_undeclared_param (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : req.params.any (fun kv => !(q.params.contains kv.1)) = true) :
    aggregate t spec p req = .error .invalidIntent := by
  unfold aggregate
  simp only [hq, ha, hp, ite_true]

theorem aggregate_unknown_object (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = none) :
    aggregate t spec p req = .error .unknownObject := by
  unfold aggregate
  simp only [hq, ha, hp, ite_false, ht, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_undeclared_filter (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : req.filter.any (fun kv => !(q.declared.contains kv.1)) = true) :
    aggregate t spec p req = .error .invalidIntent := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, ite_true, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_unknown_field (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : ¬ req.filter.any (fun kv => !(q.declared.contains kv.1)) = true)
    (hn : ((req.filter.map Prod.fst) ++ a.field.toList ++ a.groupBy.toList).any
      (fun f => (o.schema.lookup f).isNone) = true) :
    aggregate t spec p req = .error .unknownField := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, hn, ite_true, Bool.false_eq_true, ↓reduceIte]

theorem aggregate_scan_window (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (hq : spec.queries.lookup req.query = some q)
    (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true)
    (ht : t.typ? q.ty = some o)
    (hf : ¬ req.filter.any (fun kv => !(q.declared.contains kv.1)) = true)
    (hn : ¬ ((req.filter.map Prod.fst) ++ a.field.toList ++ a.groupBy.toList).any
      (fun f => (o.schema.lookup f).isNone) = true)
    (hw : maxScan < (t.recordsOf q.ty).length) :
    aggregate t spec p req = .error (.limitExceeded .scanBound) := by
  unfold aggregate aggregateOn
  simp only [hq, ha, hp, ite_false, ht, hf, hn, Src.ofTenant, Bool.false_eq_true, ↓reduceIte]
  simp only [hw, ite_true, Bool.false_eq_true, ↓reduceIte]

/-! ## Redaction and the filters of queries -/

/-- The fields an expression reads from a record. -/
def fieldsRead : Expr → List FieldId
  | .lit _ => []
  | .field _ f => [f]
  | .createdBy _ => []
  | .principal => []
  | .cmp _ a b => fieldsRead a ++ fieldsRead b
  | .and a b => fieldsRead a ++ fieldsRead b
  | .or a b => fieldsRead a ++ fieldsRead b
  | .not a => fieldsRead a
  | .isNull a => fieldsRead a

theorem lookup_filter (f : FieldId) (P : FieldId × Val → Bool) :
    ∀ (fs : Fields), (∀ fv ∈ fs, fv.1 = f → P fv = true) → (fs.filter P).lookup f = fs.lookup f
  | [], _ => rfl
  | (a, b) :: rest, h => by
    have ih := lookup_filter f P rest (fun fv hfv hf => h fv (List.mem_cons_of_mem _ hfv) hf)
    by_cases hP : P (a, b) = true
    · rw [List.filter_cons_of_pos hP]
      by_cases ha : a = f
      · subst ha
        simp [List.lookup]
      · have hne : (a == f) = false := by simpa using ha
        have hne2 : (f == a) = false := by simpa [beq_iff_eq] using Ne.symm ha
        simp [List.lookup, hne, hne2, ih]
    · have hfa : a ≠ f := by
        intro ha
        subst ha
        exact absurd (h (a, b) List.mem_cons_self rfl) (by simpa using hP)
      have hne : (f == a) = false := by simpa [beq_iff_eq] using Ne.symm hfa
      rw [List.filter_cons_of_neg (by simpa using hP)]
      simp [List.lookup, hne, ih]

theorem fields_get_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) (f : FieldId)
    (hf : shownField rs p ty r f = true) :
    Fields.get (redact rs p ty r).fields f = Fields.get r.fields f := by
  unfold Fields.get redact
  simp only
  rw [lookup_filter f (fun fv => shownField rs p ty r fv.1)]
  intro fv _ hfv
  rw [hfv]
  exact hf

theorem evalExpr_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) :
    ∀ (e : Expr), (∀ f ∈ fieldsRead e, shownField rs p ty r f = true) →
      evalExpr (readEnv p (redact rs p ty r)) e = evalExpr (readEnv p r) e
  | .lit _, _ => rfl
  | .field s f, h => by
    have hf : shownField rs p ty r f = true := h f (by simp [fieldsRead])
    cases s <;> simp only [evalExpr, readEnv, ExprEnv.scoped, fields_get_redact rs p ty r f hf]
  | .createdBy s, _ => by cases s <;> simp [evalExpr, readEnv, ExprEnv.scoped, redact]
  | .principal, _ => rfl
  | .cmp op a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .and a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .or a b, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    have hb := evalExpr_redact rs p ty r b (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha, hb]
  | .not a, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha]
  | .isNull a, h => by
    have ha := evalExpr_redact rs p ty r a (fun f hf => h f (by simp [fieldsRead, hf]))
    simp only [evalExpr, ha]

/-- A filter whose fields carry no read rule holds on a redacted record exactly when it holds on the stored
    one: the filter of a named query reads the same values in both. -/
theorem condHolds_redact (rs : FieldRules) (p : Principal) (ty : TypeId) (r : Record) (e : Expr)
    (hf : ∀ f ∈ fieldsRead e, rs.rule ty f = none) :
    condHolds (readEnv p (redact rs p ty r)) (some e) = condHolds (readEnv p r) (some e) := by
  have hshown : ∀ f ∈ fieldsRead e, shownField rs p ty r f = true := by
    intro f hfe
    cases p with
    | system => rfl
    | user n => simp [shownField, hf f hfe]
    | anonymous => simp [shownField, hf f hfe]
  simp only [condHolds, evalExpr_redact rs p ty r e hshown]

theorem length_filterMap_eq_filter {α β : Type} (f : α → Option β) (g : α → Bool) :
    ∀ (L : List α), (∀ a ∈ L, (f a).isSome = g a) → (L.filterMap f).length = (L.filter g).length
  | [], _ => rfl
  | a :: L, h => by
    have ih := length_filterMap_eq_filter f g L (fun x hx => h x (List.mem_cons_of_mem _ hx))
    have ha := h a List.mem_cons_self
    simp only [List.filterMap_cons, List.filter_cons]
    cases hfa : f a <;> cases hga : g a <;> simp_all

theorem selectedBy_plain (src : Src) (spec : ReadSpec) (p : Principal) (q : QueryDef) (req : AggReq)
    (hf : req.filter = []) (e : RecordId × Record) :
    selectedBy src spec p q req [] e = (src.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter) := by
  unfold selectedBy
  cases p <;> simp [hf]

/-- The records an aggregate with no named field and no request filter selects, in table order. -/
def plainSelected (src : Src) (spec : ReadSpec) (p : Principal) (q : QueryDef) (req : AggReq) :
    List (RecordId × Record) :=
  (src.recs q.ty).filter (selectedBy src spec p q req [])

theorem aggregateOn_count_form (src : Src) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType)
    (hk : a.kind = .count) (hfe : req.filter = []) (hfield : a.field = none) (hg : a.groupBy = none) :
    aggregateOn src spec p req q a o =
      (if maxScan < (src.recs q.ty).length then .error (.limitExceeded .scanBound)
       else if min a.scanBound maxScan < (plainSelected src spec p q req).length
         then .error (.limitExceeded .scanBound)
       else .ok { groups := [{ key := none, value := some (.int ((plainSelected src spec p q req).length : Int)),
                               rows := [] }],
                  scanned := (plainSelected src spec p q req).length }) := by
  unfold aggregateOn plainSelected
  simp only [hfe, hfield, hg, List.map_nil, List.nil_append, Option.toList,
    List.any_nil, Bool.false_eq_true, ite_false, hk, finish, Except.map, List.map_map,
    List.length_map, List.length_singleton, maxAggGroups, Nat.reduceLT, ↓reduceIte]


theorem queryAll_length_eq (t : Tenant) (spec : ReadSpec) (p : Principal) (q : QueryDef)
    (hfilt : ∀ e, q.filter = some e → ∀ f ∈ fieldsRead e, spec.rules.rule q.ty f = none) :
    (queryAll t spec p q).length =
      ((t.recordsOf q.ty).filter (fun e => t.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter)).length := by
  unfold queryAll
  rw [(List.mergeSort_perm _ _).length_eq]
  unfold scanRows
  apply length_filterMap_eq_filter
  intro e _
  by_cases hc : t.canRead p q.ty e.2 = true
  · have hcond : condHolds (readEnv p (redact spec.rules p q.ty e.2)) q.filter =
        condHolds (readEnv p e.2) q.filter := by
      cases hq : q.filter with
      | none => rfl
      | some fe => exact condHolds_redact spec.rules p q.ty e.2 fe (hfilt fe hq)
    cases hcc : condHolds (readEnv p e.2) q.filter <;> simp [hc, hcond, hcc]
  · simp [hc]

theorem aggregate_count_queryAll (t : Tenant) (spec : ReadSpec) (p : Principal) (req : AggReq)
    (q : QueryDef) (a : AggDef) (o : ObjType) (r : AggResult)
    (hq : spec.queries.lookup req.query = some q) (ha : q.aggregates.lookup req.aggregate = some a)
    (hp : ¬ req.params.any (fun kv => !(q.params.contains kv.1)) = true) (ht : t.typ? q.ty = some o)
    (hk : a.kind = .count) (hfe : req.filter = []) (hfield : a.field = none) (hg : a.groupBy = none)
    (hfilt : ∀ e, q.filter = some e → ∀ f ∈ fieldsRead e, spec.rules.rule q.ty f = none)
    (hr : aggregate t spec p req = .ok r) :
    r.groups = [{ key := none, value := some (.int ((queryAll t spec p q).length : Int)), rows := [] }] ∧
      r.scanned = (queryAll t spec p q).length := by
  unfold aggregate at hr
  simp only [hq, ha, hp, ite_false, ht] at hr
  rw [aggregateOn_count_form (Src.ofTenant t) spec p req q a o hk hfe hfield hg] at hr
  by_cases hW : maxScan < ((Src.ofTenant t).recs q.ty).length
  · simp [hW] at hr
  · by_cases hB : min a.scanBound maxScan < (plainSelected (Src.ofTenant t) spec p q req).length
    · simp [hW, hB] at hr
    · simp only [hW, hB, ite_false] at hr
      have hr' := Except.ok.inj hr
      subst hr'
      have hsel : plainSelected (Src.ofTenant t) spec p q req =
          (t.recordsOf q.ty).filter (fun e => t.canRead p q.ty e.2 && condHolds (readEnv p e.2) q.filter) := by
        unfold plainSelected
        apply List.filter_congr
        intro e _
        rw [selectedBy_plain _ _ _ _ _ hfe]
        rfl
      rw [queryAll_length_eq t spec p q hfilt, ← hsel]
      exact ⟨rfl, rfl⟩

end Strata

-- Axioms of the slice 10 search and aggregate theorems
#print axioms Strata.SoundHit
#print axioms Strata.aggregateOn_count_form
#print axioms Strata.aggregate_count_queryAll
#print axioms Strata.aggregate_scan_window
#print axioms Strata.aggregate_undeclared_filter
#print axioms Strata.aggregate_undeclared_param
#print axioms Strata.aggregate_unknown_aggregate
#print axioms Strata.aggregate_unknown_field
#print axioms Strata.aggregate_unknown_object
#print axioms Strata.aggregate_unknown_query
#print axioms Strata.candidatesOf_mem
#print axioms Strata.candidatesOf_sorted
#print axioms Strata.condHolds_redact
#print axioms Strata.decideCand_id
#print axioms Strata.decideCand_sound
#print axioms Strata.decideCand_ty
#print axioms Strata.decideRun_consumed_le
#print axioms Strata.decideRun_none_eq
#print axioms Strata.decideRun_order
#print axioms Strata.decideRun_prefix
#print axioms Strata.decideRun_some_last_eq
#print axioms Strata.decideRun_some_more_eq
#print axioms Strata.decideRun_sound
#print axioms Strata.evalExpr_redact
#print axioms Strata.fieldsRead
#print axioms Strata.fields_get_redact
#print axioms Strata.hitLt
#print axioms Strata.hits_append_sound
#print axioms Strata.length_filterMap_eq_filter
#print axioms Strata.lookup_filter
#print axioms Strata.matchedIn_match
#print axioms Strata.matchedIn_mem
#print axioms Strata.mergeSort_recs_sorted
#print axioms Strata.plainSelected
#print axioms Strata.probeType_examined_le
#print axioms Strata.probeType_order
#print axioms Strata.probeType_sound
#print axioms Strata.queryAll_length_eq
#print axioms Strata.searchOn_order
#print axioms Strata.searchOn_sound
#print axioms Strata.searchPlan_query
#print axioms Strata.searchPlan_sorted
#print axioms Strata.searchTypes_examined_le
#print axioms Strata.searchTypes_order
#print axioms Strata.searchTypes_sound
#print axioms Strata.search_examined_le
#print axioms Strata.search_no_repeat
#print axioms Strata.search_order
#print axioms Strata.search_page_out_of_range
#print axioms Strata.search_query_no_word
#print axioms Strata.search_query_too_long
#print axioms Strata.search_query_too_many_words
#print axioms Strata.search_sound
#print axioms Strata.search_unknown_object
#print axioms Strata.selectedBy_plain
#print axioms Strata.sorted_recs_strict
#print axioms Strata.take_lt_drop
#print axioms Strata.take_take_le
#print axioms Strata.types_sorted_strict

-- Slice 11: the schema change in general (Strata/SchemaChange.lean, Strata/SchemaChangeInv.lean, Strata/SchemaChangeProofs.lean, Strata/Impl/SchemaI.lean).
#print axioms Strata.changeSchemaT_unauthorized
#print axioms Strata.changeSchemaT_conflict
#print axioms Strata.changeSchemaT_empty
#print axioms Strata.changeSchemaT_limit
#print axioms Strata.applyChanges_cons
#print axioms Strata.changeSchemaT_ok
#print axioms Strata.applyChanges_valid
#print axioms Strata.changeSchemaT_valid
#print axioms Strata.changeSchemaT_rules
#print axioms Strata.changeSchemaT_strict_not_violated
#print axioms Strata.changeSchemaT_strict_conforms
#print axioms Strata.applyChanges_singleton
#print axioms Strata.lookup_putTypeList_self
#print axioms Strata.removeField_drops
#print axioms Strata.removeType_refused_with_records
#print axioms Strata.removeType_ok_empty
#print axioms Strata.lookup_filter_removed
#print axioms Strata.removeType_deleted_gone
#print axioms Strata.changeSchemaT_removeField
#print axioms Strata.changeSchemaT_parts
#print axioms Strata.stepTenant_changeSchema_parts
#print axioms Strata.chRel_applyChanges
#print axioms Strata.changeSchema_ctx
#print axioms Strata.changeSchema_distinct
#print axioms Strata.changeSchema_idsBelow
#print axioms Strata.changeSchema_states
#print axioms Strata.changeSchema_satisfied
#print axioms Strata.changeSchema_refs
#print axioms Strata.changeSchema_table
#print axioms Strata.changeSchemaT_types_congr
#print axioms Strata.schemaRulesOk_entry
#print axioms Strata.impactRefused_removed
#print axioms Strata.schemaRulesOk_rewrite
#print axioms Strata.stripRecord_get
#print axioms Strata.stripRecord_declared
#print axioms Strata.evalExpr_storedEnv_congr
#print axioms Strata.constraintProblem_storedEnv_congr
#print axioms Strata.defineType_existing
#print axioms Strata.setRequired_schema_declared
#print axioms Strata.setRequired_schema_absent
#print axioms Strata.setUnique_schema_declared
#print axioms Strata.setUnique_schema_absent
#print axioms Strata.applyChange_defineField_putType
#print axioms Strata.Impl.applyChangesI_abs
#print axioms Strata.Impl.rewriteTypesI_abs
#print axioms Strata.Impl.changeSchemaI_refines
#print axioms Strata.Impl.changeSchemaI_inv
#print axioms Strata.Impl.XTenant.rebuild_ok
#print axioms Strata.Impl.changeSchema_good
#print axioms Strata.Codec.lawful_change
#print axioms Strata.Codec.fix_intent_changeSchema
#print axioms Strata.Codec.fix_err_schemaConflict



/-! ## Slice 12: settings (`lean/Strata/Settings.lean`, the intent `writeSetting`, `lean/Main.lean`)

Resolution (`resolveWith`), the write (`writeSettingT`) with its refusals in the kernel's check order, the change an
accepted write makes, its event, and the refused write's no-transition. -/

namespace Strata

/-- The definition a lookup finds has the name the lookup asked for. -/
theorem settingDef?_name (t : Tenant) (n : String) (d : SettingDef) (h : t.settingDef? n = some d) :
    d.name = n := by
  unfold Tenant.settingDef? at h
  split at h
  · rename_i d' hf
    cases h
    have := List.find?_some hf
    simpa using this
  · rename_i hf
    have := List.find?_some h
    simpa using this

/-- Resolution is the value of the most specific open level that holds an override, else the platform's, else the
    default: `resolveWith` computes exactly this (the equation the other resolution theorems use). -/
theorem resolve_value_eq (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String) (d : SettingDef)
    (hd : t.settingDef? n = some d) :
    (resolveWith t pv p n).map (fun r => r.value) =
      .ok (((pickOverride t pv d p).getD (.default, none, d.default)).2.2) := by
  unfold resolveWith
  rw [hd]
  rfl

/-- A person's own open override is what the person reads. -/
theorem resolve_person (t : Tenant) (pv : PlatformValues) (u : PrincipalId) (n : String) (d : SettingDef)
    (o : SettingOverride) (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true)
    (ho : t.overrideOf? n (.person u) = some o) :
    (resolveWith t pv (.user u) n).map (fun r => r.value) = .ok o.value := by
  obtain rfl := settingDef?_name t n d hd
  rw [resolve_value_eq t pv (.user u) _ d hd]
  simp [pickOverride, hopen, ho]

/-- With no open person override, the first team (in the kernel's order) that holds an open override is read. -/
theorem resolve_team (t : Tenant) (pv : PlatformValues) (u : PrincipalId) (n : String) (d : SettingDef)
    (tid : TeamId) (o : SettingOverride) (hd : t.settingDef? n = some d)
    (hP : d.isOpen .person = false ∨ t.overrideOf? n (.person u) = none)
    (hopen : d.isOpen .team = true) (ht : t.teamOverride? n u = some (tid, o)) :
    (resolveWith t pv (.user u) n).map (fun r => r.value) = .ok o.value := by
  obtain rfl := settingDef?_name t n d hd
  rw [resolve_value_eq t pv (.user u) _ d hd]
  rcases hP with hP | hP <;> simp [pickOverride, hP, hopen, ht]

/-- With no open person or team override, the tenant's open override is read. -/
theorem resolve_tenant (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String) (d : SettingDef)
    (o : SettingOverride) (hd : t.settingDef? n = some d)
    (hP : ∀ u, p = .user u → d.isOpen .person = false ∨ t.overrideOf? n (.person u) = none)
    (hT : ∀ u, p = .user u → d.isOpen .team = false ∨ t.teamOverride? n u = none)
    (hopen : d.isOpen .tenant = true) (ho : t.overrideOf? n .tenant = some o) :
    (resolveWith t pv p n).map (fun r => r.value) = .ok o.value := by
  obtain rfl := settingDef?_name t n d hd
  rw [resolve_value_eq t pv p _ d hd]
  cases p with
  | system => simp [pickOverride, hopen, ho]
  | anonymous => simp [pickOverride, hopen, ho]
  | user u =>
    rcases hP u rfl with hP' | hP' <;> rcases hT u rfl with hT' | hT' <;>
      simp [pickOverride, hP', hT', hopen, ho]

/-- With no open override at the person, team or tenant level, the platform's value is read, when the definition opens
    the platform level and the value is of its type and within its limits. -/
theorem resolve_platform (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String) (d : SettingDef)
    (v : Val) (hd : t.settingDef? n = some d)
    (hP : ∀ u, p = .user u → d.isOpen .person = false ∨ t.overrideOf? n (.person u) = none)
    (hT : ∀ u, p = .user u → d.isOpen .team = false ∨ t.teamOverride? n u = none)
    (hTn : d.isOpen .tenant = false ∨ t.overrideOf? n .tenant = none)
    (hopen : d.isOpen .platform = true) (hpv : pv n = some v) (hty : d.ty.admits v = true)
    (hlim : Val.overLimit t.limits v = false) :
    (resolveWith t pv p n).map (fun r => r.value) = .ok v := by
  obtain rfl := settingDef?_name t n d hd
  rw [resolve_value_eq t pv p _ d hd]
  cases p with
  | system => rcases hTn with h | h <;> simp [pickOverride, h, hopen, hpv, hty, hlim]
  | anonymous => rcases hTn with h | h <;> simp [pickOverride, h, hopen, hpv, hty, hlim]
  | user u =>
    rcases hP u rfl with hP' | hP' <;> rcases hT u rfl with hT' | hT' <;>
      rcases hTn with hTn' | hTn' <;> simp [pickOverride, hP', hT', hTn', hopen, hpv, hty, hlim]

/-- With no override and no platform value, the default is read. -/
theorem resolve_default (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String) (d : SettingDef)
    (hd : t.settingDef? n = some d)
    (hP : ∀ u, p = .user u → d.isOpen .person = false ∨ t.overrideOf? n (.person u) = none)
    (hT : ∀ u, p = .user u → d.isOpen .team = false ∨ t.teamOverride? n u = none)
    (hTn : d.isOpen .tenant = false ∨ t.overrideOf? n .tenant = none)
    (hPl : d.isOpen .platform = false ∨ pv n = none) :
    (resolveWith t pv p n).map (fun r => r.value) = .ok d.default := by
  obtain rfl := settingDef?_name t n d hd
  rw [resolve_value_eq t pv p _ d hd]
  cases p with
  | system => rcases hTn with h | h <;> rcases hPl with h' | h' <;> simp [pickOverride, h, h']
  | anonymous => rcases hTn with h | h <;> rcases hPl with h' | h' <;> simp [pickOverride, h, h']
  | user u =>
    rcases hP u rfl with hP' | hP' <;> rcases hT u rfl with hT' | hT' <;>
      rcases hTn with hTn' | hTn' <;> rcases hPl with hPl' | hPl' <;>
      simp [pickOverride, hP', hT', hTn', hPl']

/-- A level the definition closes is not read: a tenant override under a lock that closes the tenant level leaves the
    default in place (the banner of the fixture, at the person level). -/
theorem resolve_closed_level_ignored (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String)
    (d : SettingDef) (o : SettingOverride) (hd : t.settingDef? n = some d)
    (hP : ∀ u, p = .user u → d.isOpen .person = false ∨ t.overrideOf? n (.person u) = none)
    (hT : ∀ u, p = .user u → d.isOpen .team = false ∨ t.teamOverride? n u = none)
    (hclosed : d.isOpen .tenant = false) (hPl : d.isOpen .platform = false ∨ pv n = none)
    (ho : t.overrideOf? n .tenant = some o) :
    (resolveWith t pv p n).map (fun r => r.value) = .ok d.default :=
  resolve_default t pv p n d hd hP hT (Or.inl hclosed) hPl

/-- The shape of an accepted core write: its subject is the one the write names, and the overrides are the ones before,
    with the write's own key replaced, the new override last. -/
theorem writeSettingCore_ok_subject {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat}
    (h : writeSettingCore t (.user u) n lvl team v e = .ok t') :
    ∃ subj, subjectOf t u lvl team = .ok subj := by
  cases hs : subjectOf t u lvl team with
  | error err =>
    exfalso
    unfold writeSettingCore at h
    simp only [hs] at h
    repeat' split at h
    all_goals simp_all
  | ok subj => exact ⟨subj, rfl⟩

end Strata

namespace Strata

/-- The decision view keeps the fields a write reads: these hold by reduction. -/
@[simp] theorem decisionView_holdsCap (t : Tenant) (p : Principal) (c : CapabilityId) :
    t.decisionView.holdsCap p c = t.holdsCap p c := rfl

@[simp] theorem decisionView_settingDef? (t : Tenant) (n : String) :
    t.decisionView.settingDef? n = t.settingDef? n := rfl

@[simp] theorem decisionView_overrideOf? (t : Tenant) (n : String) (s : SettingSubject) :
    t.decisionView.overrideOf? n s = t.overrideOf? n s := rfl

@[simp] theorem decisionView_subjectOf (t : Tenant) (u : PrincipalId) (lvl : SettingLevel) (team : Option TeamId) :
    subjectOf t.decisionView u lvl team = subjectOf t u lvl team := rfl

@[simp] theorem decisionView_teams (t : Tenant) : t.decisionView.teams = t.teams := rfl

@[simp] theorem decisionView_limits (t : Tenant) : t.decisionView.limits = t.limits := rfl

@[simp] theorem decisionView_overrides (t : Tenant) : t.decisionView.overrides = t.overrides := rfl

@[simp] theorem decisionView_settingRev (t : Tenant) (n : String) : t.decisionView.settingRev n = t.settingRev n := rfl

end Strata

namespace Strata

/-- Only a signed-in person writes a setting: the system and an anonymous caller are unauthorized, first of all. -/
theorem write_system_refused (t : Tenant) (n : String) (lvl : SettingLevel) (team : Option TeamId)
    (v : Option Val) (e : Nat) : writeSettingT t .system n lvl team v e = .error .unauthorized := by
  simp [writeSettingT, writeSettingCore] <;> rfl

theorem write_anonymous_refused (t : Tenant) (n : String) (lvl : SettingLevel) (team : Option TeamId)
    (v : Option Val) (e : Nat) : writeSettingT t .anonymous n lvl team v e = .error .unauthorized := by
  simp [writeSettingT, writeSettingCore] <;> rfl

/-- The platform level is the control plane's to write: a tenant's write at it is invalidIntent. -/
theorem write_platform_refused (t : Tenant) (u : PrincipalId) (n : String) (team : Option TeamId)
    (v : Option Val) (e : Nat) : writeSettingT t (.user u) n .platform team v e = .error .invalidIntent := by
  simp [writeSettingT, writeSettingCore] <;> rfl

/-- A tenant-level write needs the capability that writes at the tenant level. -/
theorem write_tenant_unauthorized (t : Tenant) (u : PrincipalId) (n : String) (team : Option TeamId)
    (v : Option Val) (e : Nat) (h : t.holdsCap (.user u) capWriteTenant = false) :
    writeSettingT t (.user u) n .tenant team v e = .error .unauthorized := by
  simp [writeSettingT, writeSettingCore, h] <;> rfl

/-- A team-level write needs the capability that writes at the team level. -/
theorem write_team_unauthorized (t : Tenant) (u : PrincipalId) (n : String) (team : Option TeamId)
    (v : Option Val) (e : Nat) (h : t.holdsCap (.user u) capWriteTeam = false) :
    writeSettingT t (.user u) n .team team v e = .error .unauthorized := by
  simp [writeSettingT, writeSettingCore, h] <;> rfl

/-- A write of a setting the tenant does not define is unknownObject, for a caller who holds the capabilities
    its level needs. -/
theorem write_undefined_refused (t : Tenant) (u : PrincipalId) (n : String) (lvl : SettingLevel)
    (team : Option TeamId) (v : Option Val) (e : Nat) (hl : lvl ≠ .platform)
    (hT : lvl = .tenant → t.holdsCap (.user u) capWriteTenant = true)
    (hTm : lvl = .team → t.holdsCap (.user u) capWriteTeam = true) (hd : t.settingDef? n = none) :
    writeSettingT t (.user u) n lvl team v e = .error .unknownObject := by
  cases lvl <;> simp_all [writeSettingT, writeSettingCore] <;> rfl

/-- A write at a level the definition does not open is invalidIntent: a level it does not allow, or one a lock
    closes. -/
theorem write_closed_level_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef)
    (lvl : SettingLevel) (team : Option TeamId) (v : Option Val) (e : Nat) (hl : lvl ≠ .platform)
    (hT : lvl = .tenant → t.holdsCap (.user u) capWriteTenant = true)
    (hTm : lvl = .team → t.holdsCap (.user u) capWriteTeam = true) (hd : t.settingDef? n = some d)
    (hc : d.isOpen lvl = false) :
    writeSettingT t (.user u) n lvl team v e = .error .invalidIntent := by
  cases lvl <;> simp_all [writeSettingT, writeSettingCore] <;> rfl

/-- A team-level write that names no team is invalidIntent. -/
theorem write_team_unnamed_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef)
    (v : Option Val) (e : Nat) (hd : t.settingDef? n = some d) (hopen : d.isOpen .team = true)
    (hcap : t.holdsCap (.user u) capWriteTeam = true) :
    writeSettingT t (.user u) n .team none v e = .error .invalidIntent := by
  simp [writeSettingT, writeSettingCore, hd, hopen, hcap, subjectOf] <;> rfl

/-- A team-level write naming a team the tenant does not have is invalidIntent. -/
theorem write_team_unknown_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef)
    (tid : TeamId) (v : Option Val) (e : Nat) (hd : t.settingDef? n = some d) (hopen : d.isOpen .team = true)
    (hcap : t.holdsCap (.user u) capWriteTeam = true) (hno : t.teams.lookup tid = none) :
    writeSettingT t (.user u) n .team (some tid) v e = .error .invalidIntent := by
  simp [writeSettingT, writeSettingCore, hd, hopen, hcap, subjectOf, hno] <;> rfl

/-- A team named at the person level is invalidIntent, when that level is open. -/
theorem write_team_elsewhere_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef)
    (team : TeamId) (v : Option Val) (e : Nat) (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true) :
    writeSettingT t (.user u) n .person (some team) v e = .error .invalidIntent := by
  simp [writeSettingT, writeSettingCore, hd, hopen, subjectOf] <;> rfl

/-- A value that is not of the setting's type, null included, is invalid (the kernel's TypeMismatch). -/
theorem write_wrong_type_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (x : Val)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true) (hty : d.ty.admits x = false) :
    writeSettingT t (.user u) n .person none (some x) 0 = .error .invalid := by
  simp [writeSettingT, writeSettingCore, hd, hopen, hty] <;> rfl

/-- A value over the value limits is limitExceeded. -/
theorem write_over_limit_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (x : Val)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true) (hty : d.ty.admits x = true)
    (hlim : Val.overLimit t.limits x = true) :
    writeSettingT t (.user u) n .person none (some x) 0 = .error .limitExceeded := by
  simp [writeSettingT, writeSettingCore, hd, hopen, hty, hlim] <;> rfl

/-- A whole number outside a range the definition allows is limitExceeded. -/
theorem write_out_of_range_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (lo hi k : Int)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true) (hr : d.allowed = some (.range lo hi))
    (hty : d.ty.admits (.int k) = true) (hout : ¬ (lo ≤ k ∧ k ≤ hi))
    (hnone : t.overrideOf? n (.person u) = none) :
    writeSettingT t (.user u) n .person none (some (.int k)) 0 = .error .limitExceeded := by
  simp [writeSettingT, writeSettingCore, allowedOk, Val.overLimit, hout, hd, hopen, hty, hnone, hr] <;> rfl

/-- A value outside a closed set the definition allows is invalidIntent. -/
theorem write_not_allowed_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (vs : List Val)
    (x : Val) (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true)
    (hc : d.allowed = some (.closed vs)) (hty : d.ty.admits x = true) (hlim : Val.overLimit t.limits x = false)
    (hnot : x ∉ vs) :
    writeSettingT t (.user u) n .person none (some x) 0 = .error .invalidIntent := by
  simp_all [writeSettingT, writeSettingCore, allowedOk] <;> rfl

/-- A write whose expected revision is not the override's is versionConflict; a removal shows it too. -/
theorem write_version_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (e : Nat)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true)
    (hexp : e ≠ ((t.overrideOf? n (.person u)).map (fun o => o.revision)).getD 0) :
    writeSettingT t (.user u) n .person none none e = .error .versionConflict := by
  simp_all [writeSettingT, writeSettingCore, subjectOf] <;> rfl

/-- A removal of an override that does not exist is invalidIntent. -/
theorem write_remove_absent_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true)
    (hnone : t.overrideOf? n (.person u) = none) :
    writeSettingT t (.user u) n .person none none 0 = .error .invalidIntent := by
  simp_all [writeSettingT, writeSettingCore, subjectOf] <;> rfl

/-- A new override past the most the tenant holds is limitExceeded. -/
theorem write_bound_refused (t : Tenant) (u : PrincipalId) (n : String) (d : SettingDef) (x : Val)
    (hd : t.settingDef? n = some d) (hopen : d.isOpen .person = true) (hty : d.ty.admits x = true)
    (hlim : Val.overLimit t.limits x = false) (hc : d.allowed = none)
    (hnone : t.overrideOf? n (.person u) = none) (hfull : maxSettingOverrides ≤ t.overrides.length) :
    writeSettingT t (.user u) n .person none (some x) 0 = .error .limitExceeded := by
  simp_all [writeSettingT, writeSettingCore, subjectOf, allowedOk] <;> rfl

end Strata

namespace Strata

/-- The revisions an accepted write leaves: the setting's entry replaced by the new revision, the others kept. -/
theorem lookup_filter_append_self (l : List (String × Nat)) (n : String) (r : Nat) :
    ((l.filter (fun x => x.1 != n)) ++ [(n, r)]).lookup n = some r := by
  induction l with
  | nil => simp [List.lookup]
  | cons x xs ih =>
    by_cases hx : x.1 = n
    · simp [List.filter, hx, ih]
    · have hne : (x.1 != n) = true := by simpa using hx
      have h2 : (n == x.1) = false := by
        rw [beq_eq_false_iff_ne]
        exact fun h => hx h.symm
      simp [List.filter, hne, List.lookup, h2, ih]

theorem lookup_filter_append_other (l : List (String × Nat)) (n m : String) (r : Nat) (hm : m ≠ n) :
    ((l.filter (fun x => x.1 != n)) ++ [(n, r)]).lookup m = l.lookup m := by
  induction l with
  | nil =>
    have h2 : (m == n) = false := by
      rw [beq_eq_false_iff_ne]
      exact hm
    simp [List.lookup, h2]
  | cons x xs ih =>
    by_cases hx : x.1 = n
    · have hmx : (m == x.1) = false := by
        rw [beq_eq_false_iff_ne]
        exact fun h => hm (h.trans hx)
      have hmn : (m == n) = false := by
        rw [beq_eq_false_iff_ne]
        exact hm
      simp [List.filter, hx, List.lookup, hmx, hmn, ih]
    · have hne : (x.1 != n) = true := by simpa using hx
      by_cases hxm : m = x.1
      · simp [List.filter, hne, List.lookup, hxm]
      · have h3 : (m == x.1) = false := by
          rw [beq_eq_false_iff_ne]
          exact hxm
        simp [List.filter, hne, List.lookup, h3, ih]

/-- The overrides and the revisions an accepted core write leaves, for the subject the write names. -/
theorem writeSettingCore_ok_form {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingCore t (.user u) n lvl team v e = .ok t') :
    t'.overrides = (t.overrides.filter (fun o => !(o.name == n && decide (o.subject = subj)))) ++
        (match v with
          | some x => [{ name := n, subject := subj, value := x, revision := t.settingRev n + 1 }]
          | none => []) ∧
      t'.settingRevs = (t.settingRevs.filter (fun x => x.1 != n)) ++ [(n, t.settingRev n + 1)] := by
  unfold writeSettingCore at h
  simp only [hs] at h
  repeat' split at h
  all_goals first
    | (have h2 := Except.ok.inj h; subst h2; simp)
    | simp at h

/-- The overrides and the revisions an accepted write leaves (`writeSettingT`, which applies the core write to the
    decision view and keeps the rest of the tenant). -/
theorem writeSettingT_ok_form {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team v e = .ok t') :
    t'.overrides = (t.overrides.filter (fun o => !(o.name == n && decide (o.subject = subj)))) ++
        (match v with
          | some x => [{ name := n, subject := subj, value := x, revision := t.settingRev n + 1 }]
          | none => []) ∧
      t'.settingRevs = (t.settingRevs.filter (fun x => x.1 != n)) ++ [(n, t.settingRev n + 1)] := by
  cases v with
  | none =>
    unfold writeSettingT at h
    cases hc : writeSettingCore t.decisionView (.user u) n lvl team none e with
    | error x => simp [hc, Except.map] at h
    | ok a =>
      simp only [hc, Except.map, Except.ok.injEq] at h
      subst h
      exact writeSettingCore_ok_form (t := t.decisionView) (by simpa using hs) hc
  | some x =>
    unfold writeSettingT at h
    cases hc : writeSettingCore t.decisionView (.user u) n lvl team (some x) e with
    | error y => simp [hc, Except.map] at h
    | ok a =>
      simp only [hc, Except.map, Except.ok.injEq] at h
      subst h
      exact writeSettingCore_ok_form (t := t.decisionView) (by simpa using hs) hc

/-- The filter keeps an override whose key is not the write's own. -/
theorem keep_pred {o : SettingOverride} {n : String} {subj : SettingSubject}
    (hk : ¬ (o.name = n ∧ o.subject = subj)) : (!(o.name == n && decide (o.subject = subj))) = true := by
  by_cases h1 : o.name = n <;> by_cases h2 : o.subject = subj
  · exact absurd ⟨h1, h2⟩ hk
  · simp [h1, h2]
  · simp [h1, h2]
  · simp [h1, h2]

/-- An accepted write keeps every override whose key is not the write's own. -/
theorem write_keeps_others {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team v e = .ok t') (o : SettingOverride)
    (ho : o ∈ t.overrides) (hk : ¬ (o.name = n ∧ o.subject = subj)) : o ∈ t'.overrides := by
  obtain ⟨hov, -⟩ := writeSettingT_ok_form hs h
  rw [hov]
  cases v with
  | none => exact List.mem_append_left _ (List.mem_filter.mpr ⟨ho, keep_pred hk⟩)
  | some x => exact List.mem_append_left _ (List.mem_filter.mpr ⟨ho, keep_pred hk⟩)

/-- An accepted write changes no override but the one its key names: every other override of the result was there
    before. -/
theorem write_only_key_changes {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team v e = .ok t') (o : SettingOverride) (ho : o ∈ t'.overrides) :
    (o ∈ t.overrides ∧ ¬ (o.name = n ∧ o.subject = subj)) ∨ (o.name = n ∧ o.subject = subj) := by
  obtain ⟨hov, -⟩ := writeSettingT_ok_form hs h
  rw [hov] at ho
  by_cases hk : o.name = n ∧ o.subject = subj
  · exact Or.inr hk
  · cases v with
    | none =>
      rw [List.append_nil, List.mem_filter] at ho
      exact Or.inl ⟨ho.1, hk⟩
    | some x =>
      rw [List.mem_append, List.mem_filter] at ho
      rcases ho with ho | ho
      · exact Or.inl ⟨ho.1, hk⟩
      · exact absurd (show o.name = n ∧ o.subject = subj from by simp only [List.mem_singleton] at ho; rw [ho]; exact ⟨rfl, rfl⟩) hk

/-- An accepted write raises the setting's revision by one, and leaves every other setting's revision as it was. -/
theorem write_raises_revision {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team v e = .ok t') :
    t'.settingRev n = t.settingRev n + 1 ∧ ∀ m, m ≠ n → t'.settingRev m = t.settingRev m := by
  obtain ⟨-, hrv⟩ := writeSettingT_ok_form hs h
  constructor
  · unfold Tenant.settingRev
    rw [hrv, lookup_filter_append_self]
    rfl
  · intro m hm
    unfold Tenant.settingRev
    rw [hrv, lookup_filter_append_other _ n m _ hm]

/-- An accepted write emits exactly one event: the setting-changed event of its key, numbered after the tenant's
    sequence and committed at its new revision. -/
theorem write_event {w : World} {n : String} {lvl : SettingLevel} {team : Option TeamId} {v : Option Val}
    {e : Nat} {c : Ctx} {tr : Transition} {t : Tenant}
    (h : transition w (.writeSetting n lvl team v e) c = .ok tr) (ht : w c.tenant = some t) :
    ∃ t₁, writeSettingT t c.principal n lvl team v e = .ok t₁ ∧
      tr.events.map (fun ev => ev.body) = [.settingChanged n (settingSubjectOf c.principal lvl team)] := by
  obtain ⟨t₁, hst, -, hev⟩ := transition_existing h ht
  refine ⟨t₁, hst, ?_⟩
  rw [hev]
  simp [eventBodies, numberEvents]

/-- A refused write has no transition: it emits no event and makes no state, so the revision stays where it was. -/
theorem refused_write_no_transition {w : World} {n : String} {lvl : SettingLevel} {team : Option TeamId}
    {v : Option Val} {e : Nat} {c : Ctx} {t : Tenant} {err : Err}
    (ht : w c.tenant = some t) (hr : reservedRefused c.principal (.writeSetting n lvl team v e) = false)
    (hw : writeSettingT t c.principal n lvl team v e = .error err) :
    transition w (.writeSetting n lvl team v e) c = .error err := by
  apply transition_refused
  simp [step, hr, ht, stepTenant, hw]

end Strata

namespace Strata

/-- An accepted write of a value leaves the override of its key holding that value, at the setting's new revision. -/
theorem write_sets_override {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {e : Nat} {subj : SettingSubject} {x : Val}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team (some x) e = .ok t') :
    ∃ o ∈ t'.overrides, o.name = n ∧ o.subject = subj ∧ o.value = x ∧ o.revision = t.settingRev n + 1 := by
  obtain ⟨hov, -⟩ := writeSettingT_ok_form hs h
  refine ⟨{ name := n, subject := subj, value := x, revision := t.settingRev n + 1 }, ?_, rfl, rfl, rfl, rfl⟩
  rw [hov]
  exact List.mem_append_right _ (List.mem_singleton.mpr rfl)

/-- An accepted removal leaves no override of its key. -/
theorem write_removes_override {t t' : Tenant} {u : PrincipalId} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {e : Nat} {subj : SettingSubject}
    (hs : subjectOf t u lvl team = .ok subj)
    (h : writeSettingT t (.user u) n lvl team none e = .ok t') :
    ∀ o ∈ t'.overrides, ¬ (o.name = n ∧ o.subject = subj) := by
  obtain ⟨hov, -⟩ := writeSettingT_ok_form hs h
  intro o ho hk
  rw [hov, List.append_nil, List.mem_filter] at ho
  have hb := ho.2
  simp [hk.1, hk.2] at hb

end Strata

/-! Axioms of the slice 12 theorems: only propext, Classical.choice and Quot.sound. -/
#print axioms Strata.resolve_value_eq
#print axioms Strata.resolve_person
#print axioms Strata.resolve_team
#print axioms Strata.resolve_tenant
#print axioms Strata.resolve_platform
#print axioms Strata.resolve_default
#print axioms Strata.resolve_closed_level_ignored
#print axioms Strata.writeSettingCore_ok_subject
#print axioms Strata.write_system_refused
#print axioms Strata.write_anonymous_refused
#print axioms Strata.write_platform_refused
#print axioms Strata.write_tenant_unauthorized
#print axioms Strata.write_team_unauthorized
#print axioms Strata.write_undefined_refused
#print axioms Strata.write_closed_level_refused
#print axioms Strata.write_team_unnamed_refused
#print axioms Strata.write_team_unknown_refused
#print axioms Strata.write_team_elsewhere_refused
#print axioms Strata.write_wrong_type_refused
#print axioms Strata.write_over_limit_refused
#print axioms Strata.write_out_of_range_refused
#print axioms Strata.write_not_allowed_refused
#print axioms Strata.write_version_refused
#print axioms Strata.write_remove_absent_refused
#print axioms Strata.write_bound_refused
#print axioms Strata.writeSettingCore_ok_form
#print axioms Strata.writeSettingT_ok_form
#print axioms Strata.keep_pred
#print axioms Strata.write_keeps_others
#print axioms Strata.write_only_key_changes
#print axioms Strata.write_raises_revision
#print axioms Strata.write_event
#print axioms Strata.refused_write_no_transition
#print axioms Strata.write_sets_override
#print axioms Strata.write_removes_override

/-! ## Slice 13: the ledger (`lean/Strata/LedgerTypes.lean`, `lean/Strata/LedgerModel.lean`, the intents
`postLedger` and `expireLedger`, `lean/Main.lean`) -/

namespace Strata

/-- The sum of the amounts of every entry of the journal: the sum of all balances of the ledger. -/
def ledgerTotal (L : LedgerState) : Int := (L.journal.map (fun e => e.amount)).sum

/-- The balance of an account, zero when the account has no header. -/
def ledgerBal (L : LedgerState) (o : LedgerOwner) (u : Nat) (s : String) : Int :=
  ((L.accounts.find? (fun a => a.is o u s)).map (fun a => a.balance)).getD 0

/-- A posting conserves the total: an accepted posting moves the total by the amount of the entry it writes (the
    event it emits), and a replay moves it by nothing. -/
theorem postOne_total {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postOne t actor now L q = .ok (L', evs)) :
    ledgerTotal L' = ledgerTotal L + (evs.map (fun e => e.amount)).sum := by
  unfold postOne postDynamic at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       simp [ledgerTotal, List.map_append, List.sum_append])
    | cases h
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       simp [ledgerTotal, List.map_append, List.sum_append])

/-- A refused posting refuses the whole intent: when the postings before it are accepted and the next one is refused on
    the state they leave, the intent is refused with that same refusal, whatever postings follow (the atomicity of the
    one to eight postings, LDG-009). -/
theorem ledger_post_atomic {t : Tenant} {actor : Principal} {now : Int} {ps₁ ps₂ : List LedgerPosting}
    {q : LedgerPosting} {e : Err} {L₁ : LedgerState} {evs₁ : List LedgerPosted}
    (hcap : t.holdsCap actor ledgerCapPost = true)
    (hlen : (ps₁ ++ q :: ps₂).length ≤ ledgerMaxPostings)
    (h₁ : List.foldlM (fun (acc : LedgerState × List LedgerPosted) (x : LedgerPosting) =>
        (postOne t actor now acc.1 x).map (fun r => (r.1, acc.2 ++ r.2))) (t.ledger, []) ps₁ = .ok (L₁, evs₁))
    (hq : postOne t actor now L₁ q = .error e) :
    postLedgerCore t actor now (ps₁ ++ q :: ps₂) = .error e := by
  unfold postLedgerCore
  have hl : decide (ledgerMaxPostings < (ps₁ ++ q :: ps₂).length) = false := by
    simp only [decide_eq_false_iff_not, Nat.not_lt]
    exact hlen
  have hn : (ps₁ ++ q :: ps₂).isEmpty = false := by
    cases hps : ps₁ ++ q :: ps₂ with
    | nil => simp at hps
    | cons x xs => rfl
  simp only [hcap, hl, hn, Bool.not_true, Bool.false_or, Bool.or_false, Bool.false_eq_true, if_false]
  rw [List.foldlM_append, h₁]
  simp [List.foldlM_cons, hq, Except.map, Except.bind, bind]

/-- What an accepted posting (one that writes an entry) establishes: its static checks passed, its key was new on the
    account, and the journal it leaves is the one before with the entry of the posting appended, that entry naming the
    account, the key, the amount, the reason and the correction of the posting. -/
theorem postOne_accepted {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postOne t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    postStaticErr t actor q = none ∧
    L.journal.find? (fun e => e.inAccount q.owner q.unit q.scope && e.key == q.key) = none ∧
    ∃ entry : LedgerEntry, L'.journal = L.journal ++ [entry] ∧ entry.owner = q.owner ∧ entry.unit = q.unit ∧
      entry.scope = q.scope ∧ entry.key = q.key ∧ entry.amount = q.amount ∧ entry.reason = q.reason ∧
      entry.corrects = q.corrects := by
  unfold postOne postDynamic at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       exact ⟨by assumption, by assumption, _, rfl, rfl, rfl, rfl, rfl, rfl, rfl, rfl⟩)
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       exact absurd rfl hne)
    | cases h

/-- A replay: a posting accepted on a state is replayed, with no change, on the state it leaves (the idempotency of a
    key, LDG-006, LDG-2931): the same posting, asked again, returns the same answer and writes nothing. -/
theorem ledger_replay_unchanged {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState}
    {q : LedgerPosting} {evs : List LedgerPosted}
    (h : postOne t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    postOne t actor now L' q = .ok (L', []) := by
  obtain ⟨hs, hf, entry, hJ, ho, hu, hsc, hk, ha, hr, hc⟩ := postOne_accepted h hne
  unfold postOne
  rw [hs]
  unfold postDynamic
  rw [hJ, List.find?_append, hf, Option.none_or]
  simp [List.find?, LedgerEntry.inAccount, ho, hu, hsc, hk, ha, hr, hc]

/-- The entries of an expiry step: each is the debit of the bucket it is keyed by, and that bucket is one the step is
    given. -/
theorem expireStep_new (e : LedgerExpiry) (t : Int) (acc : LedgerAccount × List (LedgerEntry × LedgerPosted))
    (b : LedgerBucket) :
    ∀ x ∈ (expireStep e t acc b).2, x ∈ acc.2 ∨
      (x.1.amount = -(b.remaining : Int) ∧ x.1.key = expiryKeyPrefix ++ toString b.periodStart) := by
  intro x hx
  simp only [expireStep, List.mem_append, List.mem_singleton] at hx
  rcases hx with hx | hx
  · exact Or.inl hx
  · subst hx
    exact Or.inr ⟨rfl, rfl⟩

/-- An expiry debits only the buckets whose life has ended: every entry an account's expiry writes is the debit of one of
    its buckets that started at or before the cutoff, and is keyed by that bucket's start (LDG-1545 to LDG-1547). -/
theorem expireAccount_debits_due (e : LedgerExpiry) (t cutoff : Int) (a : LedgerAccount) :
    ∀ x ∈ (expireAccount e t cutoff a).2, ∃ b ∈ a.buckets, b.periodStart ≤ cutoff ∧
      x.1.amount = -(b.remaining : Int) ∧ x.1.key = expiryKeyPrefix ++ toString b.periodStart := by
  have gen : ∀ (ds : List LedgerBucket) (acc : LedgerAccount × List (LedgerEntry × LedgerPosted)),
      (∀ x ∈ acc.2, ∃ b ∈ a.buckets, b.periodStart ≤ cutoff ∧
        x.1.amount = -(b.remaining : Int) ∧ x.1.key = expiryKeyPrefix ++ toString b.periodStart) →
      (∀ b ∈ ds, b ∈ a.buckets ∧ b.periodStart ≤ cutoff) →
      ∀ x ∈ (ds.foldl (expireStep e t) acc).2, ∃ b ∈ a.buckets, b.periodStart ≤ cutoff ∧
        x.1.amount = -(b.remaining : Int) ∧ x.1.key = expiryKeyPrefix ++ toString b.periodStart := by
    intro ds
    induction ds with
    | nil =>
      intro acc hacc _ x hx
      exact hacc x hx
    | cons b bs ih =>
      intro acc hacc hds x hx
      refine ih (expireStep e t acc b) ?_ ?_ x hx
      · intro y hy
        rcases expireStep_new e t acc b y hy with hy' | ⟨ha, hk⟩
        · exact hacc y hy'
        · exact ⟨b, (hds b (by simp)).1, (hds b (by simp)).2, ha, hk⟩
      · intro c hc
        exact hds c (by simp [hc])
  intro x hx
  unfold expireAccount at hx
  refine gen _ (a, []) (by simp) ?_ x hx
  intro b hb
  have hm := List.mem_filter.mp hb
  simp only [Bool.and_eq_true, decide_eq_true_eq] at hm
  exact ⟨hm.1, hm.2.1⟩

/-- An expiry step writes entries whose amounts are the amounts of their events (given that the entries before it do). -/
theorem expireStep_amount (e : LedgerExpiry) (t : Int) (acc : LedgerAccount × List (LedgerEntry × LedgerPosted))
    (b : LedgerBucket) (hacc : ∀ y ∈ acc.2, y.1.amount = y.2.amount) :
    ∀ x ∈ (expireStep e t acc b).2, x.1.amount = x.2.amount := by
  intro x hx
  simp only [expireStep, List.mem_append, List.mem_singleton] at hx
  rcases hx with hx | hx
  · exact hacc x hx
  · subst hx
    rfl

/-- A read of an account is refused to a reader that may not read it at all (the kernel's `may_read`). -/
theorem read_refused_when_unpermitted {t : Tenant} {p : Principal} {o : LedgerOwner} {u : Nat} {s : String}
    {own : Bool} {L : LedgerState} (h : mayReadLedger t p o = .error .unauthorized) :
    readLedgerBalance t p o u s own L = .error .unauthorized := by
  unfold readLedgerBalance
  simp [h]

/-- The entries of one account's expiry have the amounts of their events. -/
theorem expireAccount_amounts (e : LedgerExpiry) (t cutoff : Int) (a : LedgerAccount) :
    ∀ x ∈ (expireAccount e t cutoff a).2, x.1.amount = x.2.amount := by
  have gen : ∀ (ds : List LedgerBucket) (acc : LedgerAccount × List (LedgerEntry × LedgerPosted)),
      (∀ y ∈ acc.2, y.1.amount = y.2.amount) →
      ∀ x ∈ (ds.foldl (expireStep e t) acc).2, x.1.amount = x.2.amount := by
    intro ds
    induction ds with
    | nil =>
      intro acc hacc x hx
      exact hacc x hx
    | cons b bs ih =>
      intro acc hacc x hx
      exact ih _ (expireStep_amount e t acc b hacc) x hx
  intro x hx
  unfold expireAccount at hx
  exact gen _ _ (by simp) x hx

/-- Two lists of entries whose amounts agree pairwise have the same sum of amounts. -/
theorem sum_amounts_of_pairs (xs : List (LedgerEntry × LedgerPosted))
    (h : ∀ x ∈ xs, x.1.amount = x.2.amount) :
    (xs.map (fun x => x.1.amount)).sum = (xs.map (fun x => x.2.amount)).sum := by
  rw [List.map_congr_left h]

/-- An accepted ExpireLedger moves the total of the journal by the amounts of the events it emits: the expiry debits
    exactly the credit it reports, and nothing else. -/
theorem expireCore_total {t : Tenant} {actor : Principal} {now : Int} {unit : Nat} {cutoffAt : Int}
    {L' : LedgerState} {evs : List LedgerPosted} (h : expireCore t actor now unit cutoffAt = .ok (L', evs)) :
    ledgerTotal L' = ledgerTotal t.ledger + (evs.map (fun e => e.amount)).sum := by
  unfold expireCore at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       simp only [ledgerTotal, List.map_append, List.sum_append, List.map_map, Function.comp_def]
       congr 1
       apply sum_amounts_of_pairs
       intro x hx
       obtain ⟨r, hr, hxr⟩ := List.mem_flatMap.mp hx
       obtain ⟨a, _, rfl⟩ := List.mem_map.mp hr
       exact expireAccount_amounts _ _ _ a x hxr)
    | cases h

end Strata

/-! ## Slice 13: the limits of an accepted posting, the frame lemma and the balance invariant -/
namespace Strata

theorem postDynamic_new {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postDynamic t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    let cur := L.accounts.find? (fun a => a.is q.owner q.unit q.scope)
    let before : Int := (cur.map (fun a => a.balance)).getD 0
    let last : Nat := (cur.map (fun a => a.lastSeq)).getD 0
    let curPeriods : List LedgerPeriodTotal := (cur.map (fun a => a.periods)).getD []
    let applying := applyingLimits t (accountLimit? L q.owner q.unit q.scope) q.unit
    correctionErr last q = none ∧ boundErr (before + q.amount) = none ∧
    floorErr applying before (before + q.amount) q.amount = none ∧
    capErr applying before (before + q.amount) q.amount = none ∧
    periodErr applying curPeriods (decide (0 < q.amount) && !(applying.additionsOnly && q.corrects.isSome)) now q.amount = none := by
  intro cur before last curPeriods applying
  unfold postDynamic at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       simp_all [cur, before, last, curPeriods, applying])
    | cases h
end Strata
namespace Strata

theorem floorErr_none {ap : AppliedLimits} {b a amt : Int} (h : floorErr ap b a amt = none) :
    ∀ f, ap.floor = some f → f ≤ a ∨ (b < f ∧ 0 < amt) := by
  intro f hf
  unfold floorErr at h
  rw [hf] at h
  simp only [] at h
  split at h
  · cases h
  · rename_i hc
    by_cases h1 : a < f
    · right
      simp only [h1, decide_true, Bool.true_and, Bool.not_eq_true'] at hc
      simpa using hc
    · left; omega

theorem capErr_none {ap : AppliedLimits} {b a amt : Int} (h : capErr ap b a amt = none) :
    ∀ c, ap.cap = some c → a ≤ c ∨ (c < b ∧ amt < 0) := by
  intro c hc'
  unfold capErr at h
  rw [hc'] at h
  simp only [] at h
  split at h
  · cases h
  · rename_i hc
    by_cases h1 : c < a
    · right
      simp only [h1, decide_true, Bool.true_and, Bool.not_eq_true'] at hc
      simpa using hc
    · left; omega

theorem periodErr_none {ap : AppliedLimits} {cur : List LedgerPeriodTotal} {counts : Bool} {now amt : Int}
    (h : periodErr ap cur counts now amt = none) :
    ∀ cap p, ap.periodCap = some (cap, p) → counts = true → creditsIn cur p now + amt.toNat ≤ cap := by
  intro cap p hp hc
  unfold periodErr at h
  rw [hp] at h
  simp only [hc, Bool.true_and] at h
  split at h
  · cases h
  · rename_i hn
    simpa using hn

end Strata
namespace Strata

/-- The state an accepted, non-replayed posting leaves. -/
theorem postDynamic_state {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postDynamic t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    ∃ (acct : LedgerAccount) (entry : LedgerEntry),
      acct.is q.owner q.unit q.scope = true ∧
      acct.balance = ((L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance)).getD 0 + q.amount ∧
      entry.owner = q.owner ∧ entry.unit = q.unit ∧ entry.scope = q.scope ∧ entry.amount = q.amount ∧
      L'.journal = L.journal ++ [entry] ∧ L'.limits = L.limits ∧
      L'.accounts = (if (L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).isSome then
          L.accounts.map (fun a => if a.is q.owner q.unit q.scope then acct else a)
        else L.accounts ++ [acct]) := by
  unfold postDynamic at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (cases h; done)
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       exact absurd rfl hne)
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, rfl⟩ := h
       apply Exists.intro
       apply Exists.intro
       refine ⟨?h1, ?h2, ?h3, ?h4, ?h5, ?h6, ?h7, ?h8, ?h9⟩
       case h7 => rfl
       case h8 => rfl
       case h9 =>
         dsimp only
         first
           | exact (if_pos (c := (List.find? (fun a => a.is q.owner q.unit q.scope) L.accounts).isSome = true) (by simp_all [List.find?_isSome])).symm
           | exact (if_neg (c := (List.find? (fun a => a.is q.owner q.unit q.scope) L.accounts).isSome = true) (by simp_all [List.find?_isSome])).symm
       case h1 => simp [LedgerAccount.is]
       case h2 => rfl
       case h3 => rfl
       case h4 => rfl
       case h5 => rfl
       case h6 => rfl)
end Strata
namespace Strata

/-- Two accounts, or an account and the key of an entry, name the same account. -/
def LedgerAccount.sameKey (a b : LedgerAccount) : Prop :=
  a.owner = b.owner ∧ a.unit = b.unit ∧ a.scope = b.scope

theorem LedgerAccount.is_iff (a : LedgerAccount) (o : LedgerOwner) (u : Nat) (s : String) :
    a.is o u s = true ↔ (a.owner = o ∧ a.unit = u ∧ a.scope = s) := by
  simp [LedgerAccount.is, and_assoc]

theorem LedgerEntry.inAccount_iff (e : LedgerEntry) (o : LedgerOwner) (u : Nat) (s : String) :
    e.inAccount o u s = true ↔ (e.owner = o ∧ e.unit = u ∧ e.scope = s) := by
  simp [LedgerEntry.inAccount, and_assoc]

/-- Pointwise frame of a replacing map. -/
theorem find?_map_replace {α : Type} (p : α → Bool) (g : α → α) (l : List α)
    (h : ∀ a ∈ l, p (g a) = p a ∧ (p a = true → g a = a)) :
    (l.map g).find? p = l.find? p := by
  induction l with
  | nil => rfl
  | cons a l ih =>
    have ha := h a (by simp)
    simp only [List.map_cons, List.find?_cons, ha.1]
    cases hp : p a with
    | true => simp [ha.2 hp]
    | false => exact ih (fun b hb => h b (by simp [hb]))

theorem find?_map_hit {α : Type} (p : α → Bool) (g : α → α) (l : List α)
    (h : ∀ a ∈ l, p (g a) = p a) : (l.map g).find? p = (l.find? p).map g := by
  induction l with
  | nil => rfl
  | cons a l ih =>
    have ha := h a (by simp)
    simp only [List.map_cons, List.find?_cons, ha]
    cases hp : p a with
    | true => simp
    | false => exact ih (fun b hb => h b (by simp [hb]))

/-- The pointwise frame: an accepted, non-replayed posting leaves every account other than the posted one as it was,
    and the posted account holds the balance before plus the amount. -/
theorem postOne_frame {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postOne t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    (∀ o u s, ¬ (o = q.owner ∧ u = q.unit ∧ s = q.scope) →
        L'.accounts.find? (fun a => a.is o u s) = L.accounts.find? (fun a => a.is o u s)) ∧
    ((L'.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance) =
      some (((L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance)).getD 0 + q.amount)) := by
  unfold postOne at h
  cases hs : postStaticErr t actor q with
  | some e => rw [hs] at h; cases h
  | none =>
    rw [hs] at h
    simp only [] at h
    obtain ⟨acct, entry, hi, hb, -, -, -, -, -, -, hacc⟩ := postDynamic_state h hne
    obtain ⟨hao, hau, has⟩ := (LedgerAccount.is_iff _ _ _ _).mp hi
    constructor
    · intro o u s hk
      rw [hacc]
      split
      · rw [find?_map_replace]
        intro a _
        by_cases ha : a.is q.owner q.unit q.scope = true
        · have hao' := (LedgerAccount.is_iff _ _ _ _).mp ha
          have h1 : a.is o u s = false := by
            cases hh : a.is o u s
            · rfl
            · exfalso; apply hk
              have := (LedgerAccount.is_iff _ _ _ _).mp hh
              exact ⟨this.1 ▸ hao'.1 ▸ rfl, this.2.1 ▸ hao'.2.1 ▸ rfl, this.2.2 ▸ hao'.2.2 ▸ rfl⟩
          have h2 : acct.is o u s = false := by
            cases hh : acct.is o u s
            · rfl
            · exfalso; apply hk
              have := (LedgerAccount.is_iff _ _ _ _).mp hh
              exact ⟨this.1 ▸ hao ▸ rfl, this.2.1 ▸ hau ▸ rfl, this.2.2 ▸ has ▸ rfl⟩
          simp [ha, h1, h2]
        · simp [ha]
      · rw [List.find?_append]
        have h2 : acct.is o u s = false := by
          cases hh : acct.is o u s
          · rfl
          · exfalso; apply hk
            have := (LedgerAccount.is_iff _ _ _ _).mp hh
            exact ⟨this.1 ▸ hao ▸ rfl, this.2.1 ▸ hau ▸ rfl, this.2.2 ▸ has ▸ rfl⟩
        simp [h2]
    · rw [hacc]
      split
      · rename_i hsome
        rw [find?_map_hit]
        · obtain ⟨a0, ha0⟩ := Option.isSome_iff_exists.mp hsome
          have hp := List.find?_some ha0
          simp [ha0, hp, hb]
        · intro a _
          by_cases ha : a.is q.owner q.unit q.scope = true
          · simp [ha, hi]
          · simp [ha]
      · rename_i hnone
        have hn : L.accounts.find? (fun a => a.is q.owner q.unit q.scope) = none := by
          simpa using hnone
        simp [List.find?_append, hn, hi, hb]
end Strata
namespace Strata

/-- The limits hold after an accepted posting, in the kernel's rule: an accepted posting leaves the balance at or above
    the floor unless it moved the balance up from below it, and at or below the cap unless it moved it down from above
    it; and the credits of the period of its entry, with the entry, are within the cap per period whenever the posting
    counts toward it (a credit, other than a reversal under an additions-only cap). The balance is the one the posted
    account holds afterwards. -/
theorem postOne_limits {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postOne t actor now L q = .ok (L', evs)) (hne : evs ≠ []) :
    let cur := L.accounts.find? (fun a => a.is q.owner q.unit q.scope)
    let before : Int := (cur.map (fun a => a.balance)).getD 0
    let applying := applyingLimits t (accountLimit? L q.owner q.unit q.scope) q.unit
    (L'.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance) = some (before + q.amount) ∧
    (∀ f, applying.floor = some f → f ≤ before + q.amount ∨ (before < f ∧ 0 < q.amount)) ∧
    (∀ c, applying.cap = some c → before + q.amount ≤ c ∨ (c < before ∧ q.amount < 0)) ∧
    (∀ cap p, applying.periodCap = some (cap, p) →
      (0 < q.amount ∧ ¬ (applying.additionsOnly = true ∧ q.corrects.isSome = true)) →
      creditsIn ((cur.map (fun a => a.periods)).getD []) p now + q.amount.toNat ≤ cap) := by
  intro cur before applying
  have hf := postOne_frame h hne
  unfold postOne at h
  cases hs : postStaticErr t actor q with
  | some e => rw [hs] at h; cases h
  | none =>
    rw [hs] at h
    simp only [] at h
    obtain ⟨hcor, hbound, hfl, hcap, hper⟩ := postDynamic_new h hne
    refine ⟨hf.2, floorErr_none hfl, capErr_none hcap, ?_⟩
    intro cap p hp ⟨hpos, hnot⟩
    refine periodErr_none hper cap p hp ?_
    simp only [Bool.and_eq_true, decide_eq_true_eq, Bool.not_eq_true']
    refine ⟨hpos, ?_⟩
    simpa using hnot
end Strata
namespace Strata

/-- The sum of the amounts of the entries of one account in a journal. -/
def acctSum (J : List LedgerEntry) (o : LedgerOwner) (u : Nat) (s : String) : Int :=
  ((J.filter (fun e => e.inAccount o u s)).map (fun e => e.amount)).sum

/-- The ledger invariant: every account's balance is the sum of its entries' amounts, no two accounts name the same
    account, and every entry is in an account that has a header. -/
structure LedgerSound (L : LedgerState) : Prop where
  bal : ∀ a ∈ L.accounts, a.balance = acctSum L.journal a.owner a.unit a.scope
  uniq : L.accounts.Pairwise (fun a b => ¬ a.sameKey b)
  cover : ∀ e ∈ L.journal, ∃ a ∈ L.accounts, a.owner = e.owner ∧ a.unit = e.unit ∧ a.scope = e.scope

theorem acctSum_append (J K : List LedgerEntry) (o u s) : acctSum (J ++ K) o u s = acctSum J o u s + acctSum K o u s := by
  simp [acctSum, List.filter_append, List.map_append, List.sum_append]

theorem acctSum_nil (o u s) : acctSum [] o u s = 0 := by simp [acctSum]

theorem acctSum_single_same (e : LedgerEntry) (o u s) (h : e.owner = o ∧ e.unit = u ∧ e.scope = s) :
    acctSum [e] o u s = e.amount := by
  have : e.inAccount o u s = true := (LedgerEntry.inAccount_iff _ _ _ _).mpr h
  simp [acctSum, this]

theorem acctSum_single_other (e : LedgerEntry) (o u s) (h : ¬ (e.owner = o ∧ e.unit = u ∧ e.scope = s)) :
    acctSum [e] o u s = 0 := by
  have : e.inAccount o u s = false := by
    cases hh : e.inAccount o u s
    · rfl
    · exact absurd ((LedgerEntry.inAccount_iff _ _ _ _).mp hh) h
  simp [acctSum, this]

theorem acctSum_zero_of_none (J : List LedgerEntry) (o u s)
    (h : ∀ e ∈ J, ¬ (e.owner = o ∧ e.unit = u ∧ e.scope = s)) : acctSum J o u s = 0 := by
  have : J.filter (fun e => e.inAccount o u s) = [] := by
    rw [List.filter_eq_nil_iff]
    intro e he
    have := h e he
    cases hh : e.inAccount o u s
    · simp
    · exact absurd ((LedgerEntry.inAccount_iff _ _ _ _).mp hh) this
  simp [acctSum, this]

theorem sameKey_symm {a b : LedgerAccount} (h : a.sameKey b) : b.sameKey a :=
  ⟨h.1.symm, h.2.1.symm, h.2.2.symm⟩
theorem sameKey_trans {a b c : LedgerAccount} (h : a.sameKey b) (h' : b.sameKey c) : a.sameKey c :=
  ⟨h.1.trans h'.1, h.2.1.trans h'.2.1, h.2.2.trans h'.2.2⟩

/-- A header is replaced by another of the same key. -/
theorem pairwise_map_sameKey {l : List LedgerAccount} (g : LedgerAccount → LedgerAccount)
    (hg : ∀ a ∈ l, (g a).sameKey a) (hl : l.Pairwise (fun a b => ¬ a.sameKey b)) :
    (l.map g).Pairwise (fun a b => ¬ a.sameKey b) := by
  rw [List.pairwise_map]
  refine List.Pairwise.imp_of_mem ?_ hl
  intro a b ha hb hab hgab
  exact hab (sameKey_trans (sameKey_symm (hg a ha)) (sameKey_trans hgab (hg b hb)))

end Strata
namespace Strata

theorem postDynamic_replay {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postDynamic t actor now L q = .ok (L', evs)) (he : evs = []) : L' = L := by
  subst he
  unfold postDynamic at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       exact h.1.symm)
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, h2⟩ := h
       simp at h2)
    | cases h

theorem uniq_of_pairwise {l : List LedgerAccount} (hl : l.Pairwise (fun a b => ¬ a.sameKey b))
    {a b : LedgerAccount} (ha : a ∈ l) (hb : b ∈ l) (hab : a.sameKey b) : a = b := by
  induction l with
  | nil => cases ha
  | cons x xs ih =>
    rw [List.pairwise_cons] at hl
    rcases List.mem_cons.mp ha with rfl | ha'
    · rcases List.mem_cons.mp hb with rfl | hb'
      · rfl
      · exact absurd hab (hl.1 b hb')
    · rcases List.mem_cons.mp hb with rfl | hb'
      · exact absurd (sameKey_symm hab) (hl.1 a ha')
      · exact ih hl.2 ha' hb'

theorem postOne_sound {t : Tenant} {actor : Principal} {now : Int} {L L' : LedgerState} {q : LedgerPosting}
    {evs : List LedgerPosted} (h : postOne t actor now L q = .ok (L', evs)) (hL : LedgerSound L) :
    LedgerSound L' := by
  by_cases hne : evs = []
  · have : L' = L := by
      unfold postOne at h
      cases hs : postStaticErr t actor q with
      | some e => rw [hs] at h; cases h
      | none => rw [hs] at h; exact postDynamic_replay h hne
    rw [this]; exact hL
  · unfold postOne at h
    cases hs : postStaticErr t actor q with
    | some e => rw [hs] at h; cases h
    | none =>
      rw [hs] at h
      simp only [] at h
      obtain ⟨acct, entry, hi, hb, heo, heu, hes, hea, hj, -, hacc⟩ := postDynamic_state h hne
      obtain ⟨hao, hau, has⟩ := (LedgerAccount.is_iff _ _ _ _).mp hi
      have hsum : ∀ o u s, acctSum L'.journal o u s =
          acctSum L.journal o u s + (if (o = q.owner ∧ u = q.unit ∧ s = q.scope) then q.amount else 0) := by
        intro o u s
        rw [hj, acctSum_append]
        by_cases hk : (o = q.owner ∧ u = q.unit ∧ s = q.scope)
        · rw [if_pos hk, acctSum_single_same _ _ _ _ ⟨heo ▸ hk.1.symm, heu ▸ hk.2.1.symm, hes ▸ hk.2.2.symm⟩, hea]
        · rw [if_neg hk, acctSum_single_other _ _ _ _ (fun hh => hk ⟨heo ▸ hh.1.symm, heu ▸ hh.2.1.symm, hes ▸ hh.2.2.symm⟩)]
      have uniq_eq : ∀ {a b : LedgerAccount}, a ∈ L.accounts → b ∈ L.accounts → a.sameKey b → a = b := by
        intro a b ha hb hab
        exact uniq_of_pairwise hL.uniq ha hb hab
      by_cases hsome : (L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).isSome = true
      · rw [if_pos hsome] at hacc
        obtain ⟨a0, ha0⟩ := Option.isSome_iff_exists.mp hsome
        have ha0m := List.mem_of_find?_eq_some ha0
        have ha0i := List.find?_some ha0
        have ha0k := (LedgerAccount.is_iff _ _ _ _).mp ha0i
        have hbefore : ((L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance)).getD 0
            = acctSum L.journal q.owner q.unit q.scope := by
          rw [ha0]; simp only [Option.map_some, Option.getD_some]
          rw [hL.bal a0 ha0m, ha0k.1, ha0k.2.1, ha0k.2.2]
        refine ⟨?_, ?_, ?_⟩
        · rw [hacc]
          intro a ha
          obtain ⟨a1, ha1, rfl⟩ := List.mem_map.mp ha
          by_cases hk : a1.is q.owner q.unit q.scope = true
          · simp only [hk, if_true]
            rw [hb, hbefore, hsum, if_pos ⟨hao, hau, has⟩, hao, hau, has]
          · simp only [hk, Bool.false_eq_true, if_false]
            rw [hL.bal a1 ha1, hsum, if_neg]
            · simp
            · intro hh
              exact hk ((LedgerAccount.is_iff _ _ _ _).mpr ⟨hh.1, hh.2.1, hh.2.2⟩)
        · rw [hacc]
          apply pairwise_map_sameKey _ _ hL.uniq
          intro a ha
          by_cases hk : a.is q.owner q.unit q.scope = true
          · simp only [hk, if_true]
            have hk' := (LedgerAccount.is_iff _ _ _ _).mp hk
            exact ⟨hao.trans hk'.1.symm, hau.trans hk'.2.1.symm, has.trans hk'.2.2.symm⟩
          · simp only [hk, Bool.false_eq_true, if_false]
            exact ⟨rfl, rfl, rfl⟩
        · rw [hj, hacc]
          intro e he
          rcases List.mem_append.mp he with he | he
          · obtain ⟨a, ha, hae⟩ := hL.cover e he
            refine ⟨_, List.mem_map.mpr ⟨a, ha, rfl⟩, ?_⟩
            by_cases hk : a.is q.owner q.unit q.scope = true
            · have hk' := (LedgerAccount.is_iff _ _ _ _).mp hk
              simp only [hk, if_true]
              exact ⟨hao.trans (hk'.1.symm.trans hae.1), hau.trans (hk'.2.1.symm.trans hae.2.1),
                has.trans (hk'.2.2.symm.trans hae.2.2)⟩
            · simpa [hk] using hae
          · have : e = entry := by simpa using he
            subst this
            exact ⟨acct, List.mem_map.mpr ⟨a0, ha0m, by simp [ha0i]⟩, hao.trans heo.symm, hau.trans heu.symm,
              has.trans hes.symm⟩
      · rw [if_neg hsome] at hacc
        have hn : L.accounts.find? (fun a => a.is q.owner q.unit q.scope) = none := by
          simpa using hsome
        have hnone : ∀ a ∈ L.accounts, ¬ a.sameKey acct := by
          intro a ha hk
          have := List.find?_eq_none.mp hn a ha
          apply this
          exact (LedgerAccount.is_iff _ _ _ _).mpr ⟨hk.1.trans hao, hk.2.1.trans hau, hk.2.2.trans has⟩
        have hbefore : ((L.accounts.find? (fun a => a.is q.owner q.unit q.scope)).map (fun a => a.balance)).getD 0
            = acctSum L.journal q.owner q.unit q.scope := by
          rw [hn]
          symm
          apply acctSum_zero_of_none
          intro e he hk
          obtain ⟨a, ha, hae⟩ := hL.cover e he
          exact hnone a ha ⟨hae.1.trans (hk.1.trans hao.symm), hae.2.1.trans (hk.2.1.trans hau.symm),
            hae.2.2.trans (hk.2.2.trans has.symm)⟩
        refine ⟨?_, ?_, ?_⟩
        · rw [hacc]
          intro a ha
          rcases List.mem_append.mp ha with ha | ha
          · rw [hL.bal a ha, hsum, if_neg]
            · simp
            · intro hh
              exact hnone a ha ⟨hh.1.trans hao.symm, hh.2.1.trans hau.symm, hh.2.2.trans has.symm⟩
          · have : a = acct := by simpa using ha
            subst this
            rw [hb, hbefore, hsum, if_pos ⟨hao, hau, has⟩, hao, hau, has]
        · rw [hacc]
          rw [List.pairwise_append]
          refine ⟨hL.uniq, by simp, ?_⟩
          intro a ha b hb'
          have : b = acct := by simpa using hb'
          subst this
          exact hnone a ha
        · rw [hj, hacc]
          intro e he
          rcases List.mem_append.mp he with he | he
          · obtain ⟨a, ha, hae⟩ := hL.cover e he
            exact ⟨a, List.mem_append.mpr (Or.inl ha), hae⟩
          · have : e = entry := by simpa using he
            subst this
            exact ⟨acct, by simp, hao.trans heo.symm, hau.trans heu.symm, has.trans hes.symm⟩
end Strata
namespace Strata

theorem postFold_sound (t : Tenant) (actor : Principal) (now : Int) :
    ∀ (ps : List LedgerPosting) (acc : LedgerState × List LedgerPosted) (r : LedgerState × List LedgerPosted),
      List.foldlM (fun (acc : LedgerState × List LedgerPosted) (q : LedgerPosting) =>
        (postOne t actor now acc.1 q).map (fun r => (r.1, acc.2 ++ r.2))) acc ps = .ok r →
      LedgerSound acc.1 → LedgerSound r.1 := by
  intro ps
  induction ps with
  | nil =>
    intro acc r h hs
    simp only [List.foldlM, pure, Except.pure, Except.ok.injEq] at h
    subst h; exact hs
  | cons q qs ih =>
    intro acc r h hs
    simp only [List.foldlM_cons] at h
    cases hp : postOne t actor now acc.1 q with
    | error e => simp [hp, Except.map, Except.bind, bind] at h
    | ok x =>
      simp only [hp, Except.map, Except.bind, bind] at h
      exact ih _ r h (postOne_sound hp hs)

/-- An accepted `PostLedger` leaves the ledger sound when it was. -/
theorem postLedgerCore_sound {t : Tenant} {actor : Principal} {now : Int} {ps : List LedgerPosting}
    {L' : LedgerState} {evs : List LedgerPosted} (h : postLedgerCore t actor now ps = .ok (L', evs))
    (hs : LedgerSound t.ledger) : LedgerSound L' := by
  unfold postLedgerCore at h
  split at h
  · cases h
  · split at h
    · cases h
    · exact postFold_sound t actor now ps _ _ h hs
end Strata
namespace Strata

theorem keyed_snd_sublist (accts : List LedgerAccount) (unit : Nat) (cutoff : Int) :
    ((((accts.filter (fun a => a.unit == unit)).filterMap (fun a =>
      match oldestBucketStart? a with
      | some s => if s ≤ cutoff then some (s, a) else none
      | none => none)).map (fun x => x.2))).Sublist accts := by
  have gen : ∀ l : List LedgerAccount, ((l.filterMap (fun a =>
      match oldestBucketStart? a with
      | some s => if s ≤ cutoff then some (s, a) else none
      | none => none)).map (fun x => x.2)).Sublist l := by
    intro l
    induction l with
    | nil => simp
    | cons a l ih =>
      simp only [List.filterMap_cons]
      cases hs : oldestBucketStart? a with
      | none => exact ih.cons _
      | some s =>
        by_cases hc : s ≤ cutoff
        · simp only [hc, if_true, List.map_cons]
          exact ih.cons₂ _
        · simp only [hc, if_false]
          exact ih.cons _
  exact (gen _).trans (List.filter_sublist)

theorem dueAccounts_facts (accts : List LedgerAccount) (unit : Nat) (cutoff : Int) :
    (∀ d ∈ dueAccounts accts unit cutoff, d ∈ accts) ∧
    (accts.Pairwise (fun a b => ¬ a.sameKey b) → (dueAccounts accts unit cutoff).Pairwise (fun a b => ¬ a.sameKey b)) := by
  unfold dueAccounts
  simp only []
  generalize hk : ((accts.filter (fun a => a.unit == unit)).filterMap (fun a =>
      match oldestBucketStart? a with
      | some s => if s ≤ cutoff then some (s, a) else none
      | none => none)) = keyed
  have hsub : (keyed.map (fun x => x.2)).Sublist accts := hk ▸ keyed_snd_sublist accts unit cutoff
  have hperm : ((keyed.mergeSort (fun x y => decide (x.1 ≤ y.1))).map (fun x => x.2)).Perm (keyed.map (fun x => x.2)) :=
    (List.mergeSort_perm _ _).map _
  constructor
  · intro d hd
    have h1 := List.mem_of_mem_take hd
    exact hsub.subset (hperm.mem_iff.mp h1)
  · intro hp
    have h1 : (keyed.map (fun x => x.2)).Pairwise (fun a b => ¬ a.sameKey b) := hp.sublist hsub
    have h2 := hperm.pairwise_iff (fun {a b} hab hba => hab (sameKey_symm hba)) |>.mpr h1
    exact h2.sublist (List.take_sublist _ _)


theorem expireAccount_facts (e : LedgerExpiry) (t cutoff : Int) (a : LedgerAccount) :
    (expireAccount e t cutoff a).1.owner = a.owner ∧ (expireAccount e t cutoff a).1.unit = a.unit ∧
    (expireAccount e t cutoff a).1.scope = a.scope ∧
    (∀ x ∈ (expireAccount e t cutoff a).2, x.1.owner = a.owner ∧ x.1.unit = a.unit ∧ x.1.scope = a.scope) ∧
    (expireAccount e t cutoff a).1.balance =
      a.balance + (((expireAccount e t cutoff a).2).map (fun x => x.1.amount)).sum := by
  have gen : ∀ (ds : List LedgerBucket) (acc : LedgerAccount × List (LedgerEntry × LedgerPosted)),
      acc.1.owner = a.owner → acc.1.unit = a.unit → acc.1.scope = a.scope →
      (∀ x ∈ acc.2, x.1.owner = a.owner ∧ x.1.unit = a.unit ∧ x.1.scope = a.scope) →
      acc.1.balance = a.balance + (acc.2.map (fun x => x.1.amount)).sum →
      (ds.foldl (expireStep e t) acc).1.owner = a.owner ∧ (ds.foldl (expireStep e t) acc).1.unit = a.unit ∧
      (ds.foldl (expireStep e t) acc).1.scope = a.scope ∧
      (∀ x ∈ (ds.foldl (expireStep e t) acc).2, x.1.owner = a.owner ∧ x.1.unit = a.unit ∧ x.1.scope = a.scope) ∧
      (ds.foldl (expireStep e t) acc).1.balance =
        a.balance + ((ds.foldl (expireStep e t) acc).2.map (fun x => x.1.amount)).sum := by
    intro ds
    induction ds with
    | nil => intro acc h1 h2 h3 h4 h5; exact ⟨h1, h2, h3, h4, h5⟩
    | cons b bs ih =>
      intro acc h1 h2 h3 h4 h5
      apply ih
      · simpa [expireStep] using h1
      · simpa [expireStep] using h2
      · simpa [expireStep] using h3
      · intro x hx
        simp only [expireStep, List.mem_append, List.mem_singleton] at hx
        rcases hx with hx | hx
        · exact h4 x hx
        · subst hx; exact ⟨h1, h2, h3⟩
      · simp only [expireStep, List.map_append, List.sum_append, List.map_cons, List.map_nil, List.sum_cons,
          List.sum_nil]
        rw [h5]; omega
  unfold expireAccount
  exact gen _ (a, []) rfl rfl rfl (by simp) (by simp)

end Strata
namespace Strata

theorem expireCore_state {t : Tenant} {actor : Principal} {now : Int} {unit : Nat} {cutoffAt : Int}
    {L' : LedgerState} {evs : List LedgerPosted} (h : expireCore t actor now unit cutoffAt = .ok (L', evs)) :
    ∃ e : LedgerExpiry,
      L'.accounts = t.ledger.accounts.map (fun a =>
        match ((dueAccounts t.ledger.accounts unit cutoffAt).map (fun d => (expireAccount e now cutoffAt d).1)).find?
            (fun h => h.is a.owner a.unit a.scope) with
        | some h => h
        | none => a) ∧
      L'.journal = t.ledger.journal ++
        (((dueAccounts t.ledger.accounts unit cutoffAt).map (fun d => expireAccount e now cutoffAt d)).flatMap
          (fun r => r.2)).map (fun x => x.1) := by
  unfold expireCore at h
  simp only [] at h
  repeat' split at h
  all_goals first
    | (simp only [Except.ok.injEq, Prod.mk.injEq] at h
       obtain ⟨rfl, -⟩ := h
       refine ⟨‹LedgerExpiry›, ?_, ?_⟩
       · simp only [List.map_map, Function.comp_def] <;> rfl
       · simp only [List.map_map, Function.comp_def] <;> rfl)
    | (cases h; done)
end Strata
namespace Strata

theorem acctSum_keyed (es : List LedgerEntry) (o0 : LedgerOwner) (u0 : Nat) (s0 : String)
    (h : ∀ x ∈ es, x.owner = o0 ∧ x.unit = u0 ∧ x.scope = s0) (o : LedgerOwner) (u : Nat) (s : String) :
    acctSum es o u s = if (o0 = o ∧ u0 = u ∧ s0 = s) then (es.map (fun x => x.amount)).sum else 0 := by
  by_cases hk : (o0 = o ∧ u0 = u ∧ s0 = s)
  · rw [if_pos hk]
    have : es.filter (fun e => e.inAccount o u s) = es := by
      rw [List.filter_eq_self]
      intro x hx
      have := h x hx
      exact (LedgerEntry.inAccount_iff _ _ _ _).mpr ⟨this.1.trans hk.1, this.2.1.trans hk.2.1, this.2.2.trans hk.2.2⟩
    simp [acctSum, this]
  · rw [if_neg hk]
    apply acctSum_zero_of_none
    intro x hx hx'
    have := h x hx
    exact hk ⟨this.1.symm.trans hx'.1, this.2.1.symm.trans hx'.2.1, this.2.2.symm.trans hx'.2.2⟩

end Strata
namespace Strata

theorem sameKey_refl (a : LedgerAccount) : a.sameKey a := ⟨rfl, rfl, rfl⟩

theorem flat_sum (R : LedgerAccount → LedgerAccount × List (LedgerEntry × LedgerPosted))
    (hR : ∀ d, ∀ x ∈ (R d).2, x.1.owner = d.owner ∧ x.1.unit = d.unit ∧ x.1.scope = d.scope)
    (a : LedgerAccount) :
    ∀ ds : List LedgerAccount, ds.Pairwise (fun x y => ¬ x.sameKey y) → (∀ d ∈ ds, d.sameKey a → d = a) →
      acctSum (((ds.map R).flatMap (fun r => r.2)).map (fun x => x.1)) a.owner a.unit a.scope =
        if a ∈ ds then (((R a).2).map (fun x => x.1.amount)).sum else 0 := by
  intro ds
  induction ds with
  | nil => intro _ _; simp [acctSum_nil]
  | cons d ds ih =>
    intro hp hu
    rw [List.pairwise_cons] at hp
    simp only [List.map_cons, List.flatMap_cons, List.map_append, acctSum_append]
    have h1 := acctSum_keyed ((R d).2.map (fun x => x.1)) d.owner d.unit d.scope
      (by intro x hx; obtain ⟨y, hy, rfl⟩ := List.mem_map.mp hx; exact hR d y hy) a.owner a.unit a.scope
    rw [h1, ih hp.2 (fun x hx => hu x (List.mem_cons_of_mem _ hx))]
    by_cases hk : d.sameKey a
    · have hda := hu d (List.mem_cons_self) hk
      subst hda
      have : d ∉ ds := fun hm => hp.1 d hm (sameKey_refl d)
      simp [this, List.map_map, Function.comp_def]
    · have hk' : ¬ (d.owner = a.owner ∧ d.unit = a.unit ∧ d.scope = a.scope) := hk
      have : a ≠ d := fun h => hk (h ▸ sameKey_refl a)
      simp [hk', List.mem_cons, this]
end Strata
namespace Strata

theorem expire_replace (accts due : List LedgerAccount)
    (R : LedgerAccount → LedgerAccount × List (LedgerEntry × LedgerPosted))
    (hR : ∀ d, (R d).1.sameKey d) (hsub : ∀ d ∈ due, d ∈ accts)
    (hu : accts.Pairwise (fun a b => ¬ a.sameKey b)) (a : LedgerAccount) (ha : a ∈ accts) :
    (match (due.map (fun d => (R d).1)).find? (fun h => h.is a.owner a.unit a.scope) with
      | some h => h
      | none => a) = if a ∈ due then (R a).1 else a := by
  rw [List.find?_map]
  cases hf : due.find? ((fun h : LedgerAccount => h.is a.owner a.unit a.scope) ∘ (fun d => (R d).1)) with
  | some d =>
    have hd := List.mem_of_find?_eq_some hf
    have hp := List.find?_some hf
    simp only [Function.comp, LedgerAccount.is_iff] at hp
    have hk : d.sameKey a := sameKey_trans (sameKey_symm (hR d)) ⟨hp.1, hp.2.1, hp.2.2⟩
    have hda : d = a := uniq_of_pairwise hu (hsub d hd) ha hk
    subst hda
    simp [hd]
  | none =>
    have hn : a ∉ due := by
      intro hm
      have := List.find?_eq_none.mp hf a hm
      apply this
      simp only [Function.comp, decide_eq_true_eq, LedgerAccount.is_iff]
      have := hR a
      exact ⟨this.1, this.2.1, this.2.2⟩
    simp [hn]

end Strata
namespace Strata

/-- An accepted `ExpireLedger` leaves the ledger sound when it was. -/
theorem expireCore_sound {t : Tenant} {actor : Principal} {now : Int} {unit : Nat} {cutoffAt : Int}
    {L' : LedgerState} {evs : List LedgerPosted} (h : expireCore t actor now unit cutoffAt = .ok (L', evs))
    (hs : LedgerSound t.ledger) : LedgerSound L' := by
  obtain ⟨e, hacc, hj⟩ := expireCore_state h
  obtain ⟨hsub, hpw⟩ := dueAccounts_facts t.ledger.accounts unit cutoffAt
  have hpw' := hpw hs.uniq
  generalize dueAccounts t.ledger.accounts unit cutoffAt = due at hacc hj hsub hpw'
  have facts := expireAccount_facts e now cutoffAt
  have hR : ∀ d, (expireAccount e now cutoffAt d).1.sameKey d := fun d =>
    ⟨(facts d).1, (facts d).2.1, (facts d).2.2.1⟩
  have hrepl := expire_replace t.ledger.accounts due (fun d => expireAccount e now cutoffAt d) hR hsub hs.uniq
  have hFk : ∀ a ∈ t.ledger.accounts,
      (match (due.map (fun d => (expireAccount e now cutoffAt d).1)).find?
          (fun (h : LedgerAccount) => h.is a.owner a.unit a.scope) with
        | some h => h
        | none => a).sameKey a := by
    intro a ha
    rw [hrepl a ha]
    by_cases hm : a ∈ due
    · rw [if_pos hm]; exact hR a
    · rw [if_neg hm]; exact sameKey_refl a
  have hentries : ∀ d, ∀ x ∈ (expireAccount e now cutoffAt d).2,
      x.1.owner = d.owner ∧ x.1.unit = d.unit ∧ x.1.scope = d.scope := fun d => (facts d).2.2.2.1
  refine ⟨?_, ?_, ?_⟩
  · rw [hacc]
    intro a' ha'
    obtain ⟨a, ha, rfl⟩ := List.mem_map.mp ha'
    have hk := hFk a ha
    rw [hj, acctSum_append]
    have hkey : ∀ (x : LedgerAccount), x.sameKey a →
        acctSum (((due.map (fun d => expireAccount e now cutoffAt d)).flatMap (fun r => r.2)).map (fun x => x.1))
          x.owner x.unit x.scope =
        if a ∈ due then (((expireAccount e now cutoffAt a).2).map (fun x => x.1.amount)).sum else 0 := by
      intro x hx
      rw [hx.1, hx.2.1, hx.2.2]
      exact flat_sum (fun d => expireAccount e now cutoffAt d) hentries a due hpw'
        (fun d hd hda => uniq_of_pairwise hs.uniq (hsub d hd) ha hda)
    rw [hkey _ hk, hrepl a ha]
    by_cases hm : a ∈ due
    · rw [if_pos hm, if_pos hm, (facts a).2.2.2.2, hs.bal a ha]
      rw [(facts a).1, (facts a).2.1, (facts a).2.2.1]
    · rw [if_neg hm, if_neg hm, hs.bal a ha]; simp
  · rw [hacc]
    exact pairwise_map_sameKey _ hFk hs.uniq
  · rw [hj, hacc]
    intro x hx
    rcases List.mem_append.mp hx with hx | hx
    · obtain ⟨a, ha, hae⟩ := hs.cover x hx
      refine ⟨_, List.mem_map.mpr ⟨a, ha, rfl⟩, ?_⟩
      have hk := hFk a ha
      exact ⟨hk.1.trans hae.1, hk.2.1.trans hae.2.1, hk.2.2.trans hae.2.2⟩
    · obtain ⟨y, hy, rfl⟩ := List.mem_map.mp hx
      obtain ⟨r, hr, hyr⟩ := List.mem_flatMap.mp hy
      obtain ⟨d, hd, rfl⟩ := List.mem_map.mp hr
      have hk := hFk d (hsub d hd)
      have hy' := hentries d y hyr
      refine ⟨_, List.mem_map.mpr ⟨d, hsub d hd, rfl⟩, ?_⟩
      exact ⟨hk.1.trans hy'.1.symm, hk.2.1.trans hy'.2.1.symm, hk.2.2.trans hy'.2.2.symm⟩
end Strata
namespace Strata

/-- The empty ledger is sound. -/
theorem ledgerSound_empty : LedgerSound ({} : LedgerState) :=
  ⟨by simp, List.Pairwise.nil, by simp⟩

/-- An accepted `PostLedger` leaves the tenant's ledger sound when it was: each account's balance is the sum of its
    entries' amounts, no account is held twice, and every entry is in an account that has a header. -/
theorem postLedgerT_sound {t t' : Tenant} {p : Principal} {now : Int} {ps : List LedgerPosting}
    (h : postLedgerT t p now ps = .ok t') (hs : LedgerSound t.ledger) : LedgerSound t'.ledger := by
  unfold postLedgerT at h
  cases hc : postLedgerCore t p now ps with
  | error e => simp [hc, Except.map] at h
  | ok r =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    subst h
    exact postLedgerCore_sound hc hs

/-- An accepted `ExpireLedger` leaves the tenant's ledger sound when it was. -/
theorem expireLedgerT_sound {t t' : Tenant} {p : Principal} {now : Int} {u : Nat} {c : Int}
    (h : expireLedgerT t p now u c = .ok t') (hs : LedgerSound t.ledger) : LedgerSound t'.ledger := by
  unfold expireLedgerT at h
  cases hc : expireCore t p now u c with
  | error e => simp [hc, Except.map] at h
  | ok r =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    subst h
    exact expireCore_sound hc hs

end Strata

/-! Axioms of the slice 13 theorems: only propext, Classical.choice and Quot.sound. -/
#print axioms Strata.postOne_total
#print axioms Strata.ledger_post_atomic
#print axioms Strata.ledger_replay_unchanged
#print axioms Strata.expireAccount_debits_due
#print axioms Strata.expireCore_total
#print axioms Strata.read_refused_when_unpermitted
#print axioms Strata.postDynamic_new
#print axioms Strata.floorErr_none
#print axioms Strata.capErr_none
#print axioms Strata.periodErr_none
#print axioms Strata.postDynamic_state
#print axioms Strata.LedgerAccount.is_iff
#print axioms Strata.LedgerEntry.inAccount_iff
#print axioms Strata.find?_map_replace
#print axioms Strata.find?_map_hit
#print axioms Strata.postOne_frame
#print axioms Strata.postOne_limits
#print axioms Strata.acctSum_append
#print axioms Strata.acctSum_nil
#print axioms Strata.acctSum_single_same
#print axioms Strata.acctSum_single_other
#print axioms Strata.acctSum_zero_of_none
#print axioms Strata.sameKey_symm
#print axioms Strata.sameKey_trans
#print axioms Strata.pairwise_map_sameKey
#print axioms Strata.postDynamic_replay
#print axioms Strata.uniq_of_pairwise
#print axioms Strata.postOne_sound
#print axioms Strata.postFold_sound
#print axioms Strata.postLedgerCore_sound
#print axioms Strata.keyed_snd_sublist
#print axioms Strata.dueAccounts_facts
#print axioms Strata.expireAccount_facts
#print axioms Strata.expireCore_state
#print axioms Strata.acctSum_keyed
#print axioms Strata.sameKey_refl
#print axioms Strata.flat_sum
#print axioms Strata.expire_replace
#print axioms Strata.expireCore_sound
#print axioms Strata.ledgerSound_empty
#print axioms Strata.postLedgerT_sound
#print axioms Strata.expireLedgerT_sound
