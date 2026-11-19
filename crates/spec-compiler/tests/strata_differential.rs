//! Differential test of Strata's own implementation machines: every scenario of
//! `specifications/spec.jsonc` that drives only implementation-level machines is run by Exegra
//! (`node $EXEGRA_HOME/cli/exegra.js trace <scenario>`), and the generated machines are stepped
//! through the same trace:
//!
//! - the server machines, as Rust: `strata-model`'s `machines` (`crates/model`);
//! - the client machines, as JavaScript, run by `node` through
//!   `tests/strata-differential-node.mjs`: `web/src/generated/client-*-flow-impl.js`.
//!
//! The specification under test is that of the checkout the test runs in. Exegra reads a project
//! named by a `project.jsonc` link, so the test writes a link to this checkout's
//! `specifications/spec.jsonc` into `var/specc-differential/specs/checkout` (the repository's
//! ignored `var` directory) and runs Exegra with `--specs` on it, never `--project strata`
//! (which would read whatever checkout the Exegra `strata` project points at).
//!
//! The trace names the machines each event is delivered to (the step's `fired` and `ignored`
//! lists, one entry per machine instance); only those are stepped, with the event at the step's
//! time. After every step the generated machine must agree with Exegra on:
//!
//! - the state and the variables;
//! - the emitted events, in order, with their payloads, receivers and deliveries (from the
//!   specification), and the delay of a delayed emit;
//! - the timers set (event, payload, delay) and the timers pending (for machines that are not
//!   instanced);
//! - an event Exegra ignores: nothing changes and nothing is asked of the runtime.
//!
//! An event that Exegra routes to a machine but that the machine has no transition on and does
//! not ignore is not an event of the generated machine, which must refuse it (Rust cannot decode
//! it, the JavaScript machine throws `invalid-event`); Exegra must have ignored it.
//!
//! Numbers are compared by value (`1` equals `1.0`), absent optional members equal null ones.
//!
//! Scenarios with a fault that changes the model rather than the input (`enable-transition`,
//! `disable-transition`) cannot be replayed against generated code; they are listed in the
//! output as not driven. Every other scenario is driven, and the test fails when a scenario
//! cannot be, or when a machine of the specification fires in none of them.
//!
//! Fails when Exegra or `node` is missing (see `support`); skipped, with a loud banner, only when
//! `SPECC_SKIP_EXEGRA_TESTS=1` or `SPECC_SKIP_NODE_TESTS=1` says so. `STRATA_DIFFERENTIAL_JOBS`
//! sets how many Exegra traces run at once (default 2).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::marker::PhantomData;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use strata_runtime::{Machine, Time, TimerCommand};
use strata_spec_compiler::ir::Spec;
use strata_spec_compiler::wire::{self, WireCase, WireValues};
use strata_spec_compiler::{jsonc, load};

// ---- the generated machines behind one interface -----------------------------------------

/// One emit of a step: the receiver, the delivery, the event (`{event, payload}`) and its delay.
#[derive(Debug, Clone)]
struct EmitOut {
    receiver: String,
    delivery: String,
    event: Value,
    after_ms: i64,
}

/// One timer command of a step.
#[derive(Debug, Clone)]
enum TimerOut {
    Set {
        name: String,
        after_ms: i64,
        event: Value,
    },
    Cancel {
        name: String,
    },
}

/// What a generated machine did with an event.
#[derive(Debug, Clone)]
struct Stepped {
    state: String,
    vars: Value,
    emits: Vec<EmitOut>,
    timers: Vec<TimerOut>,
    /// The machine changed nothing and asked for nothing (JavaScript says so itself; for Rust it
    /// is read off the outcome).
    ignored: bool,
}

enum StepResult {
    /// The event is not one the machine receives, or its payload is not of the declared types.
    Refused(String),
    Stepped(Stepped),
}

/// A Rust machine with its types erased, speaking JSON.
trait ErasedMachine: Send + Sync {
    fn name(&self) -> &'static str;
    fn initial(&self, key: &Value) -> Result<(String, Value), String>;
    fn step(
        &self,
        state: &str,
        vars: &Value,
        event: &Value,
        now: i64,
    ) -> Result<StepResult, String>;
}

struct Typed<M>(PhantomData<M>);

impl<M> ErasedMachine for Typed<M>
where
    M: Machine,
    M::State: Serialize + DeserializeOwned,
    M::Vars: Serialize + DeserializeOwned,
{
    fn name(&self) -> &'static str {
        M::NAME
    }

    fn initial(&self, key: &Value) -> Result<(String, Value), String> {
        let key: M::Key = serde_json::from_value(key.clone())
            .map_err(|e| format!("the placeholder key {key} is not a key of {}: {e}", M::NAME))?;
        let (state, vars) = M::initial(&key);
        let state = serde_json::to_value(&state).map_err(|e| e.to_string())?;
        let state = state
            .as_str()
            .ok_or_else(|| format!("the state {state} is not a string"))?
            .to_string();
        Ok((
            state,
            serde_json::to_value(&vars).map_err(|e| e.to_string())?,
        ))
    }

    fn step(
        &self,
        state: &str,
        vars: &Value,
        event: &Value,
        now: i64,
    ) -> Result<StepResult, String> {
        let state_value = Value::String(state.to_string());
        let state_typed: M::State = serde_json::from_value(state_value.clone())
            .map_err(|e| format!("{state:?} is not a state of {}: {e}", M::NAME))?;
        let vars_typed: M::Vars = serde_json::from_value(vars.clone())
            .map_err(|e| format!("the variables {vars} are not those of {}: {e}", M::NAME))?;
        let event_typed: M::Event = match serde_json::from_value(event.clone()) {
            Ok(event) => event,
            Err(e) => {
                return Ok(StepResult::Refused(format!(
                    "not an event of the machine: {e}"
                )))
            }
        };
        let outcome = match M::step(&state_typed, &vars_typed, &event_typed, Time(now)) {
            Ok(outcome) => outcome,
            Err(e) => return Ok(StepResult::Refused(format!("the step refused: {e}"))),
        };
        let out_state = serde_json::to_value(&outcome.state).map_err(|e| e.to_string())?;
        let out_vars = serde_json::to_value(&outcome.vars).map_err(|e| e.to_string())?;
        let mut emits = Vec::new();
        for e in &outcome.emits {
            emits.push(EmitOut {
                receiver: e.receiver.as_str().to_string(),
                delivery: e.delivery.name().to_string(),
                event: serde_json::to_value(&e.event).map_err(|e| e.to_string())?,
                after_ms: e
                    .after
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
            });
        }
        let mut timers = Vec::new();
        for t in &outcome.timers {
            timers.push(match t {
                TimerCommand::Set { name, after, event } => TimerOut::Set {
                    name: name.clone(),
                    after_ms: i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
                    event: serde_json::to_value(event).map_err(|e| e.to_string())?,
                },
                TimerCommand::Cancel { name } => TimerOut::Cancel { name: name.clone() },
            });
        }
        let ignored =
            out_state == state_value && out_vars == *vars && emits.is_empty() && timers.is_empty();
        Ok(StepResult::Stepped(Stepped {
            state: out_state
                .as_str()
                .ok_or_else(|| format!("the state {out_state} is not a string"))?
                .to_string(),
            vars: out_vars,
            emits,
            timers,
            ignored,
        }))
    }
}

