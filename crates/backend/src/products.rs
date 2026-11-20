//! Install, upgrade and remove a product at run time, and list what is installed (the contracts
//! install-bundle, upgrade-bundle, remove-bundle and list-installed-products; D-6580 to D-6583).
//!
//! Each change is one `ChangeSchema` intent decided by the kernel: the entities of the bundle and
//! the ownership record of the product (`installed-product`, kept in the tenant schema) are edits
//! of that one intent, so they are written in one commit under one schema version. The planning
//! (what is created, changed and removed, from the ownership record) is `strata_substrate::product`;
//! this module adds who may ask, step-up, the idempotency key, the dry run and the replies.
//!
//! - The owner of the tenant installs, upgrades and removes, with a session that has authenticated
//!   recently (step-up, IDN-019). A dry run changes nothing and answers the projection that the
//!   change would make; a person who may manage the schema lists what is installed.
//! - The body names `bundle` (install and upgrade) or `product` and `delete_records` (remove),
//!   `dry_run`, `expected_version` and `idempotency_key`. The idempotency key may also come in the
//!   `Idempotency-Key` header, which the BFF forwards; a repeat with the same key answers the first
//!   result and changes nothing (D-6595).
//! - The reply is `{ "schema_version", "unchanged", "projection" }`; the projection is the
//!   `projection-summary` (`diverged`, `summary`) with the entities `created`, `changed` and
//!   `removed` and the number of `records_deleted`.
//!
//! Implements: CT-236, CT-835, CT-836, CT-837, INT-1581, INT-1582, INT-1583

use crate::app::{ApiError, App, Shared};
use crate::auth::{self, Identity};
use crate::ops;
use crate::service;
use crate::signin::stepup;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use strata_kernel::{Context, EventBody, Intent, Output, SchemaVersion};
use strata_protocol::api::{valid_idempotency_key, ErrorCode, HEADER_IDEMPOTENCY};
use strata_substrate::bundle::{
    plan_install, plan_remove, plan_upgrade, run_planned, BundleError, BundleErrorKind,
    InstalledRecord, OwnedEntity, Planned, ProductBundle, ProductChange, ProductInstall,
    ProductInstallError, ProductTarget,
};

/// The platform release this backend implements, which a bundle's platform range is checked
/// against (D-6583): the version of the specification it is built from.
pub const PLATFORM_RELEASE: &str = "0.2-draft";

/// The most products one page of the list holds, and how many it holds when no limit is given
/// (D-6595).
const MAX_PAGE: usize = 100;
const DEFAULT_PAGE: usize = 50;

/// The three changes of a product.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Install,
    Upgrade,
    Remove,
}

/// A bundle problem as the reply of the route says it: the contract errors `invalid-bundle` and
/// `invalid` are 422, `conflict` 409 and `not-found` 404.
fn bundle_error(error: BundleError) -> ApiError {
    let (code, kind) = match error.kind {
        BundleErrorKind::InvalidBundle => (ErrorCode::Invalid, "invalid-bundle"),
        BundleErrorKind::Invalid => (ErrorCode::Invalid, "invalid"),
        BundleErrorKind::Conflict => (ErrorCode::Conflict, "conflict"),
        BundleErrorKind::NotFound => (ErrorCode::NotFound, "not-found"),
    };
    let mut reply = ApiError::new(code, error.message);
    reply.0.kind = Some(kind.to_string());
    reply
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// The idempotency key of the `Idempotency-Key` header, when the request has one.
fn header_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    match headers.get(HEADER_IDEMPOTENCY) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .ok()
            .filter(|key| valid_idempotency_key(key))
            .map(|key| Some(key.to_string()))
            .ok_or_else(|| invalid("An idempotency key is 1 to 255 visible ASCII characters.")),
    }
}

