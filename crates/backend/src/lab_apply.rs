//! Applying the change an Impact Lab projection showed (contract apply-lab-change, AGT-2940).
//!
//! The Lab projects a schema change at a revision of the site and writes nothing ([`crate::lab`]).
//! This is the one place that applies one. The change is what the projection was shown: the text,
//! the way it handles existing records, and the revision it was taken at, found in the run's
//! result (`run`) or named by the caller (`change`, `existing`, `revision`). It is committed only
//! while the site is still at that revision: the kernel's `expected_version` of a schema change
//! compares the schema version, which a record edit leaves alone, so the revision is held to the
//! commit itself ([`crate::ops::run_intent_at`]), and a site that has moved on is a conflict that
//! names the revision it is at, with nothing changed and no new decision on the newer state.
//!
//! A second apply of the same projection finds the site moved on, by the first, and is refused
//! the same way: a change is committed once. Only an owner of the site applies, with a fresh
//! authentication (step-up), and every call that commits is an audit entry of the site.
//!
//! Implements: AGT-2940

use crate::app::{ApiError, App};
use crate::auth::{Caller, Identity};
use crate::ops::{header, run_intent_at, stale_revision};
use serde_json::{json, Value as Json};
use strata_kernel::{ExistingData, Intent, Output, SchemaVersion, StateRevision};
use strata_protocol::api::ErrorCode;
use strata_substrate::proposal;

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// What is applied: the change as the proposal syntax, how existing records are handled, the
/// revision it was projected at, and the schema version of that revision when the projection
/// said so.
struct Target {
    change: String,
    existing: ExistingData,
    revision: StateRevision,
    schema_version: Option<SchemaVersion>,
    run: Option<String>,
}

fn existing_of(name: &str) -> Result<ExistingData, ApiError> {
    match name {
        "reject" => Ok(ExistingData::Reject),
        "grandfather" => Ok(ExistingData::Grandfather),
        other => Err(invalid(format!(
            "unknown handling of existing records {other:?}"
        ))),
    }
}

fn text<'a>(body: &'a Json, name: &str) -> Result<&'a str, ApiError> {
    body.get(name)
        .and_then(Json::as_str)
        .ok_or_else(|| invalid(format!("The request needs {name}.")))
}

/// The target of a request: the finished run it names, or the change, the way existing records are
/// handled and the revision it names itself.
fn target_of(app: &App, identity: &Identity, body: &Json) -> Result<Target, ApiError> {
    if let Some(run) = body.get("run").and_then(Json::as_str) {
        let held = app.lab.result(run, identity).ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotFound,
                "No such run. Results are held in this backend process only: a restart loses them.",
            )
        })?;
        if held["status"] != "finished" {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "The projection has not finished, so there is nothing to apply yet.",
            ));
        }
        let result = &held["result"];
        let revision = result["snapshot"]["revision"]
            .as_u64()
            .ok_or_else(|| invalid("The run's result names no revision."))?;
        return Ok(Target {
            change: text(&result["request"], "change")?.to_string(),
            existing: existing_of(text(&result["request"], "existing")?)?,
            revision: StateRevision(revision),
            schema_version: result["snapshot"]["schema_version"]
                .as_u64()
                .map(SchemaVersion),
            run: Some(run.to_string()),
        });
    }
    Ok(Target {
        change: text(body, "change")?.to_string(),
        existing: existing_of(text(body, "existing")?)?,
        revision: StateRevision(
            body.get("revision")
                .and_then(Json::as_u64)
                .ok_or_else(|| invalid("The request needs revision, a whole number."))?,
        ),
        schema_version: body
            .get("schema_version")
            .and_then(Json::as_u64)
            .map(SchemaVersion),
        run: None,
    })
}

/// Apply the change of a projection: `{ "run": <id of a finished projection> }`, or `{ "change":
/// <proposal text>, "existing": "reject" | "grandfather", "revision": <revision it was projected
/// at>, "schema_version": <optional> }`. The caller has been held to an owner with a fresh
/// authentication. The answer is `{ "version": <new schema version>, "revision": <new revision> }`.
pub fn apply(
    app: &App,
    _caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    let target = target_of(app, identity, body)?;
    let tenant = header(app, identity)?;
    // Before anything is decided: a site that is not where the projection was taken is refused,
    // naming where it is. The commit holds the same line again, for a change that lands between.
    if tenant.revision != target.revision {
        return Err(stale_revision(target.revision, Some(tenant.revision)));
    }
    let changes = proposal::parse(&tenant.schema, &target.change)
        .map_err(|problem| ApiError::new(ErrorCode::Invalid, problem))?;
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: Some(target.schema_version.unwrap_or(tenant.schema.version)),
        existing: target.existing,
    };
    let person = crate::service::person(identity)?;
    let ctx = identity.context(app);
    let done = run_intent_at(app, &ctx, &intent, false, Some(target.revision))?;
    let version = match done.transition.output {
        Output::Schema { version } => version.0,
        other => {
            return Err(ApiError::new(
                ErrorCode::Internal,
                format!("unexpected output {other:?}"),
            ))
        }
    };
    app.audit(
        "lab.change_applied",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "principal": person.as_str(),
            "run": target.run,
            "projected_at": target.revision.0,
            "revision": done.receipt.revision.0,
            "schema_version": version,
        }),
    );
    Ok(json!({ "version": version, "revision": done.receipt.revision.0 }))
}