macro_rules! registry {
    ($($path:ident :: $module:ident :: $machine:ident),* $(,)?) => {
        /// Every server machine of the model, by behavior id: the shipped ones (`machines`) and
        /// the target ones (`target_machines`, which the `target-models` feature of
        /// `strata-model` compiles and this crate's manifest enables).
        fn rust_machines() -> BTreeMap<String, Box<dyn ErasedMachine>> {
            let all: Vec<Box<dyn ErasedMachine>> = vec![
                $(Box::new(Typed::<strata_model::$path::$module::$machine>(PhantomData))),*
            ];
            all.into_iter().map(|m| (m.name().to_string(), m)).collect()
        }
    };
}

registry! {
    machines::gateway_session_flow_impl::GatewaySessionFlowImpl,
    machines::origin_content_flow_impl::OriginContentFlowImpl,
    machines::arc_outbox_delivery_flow_impl::ArcOutboxDeliveryFlowImpl,
    machines::arc_rolling_change_flow_impl::ArcRollingChangeFlowImpl,
    machines::preview_link_flow_impl::PreviewLinkFlowImpl,
    machines::gateway_outage_flow_impl::GatewayOutageFlowImpl,
    machines::token_render_flow_impl::TokenRenderFlowImpl,
    machines::commit_flow_impl::CommitFlowImpl,
    machines::template_install_flow_impl::TemplateInstallFlowImpl,
    machines::mail_render_flow_impl::MailRenderFlowImpl,
    machines::runtime_credential_flow_impl::RuntimeCredentialFlowImpl,
    machines::kernel_upgrade_flow_impl::KernelUpgradeFlowImpl,
    machines::draft_save_flow_impl::DraftSaveFlowImpl,
    machines::asrt_flow_impl::AsrtFlowImpl,
    machines::token_verifier_flow_impl::TokenVerifierFlowImpl,
    target_machines::emailed_link_flow_impl::EmailedLinkFlowImpl,
    target_machines::oidc_callback_flow_impl::OidcCallbackFlowImpl,
    target_machines::session_authority_flow_impl::SessionAuthorityFlowImpl,
    target_machines::job_lease_flow_impl::JobLeaseFlowImpl,
    target_machines::edge_copy_flow_impl::EdgeCopyFlowImpl,
    target_machines::provisioning_flow_impl::ProvisioningFlowImpl,
    target_machines::proposal_flow_impl::ProposalFlowImpl,
    target_machines::arc_outbox_consumer_flow_impl::ArcOutboxConsumerFlowImpl,
    target_machines::inbound_run_flow_impl::InboundRunFlowImpl,
    target_machines::cv_change_flow_impl::CvChangeFlowImpl,
    target_machines::cv_authority_flow_impl::CvAuthorityFlowImpl,
    target_machines::saml_assertion_flow_impl::SamlAssertionFlowImpl,
    target_machines::guess_throttle_flow_impl::GuessThrottleFlowImpl,
    target_machines::trigger_chain_flow_impl::TriggerChainFlowImpl,
    target_machines::erasure_copy_flow_impl::ErasureCopyFlowImpl,
    target_machines::subject_key_flow_impl::SubjectKeyFlowImpl,
    target_machines::audit_chain_flow_impl::AuditChainFlowImpl,
    target_machines::agent_halt_flow_impl::AgentHaltFlowImpl,
    target_machines::approval_separation_flow_impl::ApprovalSeparationFlowImpl,
    target_machines::mail_dispatch_flow_impl::MailDispatchFlowImpl,
    target_machines::mail_adapter_flow_impl::MailAdapterFlowImpl,
    target_machines::colour_switch_flow_impl::ColourSwitchFlowImpl,
    target_machines::release_promotion_flow_impl::ReleasePromotionFlowImpl,
    target_machines::migration_batch_flow_impl::MigrationBatchFlowImpl,
    target_machines::tenant_move_flow_impl::TenantMoveFlowImpl,
    target_machines::source_throttle_flow_impl::SourceThrottleFlowImpl,
    target_machines::job_run_flow_impl::JobRunFlowImpl,
    target_machines::schedule_flow_impl::ScheduleFlowImpl,
    target_machines::trigger_commit_flow_impl::TriggerCommitFlowImpl,
    target_machines::lease_reach_flow_impl::LeaseReachFlowImpl,
    target_machines::job_admission_flow_impl::JobAdmissionFlowImpl,
    target_machines::wake_flow_impl::WakeFlowImpl,
    target_machines::bulk_change_flow_impl::BulkChangeFlowImpl,
    target_machines::workflow_definition_flow_impl::WorkflowDefinitionFlowImpl,
    target_machines::workflow_activation_flow_impl::WorkflowActivationFlowImpl,
    target_machines::workflow_run_flow_impl::WorkflowRunFlowImpl,
    target_machines::workflow_compensation_flow_impl::WorkflowCompensationFlowImpl,
    target_machines::approval_flow_impl::ApprovalFlowImpl,
    target_machines::approval_inbox_flow_impl::ApprovalInboxFlowImpl,
    target_machines::call_step_flow_impl::CallStepFlowImpl,
    target_machines::outbound_call_flow_impl::OutboundCallFlowImpl,
    target_machines::webhook_delivery_flow_impl::WebhookDeliveryFlowImpl,
    target_machines::scheduled_publication_flow_impl::ScheduledPublicationFlowImpl,
    target_machines::inbound_signature_flow_impl::InboundSignatureFlowImpl,
    target_machines::mail_enqueue_flow_impl::MailEnqueueFlowImpl,
    target_machines::mail_attempt_flow_impl::MailAttemptFlowImpl,
    target_machines::mail_quota_flow_impl::MailQuotaFlowImpl,
    target_machines::mail_provider_flow_impl::MailProviderFlowImpl,
    target_machines::mail_log_read_flow_impl::MailLogReadFlowImpl,
    target_machines::sender_domain_flow_impl::SenderDomainFlowImpl,
    target_machines::delivery_report_flow_impl::DeliveryReportFlowImpl,
    target_machines::audience_mail_flow_impl::AudienceMailFlowImpl,
    target_machines::tenant_suspension_flow_impl::TenantSuspensionFlowImpl,
    target_machines::write_quota_flow_impl::WriteQuotaFlowImpl,
    target_machines::plan_change_flow_impl::PlanChangeFlowImpl,
    target_machines::connection_quota_flow_impl::ConnectionQuotaFlowImpl,
    target_machines::bootstrap_credential_flow_impl::BootstrapCredentialFlowImpl,
    target_machines::placement_choice_flow_impl::PlacementChoiceFlowImpl,
    target_machines::tenant_deletion_flow_impl::TenantDeletionFlowImpl,
    target_machines::backup_key_flow_impl::BackupKeyFlowImpl,
    target_machines::export_restore_flow_impl::ExportRestoreFlowImpl,
    target_machines::point_in_time_flow_impl::PointInTimeFlowImpl,
    target_machines::placement_switch_flow_impl::PlacementSwitchFlowImpl,
    target_machines::schema_change_flow_impl::SchemaChangeFlowImpl,
    target_machines::layout_rollout_flow_impl::LayoutRolloutFlowImpl,
    target_machines::staging_promotion_flow_impl::StagingPromotionFlowImpl,
    target_machines::field_change_flow_impl::FieldChangeFlowImpl,
    target_machines::workflow_version_flow_impl::WorkflowVersionFlowImpl,
    target_machines::scheduled_job_check_flow_impl::ScheduledJobCheckFlowImpl,
    target_machines::template_proposal_flow_impl::TemplateProposalFlowImpl,
    target_machines::import_flow_impl::ImportFlowImpl,
    target_machines::samlv_flow_impl::SamlvFlowImpl,
    target_machines::oidcv_flow_impl::OidcvFlowImpl,
    target_machines::oidcs_flow_impl::OidcsFlowImpl,
    target_machines::fedp_flow_impl::FedpFlowImpl,
    target_machines::fedc_flow_impl::FedcFlowImpl,
    target_machines::fedl_flow_impl::FedlFlowImpl,
    target_machines::prov_flow_impl::ProvFlowImpl,
    target_machines::idpa_flow_impl::IdpaFlowImpl,
    target_machines::tokiss_flow_impl::TokissFlowImpl,
    target_machines::tokact_flow_impl::TokactFlowImpl,
    target_machines::tokint_flow_impl::TokintFlowImpl,
    target_machines::stepup_flow_impl::StepupFlowImpl,
    target_machines::facg_flow_impl::FacgFlowImpl,
    target_machines::realm_flow_impl::RealmFlowImpl,
    target_machines::auditk_flow_impl::AuditkFlowImpl,
    target_machines::ppreg_flow_impl::PpregFlowImpl,
    target_machines::ppauth_flow_impl::PpauthFlowImpl,
    target_machines::session_lifetime_flow_impl::SessionLifetimeFlowImpl,
    target_machines::second_factor_flow_impl::SecondFactorFlowImpl,
    target_machines::recovery_flow_impl::RecoveryFlowImpl,
    target_machines::code_spend_flow_impl::CodeSpendFlowImpl,
    target_machines::mfp_limit_flow_impl::MfpLimitFlowImpl,
    target_machines::lea_limit_flow_impl::LeaLimitFlowImpl,
    target_machines::leb_limit_flow_impl::LebLimitFlowImpl,
    target_machines::sus_limit_flow_impl::SusLimitFlowImpl,
    target_machines::tcl_limit_flow_impl::TclLimitFlowImpl,
    target_machines::realm_rate_flow_impl::RealmRateFlowImpl,
    target_machines::idp_outage_flow_impl::IdpOutageFlowImpl,
    target_machines::provider_keys_flow_impl::ProviderKeysFlowImpl,
    target_machines::idp_offer_flow_impl::IdpOfferFlowImpl,
    target_machines::first_sign_in_role_flow_impl::FirstSignInRoleFlowImpl,
    target_machines::signing_key_flow_impl::SigningKeyFlowImpl,
    target_machines::signup_notice_flow_impl::SignupNoticeFlowImpl,
    target_machines::new_device_flow_impl::NewDeviceFlowImpl,
}