/// The bundle a request carries, read as the tenant bundle of a product.
fn bundle_of(body: &Json_) -> Result<ProductBundle, ApiError> {
    let bundle = body
        .get("bundle")
        .filter(|bundle| bundle.is_object())
        .ok_or_else(|| invalid("The request must carry the bundle to install as \"bundle\"."))?;
    ProductBundle::from_json(&bundle.to_string()).map_err(bundle_error)
}

/// The entities of a plan as the reply lists them.
fn entities(list: &[OwnedEntity]) -> Vec<Json_> {
    list.iter()
        .map(|entity| json!({ "kind": entity.kind, "id": entity.id }))
        .collect()
}

/// The projection of a change (`projection-summary`, INT-154): what the kernel says it does, line
/// by line, and the entities it creates, changes and removes.
fn projection(change: &ProductChange, summary: Vec<String>, records_deleted: usize) -> Json_ {
    json!({
        "diverged": false,
        "summary": summary,
        "created": entities(&change.created),
        "changed": entities(&change.changed),
        "removed": entities(&change.removed),
        "records_deleted": records_deleted,
    })
}

/// The lines the kernel describes a committed or simulated change with, and how many records it
/// deletes.
fn lines_of(events: &[strata_kernel::DomainEvent]) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut deleted = 0;
    for event in events {
        match &event.body {
            EventBody::SchemaChanged { summary, .. } => lines.extend(summary.iter().cloned()),
            EventBody::RecordDeleted { .. } => deleted += 1,
            _ => {}
        }
    }
    (lines, deleted)
}

/// Decides the change against the whole of the tenant, as a commit would, and commits nothing.
fn simulate(
    app: &App,
    identity: &Identity,
    change: &ProductChange,
    expected_version: Option<SchemaVersion>,
) -> Result<(Vec<String>, usize), ApiError> {
    let state = app.store.load_complete(&identity.tenant)?;
    let ctx = identity.context(app);
    let transition = strata_kernel::transition(&state, &change.intent(expected_version), &ctx)
        .map_err(|error| ApiError::domain(error, identity.anonymous()))?;
    Ok(lines_of(&transition.events))
}

/// One change of a product, planned from the tenant as it is now and then simulated (a dry run) or
/// committed as one intent.
fn change(
    app: &App,
    identity: &Identity,
    operation: Operation,
    body: &Json_,
    key_of_header: Option<String>,
) -> Result<Json_, ApiError> {
    let dry_run = body
        .get("dry_run")
        .and_then(Json_::as_bool)
        .unwrap_or(false);
    let key = key_of_header.or_else(|| {
        body.get("idempotency_key")
            .and_then(Json_::as_str)
            .map(str::to_string)
    });
    if key
        .as_deref()
        .is_some_and(|key| !valid_idempotency_key(key))
    {
        return Err(invalid(
            "An idempotency key is 1 to 255 visible ASCII characters.",
        ));
    }
    let expected_version = body
        .get("expected_version")
        .and_then(Json_::as_u64)
        .map(SchemaVersion);
    let tenant = ops::header(app, identity)?;
    let target = ProductTarget {
        tenant: &identity.tenant,
        principal: identity.principal.clone(),
        now: app.now().0,
        release: PLATFORM_RELEASE,
        schema: &tenant.schema,
        installed: None,
        expected_version,
        idempotency_key: key,
    };
    let planned = match operation {
        Operation::Install => plan_install(&bundle_of(body)?, &target),
        Operation::Upgrade => plan_upgrade(&bundle_of(body)?, &target),
        Operation::Remove => {
            let product = service::text(body, "product", 64)?;
            let delete_records = body
                .get("delete_records")
                .and_then(Json_::as_bool)
                .ok_or_else(|| {
                    invalid("delete_records must say whether the records are deleted.")
                })?;
            plan_remove(product, delete_records, &target)
        }
    }
    .map_err(bundle_error)?;

    if dry_run {
        return Ok(match &planned {
            Planned::Unchanged { schema_version } => json!({
                "schema_version": schema_version,
                "unchanged": true,
                "dry_run": true,
            }),
            Planned::Change(change) => {
                let (lines, deleted) = simulate(app, identity, change, expected_version)?;
                json!({
                    "schema_version": tenant.schema.version.0,
                    "unchanged": false,
                    "dry_run": true,
                    "projection": projection(change, lines, deleted),
                })
            }
        });
    }

    // The summary and the deletions of the committed change, from the events of its commit.
    let mut committed: (Vec<String>, usize) = (Vec::new(), 0);
    let mut run = |sub: Context, intent: Intent| -> Result<Output, ApiError> {
        let mut ctx = identity.context(app);
        ctx.idempotency_key = sub.idempotency_key;
        let done = ops::run_intent(app, &ctx, &intent, identity.anonymous())?;
        committed = lines_of(&done.transition.events);
        Ok(done.transition.output.clone())
    };
    let result = run_planned(planned.clone(), &target, &mut run).map_err(|error| match error {
        ProductInstallError::Refused(error) => bundle_error(error),
        ProductInstallError::Step { error, .. } => error,
    })?;
    Ok(match (result, planned) {
        (ProductInstall::Unchanged { schema_version }, _) => json!({
            "schema_version": schema_version,
            "unchanged": true,
        }),
        (ProductInstall::Applied { schema_version, .. }, Planned::Change(change)) => json!({
            "schema_version": schema_version,
            "unchanged": false,
            "projection": projection(&change, committed.0, committed.1),
        }),
        (ProductInstall::Applied { schema_version, .. }, Planned::Unchanged { .. }) => json!({
            "schema_version": schema_version,
            "unchanged": true,
        }),
    })
}

