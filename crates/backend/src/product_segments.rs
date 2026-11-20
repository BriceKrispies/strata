//! The segments of a product bundle (WG-5166, D-6580 to D-6582): defined, replaced and removed
//! through the segment store around the one schema change that installs, upgrades or removes the
//! product.
//!
//! A segment is no schema change: it lives in the segment block beside the tenant state, so the
//! one owner-approved change of a product cannot carry it in the same commit. What the owner
//! approves is still one operation, and this is what holds of it:
//!
//! - **Checked first.** Before anything is touched, the change is decided against the whole of the
//!   tenant as a commit would ([`precheck`]) and every segment the bundle declares is decided
//!   against the schema that change leaves, as `define-segment` decides it: the capability of the
//!   caller, the predicate, the principal field, the bounds of a tenant (SEG-001 to SEG-007,
//!   SEG-029). A change that would be refused, or a segment that would be, touches nothing.
//! - **The schema change is one commit** with the ownership record of the product, which lists
//!   the segments by their keys (`owns`). There is no moment at which the record names an entity
//!   that no definition of the bundle declares.
//! - **Segments a change removes go first** ([`take`]): a schema change that removes or retypes a
//!   field a segment names is refused (SEG-006), so the segments of a removed product, or those a
//!   later version no longer declares, are removed before the schema change, with no exit event
//!   (SEG-026). When the schema change is then refused anyway (a race: the pre-check passed), they
//!   are put back ([`put_back`]) and their members are decided again by the batches of the new
//!   definition, which writes entry events again.
//! - **Segments a change defines come last** ([`define_all`]), because they name the object types
//!   the change creates. A definition that changes nothing is a no-op, so the same bundle again
//!   gives the same segment id at the same version (D-6582). If a definition is refused after the
//!   schema change was committed (a race the pre-check could not see), an install undoes itself:
//!   the segments it defined are removed and the product is removed with its record; an upgrade
//!   cannot be undone and says so, and the bundle installed again at the same version defines what
//!   is missing ([`ensure`]), as it does for a process that stopped between the two.
//!
//! So the atomicity that holds is: the schema, the ownership record and, for the owner's request,
//! the outcome are one or nothing as long as the pre-check agrees with the commits, and a failure
//! after the schema commit is repaired by an inverse operation or by repeating the install, never
//! left as a record that names a segment nobody can define.

use crate::app::{ApiError, App};
use crate::auth::Identity;
use strata_kernel::segments::{self as seg, Definition, Segment, SegmentId};
use strata_kernel::SchemaVersion;
use strata_protocol::api::ErrorCode;
use strata_storage_api::segments as kept;
use strata_substrate::bundle::{ProductBundle, ProductChange, SegmentChanges};

/// The segments a change took out before its schema change, to put back when that is refused.
#[derive(Debug, Default)]
pub struct Taken(Vec<Segment>);