/// The Node process that steps the generated client machines, over lines of JSON.
struct Node {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Node {
    fn start(generated: &Path) -> Node {
        let driver =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/strata-differential-node.mjs");
        let mut child = Command::new("node")
            .arg(driver)
            .arg(generated)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|e| panic!("node could not be run ({e})"));
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Node {
            child,
            stdin,
            stdout,
        }
    }

    fn call(&mut self, request: Value) -> Result<Value, String> {
        writeln!(self.stdin, "{request}").map_err(|e| format!("node closed its input: {e}"))?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("node closed its output: {e}"))?;
        if n == 0 {
            return Err("node exited without answering".to_string());
        }
        serde_json::from_str(&line).map_err(|e| format!("node answered {line:?}: {e}"))
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The generated machines of both targets.
struct Engines {
    rust: BTreeMap<String, Box<dyn ErasedMachine>>,
    generated_js: PathBuf,
    node: Option<Node>,
}

impl Engines {
    fn new(root: &Path) -> Engines {
        Engines {
            rust: rust_machines(),
            generated_js: root.join("web/src/generated"),
            node: None,
        }
    }

    fn is_js(&self, machine: &str) -> bool {
        !self.rust.contains_key(machine)
            && self.generated_js.join(format!("{machine}.js")).is_file()
    }