/// A change of a product: the owner, with a session that has authenticated recently. A dry run
/// changes nothing and asks no fresh factor.
async fn write(
    app: Shared,
    headers: HeaderMap,
    body: Json_,
    operation: Operation,
) -> Result<Json<Json_>, ApiError> {
    let key = header_key(&headers)?;
    let dry_run = body
        .get("dry_run")
        .and_then(Json_::as_bool)
        .unwrap_or(false);
    crate::http::as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        if !dry_run {
            stepup::require_fresh(app, caller, identity)?;
        }
        change(app, identity, operation, &body, key)
    })
    .await
}

/// install-bundle: `POST /internal/products/install`.
///
/// Implements: CT-236
pub async fn install(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    write(app, headers, body, Operation::Install).await
}

/// upgrade-bundle: `POST /internal/products/upgrade`.
///
/// Implements: CT-835
pub async fn upgrade(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    write(app, headers, body, Operation::Upgrade).await
}

/// remove-bundle: `POST /internal/products/remove`.
///
/// Implements: CT-836
pub async fn remove(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    write(app, headers, body, Operation::Remove).await
}

/// The installed products of the tenant, in product-id order, `limit` of them after `cursor` (the
/// last product id of the page before).
fn installed(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to see the installed products.",
        ));
    }
    let tenant = ops::header(app, identity)?;
    if !auth::session_view(&tenant, identity).may_manage_schema {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only someone who can change the schema may see the installed products.",
        ));
    }
    let limit = match body.get("limit") {
        None | Some(Json_::Null) => DEFAULT_PAGE,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=MAX_PAGE as u64).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| invalid("limit is from 1 to 100."))?,
    };
    let after = body.get("cursor").and_then(Json_::as_str);
    let all: Vec<InstalledRecord> = InstalledRecord::all(&tenant.schema)
        .into_iter()
        .filter(|record| after.is_none_or(|after| record.product.as_str() > after))
        .collect();
    let next = (all.len() > limit).then(|| all[limit - 1].product.clone());
    let page: Vec<&InstalledRecord> = all.iter().take(limit).collect();
    Ok(json!({ "products": page, "next": next }))
}

/// list-installed-products: `POST /internal/products`.
///
/// Implements: CT-837
pub async fn list(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, move |app, identity| {
        installed(app, identity, &body)
    })
    .await
}