impl Taken {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn bundle_problem(error: strata_substrate::bundle::BundleError) -> ApiError {
    let mut reply = ApiError::new(ErrorCode::Invalid, error.message);
    reply.0.kind = Some("invalid-bundle".to_string());
    reply
}

/// The definition a segment row stands for, to define it again.
fn definition_of(segment: &Segment) -> Definition {
    Definition {
        id: segment.id,
        name: segment.name.clone(),
        subject: segment.subject.clone(),
        predicate: segment.predicate.clone(),
        refresh_seconds: segment.refresh_seconds,
        principal_field: segment.principal_field.clone(),
        visible_to_members: segment.visible_to_members,
    }
}

/// Decide the change and its segments without committing anything (see the module).
pub fn precheck(
    app: &App,
    identity: &Identity,
    change: &ProductChange,
    expected_version: Option<SchemaVersion>,
) -> Result<(), ApiError> {
    if change.segments.is_empty() {
        return Ok(());
    }
    let defs = change
        .segments
        .definitions(&identity.tenant)
        .map_err(bundle_problem)?;
    let anonymous = identity.anonymous();
    let state = app.store.load_complete(&identity.tenant)?;
    let ctx = identity.context(app);
    let transition = strata_kernel::transition(&state, &change.intent(expected_version), &ctx)
        .map_err(|error| ApiError::domain(error, anonymous))?;
    if defs.is_empty() {
        return Ok(());
    }
    let after = strata_kernel::apply_delta(state.tenant.as_deref(), &transition.delta)
        .map_err(|error| ApiError::domain(error, anonymous))?;
    let mut catalog = app.store.segment_catalog(&identity.tenant)?;
    let removed = change.segments.removals(&identity.tenant);
    catalog.retain(|segment| !removed.contains(&segment.id));
    for def in defs {
        let defined = seg::define(&after, &catalog, def, &ctx)
            .map_err(|error| ApiError::domain(error, anonymous))?;
        catalog.retain(|segment| segment.id != defined.segment.id);
        catalog.push(defined.segment);
    }
    Ok(())
}

/// Remove the segments the change removes, before its schema change.
pub fn take(app: &App, identity: &Identity, change: &ProductChange) -> Result<Taken, ApiError> {
    let ids = change.segments.removals(&identity.tenant);
    if ids.is_empty() {
        return Ok(Taken::default());
    }
    let catalog = app.store.segment_catalog(&identity.tenant)?;
    let ctx = identity.context(app);
    let mut taken = Taken::default();
    for id in ids {
        let Some(found) = catalog.iter().find(|segment| segment.id == id) else {
            continue;
        };
        match kept::remove(app.store.as_ref(), &ctx, id) {
            Ok(()) => taken.0.push(found.clone()),
            Err(error) => {
                put_back(app, identity, &taken);
                return Err(ApiError::execute(error, identity.anonymous()));
            }
        }
    }
    Ok(taken)
}

/// Define again the segments [`take`] removed.
pub fn put_back(app: &App, identity: &Identity, taken: &Taken) {
    let ctx = identity.context(app);
    for segment in &taken.0 {
        let _ = kept::define(app.store.as_ref(), &ctx, &definition_of(segment));
    }
}

/// Define the segments of a set of changes, each once. On a refusal the segments this call made
/// are removed again and the error is returned: the segments that existed are left as they are.
fn define_changes(
    app: &App,
    identity: &Identity,
    changes: &SegmentChanges,
) -> Result<(), ApiError> {
    if changes.define.is_empty() {
        return Ok(());
    }
    let defs = changes
        .definitions(&identity.tenant)
        .map_err(bundle_problem)?;
    let ctx = identity.context(app);
    let before: Vec<SegmentId> = app
        .store
        .segment_catalog(&identity.tenant)?
        .into_iter()
        .map(|segment| segment.id)
        .collect();
    let mut made: Vec<SegmentId> = Vec::new();
    for def in defs {
        let id = def.id;
        match kept::define(app.store.as_ref(), &ctx, &def) {
            Ok(_) => {
                if !before.contains(&id) {
                    made.push(id);
                }
            }
            Err(error) => {
                for id in made {
                    let _ = kept::remove(app.store.as_ref(), &ctx, id);
                }
                return Err(ApiError::execute(error, identity.anonymous()));
            }
        }
    }
    Ok(())
}

/// Define the segments a committed change declares.
pub fn define_all(app: &App, identity: &Identity, change: &ProductChange) -> Result<(), ApiError> {
    define_changes(app, identity, &change.segments)
}

/// Define the segments a bundle declares, for an install of the version that is installed
/// already: nothing of the schema runs, and a segment that is missing is defined. Segments that
/// exist are untouched.
pub fn ensure(app: &App, identity: &Identity, bundle: &ProductBundle) -> Result<(), ApiError> {
    let changes = SegmentChanges {
        define: bundle
            .segments
            .iter()
            .map(|(key, decl)| (key.clone(), decl.clone()))
            .collect(),
        remove: Vec::new(),
    };
    define_changes(app, identity, &changes)
}