    fn has(&self, machine: &str) -> bool {
        self.rust.contains_key(machine) || self.is_js(machine)
    }

    fn node(&mut self) -> &mut Node {
        let dir = self.generated_js.clone();
        self.node.get_or_insert_with(|| Node::start(&dir))
    }

    fn initial(&mut self, machine: &str, key: &Value) -> Result<(String, Value), String> {
        if let Some(m) = self.rust.get(machine) {
            return m.initial(key);
        }
        let reply = self
            .node()
            .call(json!({ "op": "initial", "machine": machine, "key": key }))?;
        let ok = node_ok(&reply)?;
        Ok((
            ok["state"]
                .as_str()
                .ok_or("the initial state is not a string")?
                .to_string(),
            ok["vars"].clone(),
        ))
    }

    fn step(
        &mut self,
        machine: &str,
        state: &str,
        vars: &Value,
        event: &Value,
        now: i64,
    ) -> Result<StepResult, String> {
        if let Some(m) = self.rust.get(machine) {
            return m.step(state, vars, event, now);
        }
        let reply = self.node().call(json!({
            "op": "step", "machine": machine, "state": state, "vars": vars, "event": event, "now": now
        }))?;
        if let Some(refused) = reply.get("refused") {
            return Ok(StepResult::Refused(format!(
                "{}: {}",
                refused["kind"].as_str().unwrap_or("?"),
                refused["message"].as_str().unwrap_or("?")
            )));
        }
        let ok = node_ok(&reply)?;
        if ok["ignored"] == json!(true) {
            return Ok(StepResult::Stepped(Stepped {
                state: state.to_string(),
                vars: vars.clone(),
                emits: Vec::new(),
                timers: Vec::new(),
                ignored: true,
            }));
        }
        let mut emits = Vec::new();
        for e in ok["emits"].as_array().ok_or("the outcome has no emits")? {
            emits.push(EmitOut {
                receiver: e["to"]
                    .as_str()
                    .ok_or("an emit has no receiver")?
                    .to_string(),
                delivery: e["delivery"]
                    .as_str()
                    .ok_or("an emit has no delivery")?
                    .to_string(),
                event: json!({ "event": e["event"], "payload": e["payload"] }),
                after_ms: e["afterMs"].as_i64().unwrap_or(0),
            });
        }
        let mut timers = Vec::new();
        for t in ok["timers"].as_array().ok_or("the outcome has no timers")? {
            timers.push(if let Some(name) = t["cancel"].as_str() {
                TimerOut::Cancel {
                    name: name.to_string(),
                }
            } else {
                TimerOut::Set {
                    name: t["set"].as_str().ok_or("a timer has no name")?.to_string(),
                    after_ms: t["afterMs"].as_i64().ok_or("a timer has no delay")?,
                    event: json!({ "event": t["event"], "payload": t["payload"] }),
                }
            });
        }
        Ok(StepResult::Stepped(Stepped {
            state: ok["state"]
                .as_str()
                .ok_or("the outcome has no state")?
                .to_string(),
            vars: ok["vars"].clone(),
            emits,
            timers,
            ignored: false,
        }))
    }
}

fn node_ok(reply: &Value) -> Result<&Value, String> {
    if let Some(crashed) = reply.get("crashed") {
        return Err(format!("the JavaScript machine crashed: {crashed}"));
    }
    reply
        .get("ok")
        .ok_or_else(|| format!("node answered {reply}"))
}

// ---- the specification -----------------------------------------------------------------------

/// What the test reads of one implementation machine from the specification.
struct Info {
    id: String,
    /// Several instances, keyed by an event member.
    instanced: bool,
    /// Events the machine receives: its transitions' events, those its states ignore and those
    /// its timers fire.
    received: BTreeSet<String>,
    transitions: BTreeMap<String, Value>,
    /// A key of the machine's key type, to start an instance with (the initial state and
    /// variables do not depend on the key).
    placeholder_key: Value,
}

struct Scenario {
    id: String,
    machines: Vec<String>,
    /// Why the scenario cannot be replayed against generated code, if it cannot.
    undrivable: Option<String>,
}

struct Specification {
    json: Value,
    typed: Spec,
    cases: BTreeMap<String, WireCase>,
    infos: BTreeMap<String, Info>,
    scenarios: Vec<Scenario>,
}

fn items<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn placeholder(spec: &Value, builtin_or_type: &str, depth: usize) -> Value {
    match builtin_or_type {
        "string" => json!("placeholder"),
        "uuid" => json!("00000000-0000-0000-0000-000000000000"),
        "bool" => json!(false),
        "f64" | "u8" | "u16" | "u32" | "u64" | "i32" | "i64" | "timestamp" | "duration" => json!(0),
        other if depth < 8 => items(spec, "types")
            .iter()
            .find(|t| t["id"] == other)
            .and_then(|t| t["repr"].as_str())
            .map_or(json!("placeholder"), |repr| {
                placeholder(spec, repr, depth + 1)
            }),
        _ => json!("placeholder"),
    }
}

fn info_of(spec: &Value, b: &Value) -> Info {
    let id = b["id"].as_str().unwrap().to_string();
    let mut received = BTreeSet::new();
    let mut transitions = BTreeMap::new();
    for t in items(b, "transitions") {
        if let Some(on) = t["on"].as_str() {
            received.insert(on.to_string());
        }
        for e in items(t, "effects") {
            if e.get("timer").is_some() {
                if let Some(event) = e["emit"].as_str() {
                    received.insert(event.to_string());
                }
            }
        }
        transitions.insert(t["id"].as_str().unwrap().to_string(), t.clone());
    }
    for s in items(b, "states") {
        for ignored in items(s, "ignores") {
            received.insert(ignored.as_str().unwrap().to_string());
        }
    }
    let key = b["instances"]["key"]
        .as_str()
        .or_else(|| b["storage"]["key"].as_str());
    let record = b["storage"]["record"]
        .as_str()
        .or_else(|| b["instances"]["storedIn"].as_str());
    let key_type = items(spec, "records")
        .iter()
        .find(|r| Some(r["id"].as_str().unwrap_or_default()) == record)
        .and_then(|r| {
            items(r, "fields")
                .iter()
                .find(|f| f["name"].as_str() == key)
        })
        .and_then(|f| f["type"].as_str());
    let placeholder_key = key_type.map_or(json!("placeholder"), |t| placeholder(spec, t, 0));
    Info {
        id,
        instanced: b.get("instances").is_some(),
        received,
        transitions,
        placeholder_key,
    }
}

fn read_specification(root: &Path) -> Specification {
    let path = "specifications/spec.jsonc";
    let text = std::fs::read_to_string(root.join(path)).unwrap();
    let json = jsonc::parse(&text).unwrap();
    let typed = load::load(path, &text).unwrap_or_else(|d| {
        panic!(
            "the specification was refused:\n{}",
            d.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    let cases = wire::type_cases(&typed);
    let implementation: Vec<&Value> = items(&json, "behaviors")
        .iter()
        .filter(|b| b["level"] == json!("implementation"))
        .collect();
    let infos: BTreeMap<String, Info> = implementation
        .iter()
        .map(|b| (b["id"].as_str().unwrap().to_string(), info_of(&json, b)))
        .collect();
    let mut scenarios = Vec::new();
    for sc in items(&json, "scenarios") {
        let machines: Vec<String> = items(sc, "behaviors")
            .iter()
            .filter_map(|b| b.as_str().map(str::to_string))
            .collect();
        if machines.is_empty() || !machines.iter().all(|m| infos.contains_key(m)) {
            continue;
        }
        let mut undrivable = None;
        for fault in items(sc, "faults") {
            let kind = items(&json, "faults")
                .iter()
                .find(|f| f["id"] == *fault)
                .and_then(|f| f["kind"].as_str())
                .unwrap_or("unknown");
            if !matches!(kind, "drop-event" | "delay-event" | "inject-event") {
                undrivable = Some(format!(
                    "fault {fault} is of kind {kind}, which changes the model, not the input"
                ));
            }
        }
        scenarios.push(Scenario {
            id: sc["id"].as_str().unwrap().to_string(),
            machines,
            undrivable,
        });
    }
    Specification {
        json,
        typed,
        cases,
        infos,
        scenarios,
    }
}

// ---- Exegra --------------------------------------------------------------------------------

/// `path` with `.` and `..` resolved lexically.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The `--specs` directory whose project `checkout` is the specification of this checkout.
fn specs_for(root: &Path) -> PathBuf {
    let specs = root.join("var").join("specc-differential").join("specs");
    let project = specs.join("checkout");
    std::fs::create_dir_all(&project).unwrap();
    let link = project.join("project.jsonc");
    // From var/specc-differential/specs/checkout to the root: four directories up.
    let text = "// Written by tests/strata_differential.rs: the specification of the checkout the test runs in.\n{ \"spec\": \"../../../../specifications/spec.jsonc\" }\n";
    if std::fs::read_to_string(&link).ok().as_deref() != Some(text) {
        std::fs::write(&link, text).unwrap();
    }
    specs
}

fn exegra_trace(cli: &Path, specs: &Path, scenario: &str) -> Result<Value, String> {
    let output = Command::new("node")
        .arg(cli)
        .args(["trace", scenario, "--project", "checkout", "--specs"])
        .arg(specs)
        .output()
        .map_err(|e| format!("node could not be run ({e})"))?;
    let envelope: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "the Exegra CLI did not print JSON ({e}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    if envelope["ok"] != json!(true) {
        return Err(format!("Exegra refused: {}", envelope["error"]));
    }
    Ok(envelope["result"].clone())
}

// ---- comparing ----------------------------------------------------------------------------

/// A value as both sides compare it: absent optional members are null ones, and numbers compare
/// by value.
fn canon(value: Value) -> Value {
    fn strip(value: Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k, strip(v)))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(strip).collect()),
            other => other,
        }
    }
    support::norm(strip(value))
}

fn same(context: &str, what: &str, got: &Value, want: &Value) -> Result<(), String> {
    let (got, want) = (canon(got.clone()), canon(want.clone()));
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{context}: {what}\n  generated: {got}\n  exegra:    {want}"
        ))
    }
}

struct Instance {
    state: String,
    vars: Value,
    /// Pending timers: name to the time they are due and the event they fire.
    timers: BTreeMap<String, (i64, Value)>,
}

#[derive(Default)]
struct Stats {
    steps: usize,
    fired: usize,
    ignored: usize,
    delayed_emits: usize,
    timers_set: usize,
    fired_by: BTreeMap<String, usize>,
}

fn elapsed(spec: &Value, event: &str) -> bool {
    items(spec, "events")
        .iter()
        .any(|e| e["id"] == json!(event) && e["elapsed"] == json!(true))
}

fn delivery_of(spec: &Value, event: &str) -> Option<String> {
    items(spec, "events")
        .iter()
        .find(|e| e["id"] == json!(event))
        .and_then(|e| e["delivery"].as_str().map(str::to_string))
}

/// The key of a machine's entry in a configuration of the trace.
fn config_key(info: &Info, instance: &Value) -> String {
    if info.instanced {
        format!("{}#{}", info.id, instance)
    } else {
        info.id.clone()
    }
}

fn wire_event(spec: &Specification, event: &str, payload: &Value) -> Value {
    let payload = if payload.is_null() {
        json!({})
    } else {
        payload.clone()
    };
    let values = WireValues::new(&spec.typed, &spec.cases);
    json!({ "event": event, "payload": values.payload(event, &payload) })
}

fn compare_scenario(
    spec: &Specification,
    engines: &mut Engines,
    scenario: &Scenario,
    trace: &Value,
) -> Result<Stats, String> {
    let values = WireValues::new(&spec.typed, &spec.cases);
    let sid = &scenario.id;
    if trace["outcome"] != json!("quiescent") {
        return Err(format!(
            "{sid}: the trace ends {}, not quiescent",
            trace["outcome"]
        ));
    }
    let mut stats = Stats::default();
    let mut instances: BTreeMap<(String, String), Instance> = BTreeMap::new();

    // The machines that are not instanced exist from the start: their initial state and variables
    // are Exegra's initial configuration.
    for id in &scenario.machines {
        let info = &spec.infos[id];
        if info.instanced {
            continue;
        }
        let (state, vars) = engines.initial(id, &info.placeholder_key)?;
        let initial = &trace["initial"]["machines"][id];
        if initial["state"].as_str() != Some(&state) {
            return Err(format!(
                "{sid}: initially {id} is in {state}, Exegra has {}",
                initial["state"]
            ));
        }
        same(
            &format!("{sid}: initially {id}"),
            "variables",
            &vars,
            &values.vars(id, &initial["vars"]),
        )?;
        instances.insert(
            (id.clone(), "null".to_string()),
            Instance {
                state,
                vars,
                timers: BTreeMap::new(),
            },
        );
    }

    let steps = trace["steps"]
        .as_array()
        .ok_or_else(|| format!("{sid}: the trace has no steps"))?;
    for (position, step) in steps.iter().enumerate() {
        let index = step["index"]
            .as_u64()
            .ok_or_else(|| format!("{sid}: step {position} has no index"))?;
        let time = step["time"]
            .as_i64()
            .ok_or_else(|| format!("{sid}: step {index} has no time"))?;
        let event_id = step["event"]["id"]
            .as_str()
            .ok_or_else(|| format!("{sid}: step {index} has no event id"))?;
        let kind = step["kind"]
            .as_str()
            .ok_or_else(|| format!("{sid}: step {index} has no kind"))?;
        let context = format!("{sid}: step {index} (t={time}, {event_id})");
        stats.steps += 1;
        let fired_list = step["fired"].as_array().map_or(&[][..], Vec::as_slice);
        let ignored_list = step["ignored"].as_array().map_or(&[][..], Vec::as_slice);
        if kind != "deliver" {
            if !fired_list.is_empty() || !ignored_list.is_empty() {
                return Err(format!("{context}: a {kind} step delivers to a machine"));
            }
            continue;
        }
        let event = wire_event(spec, event_id, &step["event"]["payload"]);

        // The timer whose firing this event is.
        let timer = (step["event"]["source"]["kind"] == json!("timer")).then(|| {
            (
                step["event"]["source"]["behavior"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                step["event"]["source"]["instance"].clone(),
                step["event"]["source"]["timer"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        });

        let routed = fired_list
            .iter()
            .map(|f| (f, true))
            .chain(ignored_list.iter().map(|i| (i, false)));
        for (entry, fired) in routed {
            let machine = entry["behavior"]
                .as_str()
                .ok_or_else(|| format!("{context}: a routed entry has no behavior"))?;
            let info = spec
                .infos
                .get(machine)
                .filter(|_| scenario.machines.iter().any(|m| m == machine))
                .ok_or_else(|| format!("{context}: Exegra routed the event to {machine}, which the scenario does not run"))?;
            let instance_key = if info.instanced {
                if entry.get("instance").is_none() {
                    return Err(format!(
                        "{context}: the entry for {machine} names no instance"
                    ));
                }
                entry["instance"].clone()
            } else {
                Value::Null
            };
            let slot = (machine.to_string(), instance_key.to_string());
            if !instances.contains_key(&slot) {
                let (state, vars) = engines.initial(machine, &info.placeholder_key)?;
                instances.insert(
                    slot.clone(),
                    Instance {
                        state,
                        vars,
                        timers: BTreeMap::new(),
                    },
                );
            }
            let context = format!(
                "{context}, {machine}{}",
                if info.instanced {
                    format!(" {instance_key}")
                } else {
                    String::new()
                }
            );

            // A timer that fires is no longer pending; it must be due now, with this event.
            if let Some((owner, owner_instance, name)) = &timer {
                let owner_key = if info.instanced {
                    owner_instance.clone()
                } else {
                    Value::Null
                };
                if owner == machine && owner_key == instance_key {
                    let instance = instances.get_mut(&slot).unwrap();
                    let (due, scheduled) = instance
                        .timers
                        .remove(name)
                        .ok_or_else(|| format!("{context}: timer {name} fired, but it is not pending in the generated machine"))?;
                    if due != time {
                        return Err(format!(
                            "{context}: timer {name} is due at {due}, Exegra fired it at {time}"
                        ));
                    }
                    same(
                        &context,
                        &format!("the event of timer {name}"),
                        &scheduled,
                        &event,
                    )?;
                }
            }

            let instance = instances.get_mut(&slot).unwrap();
            let result = engines.step(machine, &instance.state, &instance.vars, &event, time)?;

            if !info.received.contains(event_id) {
                // Not an event of the generated machine: Exegra only ignored it, and the machine
                // must refuse it.
                if fired {
                    return Err(format!(
                        "{context}: Exegra fired {} on an event the machine has no transition on",
                        entry["transition"]
                    ));
                }
                if let StepResult::Stepped(_) = result {
                    return Err(format!("{context}: the generated machine accepted {event_id}, which it does not receive"));
                }
                stats.ignored += 1;
                continue;
            }
            let out = match result {
                StepResult::Refused(why) => {
                    return Err(format!(
                        "{context}: the generated machine refused an event Exegra delivered: {why}"
                    ))
                }
                StepResult::Stepped(out) => out,
            };
            let config = &step["config"]["machines"][config_key(info, &instance_key)];
            if config.is_null() {
                return Err(format!(
                    "{context}: the configuration has no machine {}",
                    config_key(info, &instance_key)
                ));
            }
            let expected_vars = values.vars(machine, &config["vars"]);

            if !fired {
                if !out.ignored {
                    return Err(format!("{context}: Exegra ignored the event, but the generated machine changed or asked for something: {out:?}"));
                }
                same(
                    &context,
                    "variables after an ignored event",
                    &out.vars,
                    &expected_vars,
                )?;
                if config["state"].as_str() != Some(&out.state) {
                    return Err(format!(
                        "{context}: ignored, state {} against Exegra's {}",
                        out.state, config["state"]
                    ));
                }
                stats.ignored += 1;
                continue;
            }
            if out.ignored {
                // A transition that changes nothing and asks for nothing is not an ignored event
                // for JavaScript, which says so itself.
                if engines.is_js(machine) {
                    return Err(format!(
                        "{context}: the generated machine ignored the event, but Exegra fired {}",
                        entry["transition"]
                    ));
                }
            }
            stats.fired += 1;
            *stats.fired_by.entry(machine.to_string()).or_default() += 1;

            let transition_id = entry["transition"]
                .as_str()
                .ok_or_else(|| format!("{context}: a firing names no transition"))?;
            let transition = info.transitions.get(transition_id).ok_or_else(|| {
                format!("{context}: Exegra fired {transition_id}, which {machine} does not declare")
            })?;
            if entry["from"].as_str() != Some(&instance.state) {
                return Err(format!("{context}: Exegra fired {transition_id} from {}, the generated machine was in {}", entry["from"], instance.state));
            }
            if entry["to"].as_str() != Some(&out.state) {
                return Err(format!(
                    "{context}: target state {} against Exegra's {}",
                    out.state, entry["to"]
                ));
            }
            if config["state"].as_str() != Some(&out.state) {
                return Err(format!(
                    "{context}: configuration state {} against {}",
                    config["state"], out.state
                ));
            }
            same(&context, "variables", &out.vars, &expected_vars)?;

            // Emits and timers: what Exegra emitted splits into events for receivers and the
            // elapsed-time events of timers.
            let mut exegra_emits = Vec::new();
            let mut exegra_timers = Vec::new();
            for e in entry["emitted"].as_array().map_or(&[][..], Vec::as_slice) {
                let id = e["event"]
                    .as_str()
                    .ok_or_else(|| format!("{context}: an emitted event has no id"))?;
                let due = e["time"]
                    .as_i64()
                    .ok_or_else(|| format!("{context}: the emitted {id} has no time"))?;
                let value = wire_event(spec, id, &e["payload"]);
                if elapsed(&spec.json, id) {
                    exegra_timers.push((value, due - time));
                } else {
                    exegra_emits.push((value, due - time));
                }
            }
            let got_emits: Vec<Value> = out
                .emits
                .iter()
                .map(|e| json!({ "event": e.event, "afterMs": e.after_ms }))
                .collect();
            let want_emits: Vec<Value> = exegra_emits
                .iter()
                .map(|(e, after)| json!({ "event": e, "afterMs": after }))
                .collect();
            same(
                &context,
                "emitted events and delays",
                &Value::Array(got_emits),
                &Value::Array(want_emits),
            )?;
            stats.delayed_emits += out.emits.iter().filter(|e| e.after_ms > 0).count();
            let effects: Vec<&Value> = items(transition, "effects")
                .iter()
                .filter(|e| e.get("emit").is_some() && e.get("timer").is_none())
                .collect();
            if effects.len() != out.emits.len() {
                return Err(format!("{context}: {transition_id} has {} emit effects, the generated machine emitted {}", effects.len(), out.emits.len()));
            }
            for (emit, effect) in out.emits.iter().zip(effects) {
                let event = emit.event["event"].as_str().unwrap_or_default();
                if let Some(to) = effect["to"].as_str() {
                    if emit.receiver != to {
                        return Err(format!(
                            "{context}: {event} goes to {}, the specification says {to}",
                            emit.receiver
                        ));
                    }
                }
                if Some(emit.delivery.clone()) != delivery_of(&spec.json, event) {
                    return Err(format!(
                        "{context}: delivery of {event} is {}, the specification says {:?}",
                        emit.delivery,
                        delivery_of(&spec.json, event)
                    ));
                }
            }

            let mut set = Vec::new();
            for command in &out.timers {
                match command {
                    TimerOut::Set {
                        name,
                        after_ms,
                        event,
                    } => {
                        set.push(json!({ "event": event, "afterMs": after_ms }));
                        instance
                            .timers
                            .insert(name.clone(), (time + after_ms, event.clone()));
                        stats.timers_set += 1;
                    }
                    TimerOut::Cancel { name } => {
                        instance.timers.remove(name);
                    }
                }
            }
            let want_timers: Vec<Value> = exegra_timers
                .iter()
                .map(|(e, after)| json!({ "event": e, "afterMs": after }))
                .collect();
            same(
                &context,
                "timers set",
                &Value::Array(set),
                &Value::Array(want_timers),
            )?;

            // The timers pending (Exegra names them `<behavior>/<name>`).
            if !info.instanced {
                if let Some(timers) = step["config"]["timers"].as_object() {
                    let prefix = format!("{machine}/");
                    let want: BTreeSet<&str> = timers
                        .keys()
                        .filter_map(|k| k.strip_prefix(&prefix))
                        .collect();
                    let got: BTreeSet<&str> = instance.timers.keys().map(String::as_str).collect();
                    if want != got {
                        return Err(format!(
                            "{context}: timers pending {got:?}, Exegra has {want:?}"
                        ));
                    }
                }
            }
            instance.state = out.state;
            instance.vars = out.vars;
        }
    }

    // The end: every instance is where Exegra's final configuration has it.
    for ((machine, instance_key), instance) in &instances {
        let info = &spec.infos[machine];
        let key: Value = serde_json::from_str(instance_key).unwrap();
        let config = &trace["final"]["machines"][config_key(info, &key)];
        if config["state"].as_str() != Some(&instance.state) {
            return Err(format!(
                "{sid}: finally {machine} {instance_key} is in {}, Exegra has {}",
                instance.state, config["state"]
            ));
        }
        same(
            &format!("{sid}: finally {machine} {instance_key}"),
            "variables",
            &instance.vars,
            &values.vars(machine, &config["vars"]),
        )?;
    }
    Ok(stats)
}

#[test]
fn the_generated_machines_step_as_exegra_runs_the_specification() {
    let what = "the comparison of the implementation machines of the specification with Exegra";
    let Some(cli) = support::exegra_cli(what) else {
        return;
    };
    if !support::node_required(what) {
        return;
    }
    let root = clean(&support::repo_root());
    let spec = read_specification(&root);
    let mut engines = Engines::new(&root);

    // Every implementation machine has a generated machine to compare.
    let missing: Vec<&String> = spec.infos.keys().filter(|id| !engines.has(id)).collect();
    assert!(
        missing.is_empty(),
        "no generated machine (crates/model machines, web/src/generated) for {missing:?}; regenerate"
    );

    let drivable: Vec<&Scenario> = spec
        .scenarios
        .iter()
        .filter(|s| s.undrivable.is_none())
        .collect();
    assert!(
        !drivable.is_empty(),
        "the specification has no scenario that runs implementation machines"
    );

    // Exegra's traces, a few at a time.
    let specs = specs_for(&root);
    let jobs = std::env::var("STRATA_DIFFERENTIAL_JOBS")
        .ok()
        .and_then(|j| j.parse::<usize>().ok())
        .filter(|j| *j > 0)
        .unwrap_or(2);
    let next = AtomicUsize::new(0);
    let traces: Mutex<BTreeMap<String, Result<Value, String>>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(scenario) = drivable.get(i) else {
                    break;
                };
                let trace = exegra_trace(&cli, &specs, &scenario.id);
                traces.lock().unwrap().insert(scenario.id.clone(), trace);
            });
        }
    });
    let traces = traces.into_inner().unwrap();

    let mut failures = Vec::new();
    let mut total = Stats::default();
    let mut fired_by: BTreeMap<String, usize> = BTreeMap::new();
    let mut driven = 0;
    for scenario in &drivable {
        let trace = match &traces[&scenario.id] {
            Ok(trace) => trace,
            Err(e) => {
                failures.push(format!("{}: no trace: {e}", scenario.id));
                continue;
            }
        };
        match compare_scenario(&spec, &mut engines, scenario, trace) {
            Ok(stats) => {
                driven += 1;
                total.steps += stats.steps;
                total.fired += stats.fired;
                total.ignored += stats.ignored;
                total.delayed_emits += stats.delayed_emits;
                total.timers_set += stats.timers_set;
                for (m, n) in stats.fired_by {
                    *fired_by.entry(m).or_default() += n;
                }
            }
            Err(e) => failures.push(e),
        }
    }
    for scenario in spec.scenarios.iter().filter(|s| s.undrivable.is_some()) {
        eprintln!(
            "not driven: scenario {}: {}",
            scenario.id,
            scenario.undrivable.as_deref().unwrap()
        );
    }
    eprintln!(
        "compared {driven} of {} scenarios: {} steps, {} firings, {} ignored events, {} delayed emits, {} timers set",
        spec.scenarios.len(),
        total.steps,
        total.fired,
        total.ignored,
        total.delayed_emits,
        total.timers_set
    );
    assert!(
        failures.is_empty(),
        "{} of {} scenarios disagree with Exegra:\n\n{}",
        failures.len(),
        drivable.len(),
        failures.join("\n\n")
    );

    // Every machine a scenario runs took part: it fired in at least one of them.
    let in_scenarios: BTreeSet<&String> = drivable.iter().flat_map(|s| s.machines.iter()).collect();
    let silent: Vec<&&String> = in_scenarios
        .iter()
        .filter(|id| !fired_by.contains_key(**id))
        .collect();
    assert!(
        silent.is_empty(),
        "these implementation machines fire in no scenario, so nothing compares them with Exegra: {silent:?}"
    );
    let unscripted: Vec<&String> = spec
        .infos
        .keys()
        .filter(|id| !in_scenarios.contains(id))
        .collect();
    if !unscripted.is_empty() {
        eprintln!("implementation machines no scenario runs, so not compared: {unscripted:?}");
    }
    assert!(
        total.delayed_emits > 0,
        "no scenario exercises a delayed emit"
    );
    assert!(total.timers_set > 0, "no scenario exercises a timer");
}
