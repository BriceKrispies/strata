//! Searching a tenant for a caller: the store answers candidates from its search index, the
//! kernel decides each one for the caller (the Read policies, then the fields the caller may read)
//! and the hits are reported as plain JSON. No rule about what a reader may find is written here
//! (SRH-150, SRH-1590, D-6586): a record the caller may not read, and a word that only a field
//! the caller may not read holds, produce no hit.
//!
//! The caller names no audience. The kernel reads the policies of the tenant for the principal of
//! the session, and for the anonymous principal when there is none (CPY-005).
//!
//! Implements: CT-175

use crate::app::{ApiError, App};
use crate::auth::Identity;
use crate::ops::header;
use serde_json::{json, Map, Value as Json};
use strata_kernel::search::{SearchCursor, SearchRequest};
use strata_kernel::{ObjectTypeId, RecordExt};
use strata_protocol::api::{ErrorCode, SearchRunRequest};
use strata_substrate::plain::to_plain;

/// Search the tenant as the caller, answering one page of hits and the cursor of the next.
pub fn run(app: &App, identity: &Identity, request: &SearchRunRequest) -> Result<Json, ApiError> {
    let tenant = header(app, identity)?;
    let mut objects = Vec::new();
    for name in &request.objects {
        objects.push(
            ObjectTypeId::new(name.as_str()).map_err(|_| {
                ApiError::new(ErrorCode::NotFound, format!("no object type {name}"))
            })?,
        );
    }
    let cursor = match &request.cursor {
        Some(text) => Some(SearchCursor::decode(text).ok_or_else(|| {
            ApiError::new(
                ErrorCode::Invalid,
                "The cursor is not one this search issued.",
            )
        })?),
        None => None,
    };
    let ask = SearchRequest {
        query: request.query.clone(),
        objects,
        page_size: request.limit,
        cursor,
    };
    let ctx = identity.context(app);
    let result = strata_storage_api::search::run(app.store.as_ref(), &ask, &ctx)
        .map_err(|error| ApiError::execute(error, identity.anonymous()))?;
    // A restricted person's personal fields are not shown, and a record found only by one of them
    // is not a hit (PRV-352, D-7741): the same restriction a view applies.
    let restrictions = crate::subject_rights::Rights::load_restrictions(app, &identity.tenant)?;
    let hits: Vec<Json> = result
        .hits
        .iter()
        .filter_map(|hit| restrictions.hide_hit(&tenant.schema, &hit.record, &hit.matched))
        .map(|(record, matched)| {
            let record = &record;
            let fields: Map<String, Json> = record
                .fields
                .iter()
                .map(|(field, value)| (field.as_str().to_string(), to_plain(value)))
                .collect();
            // The status of a hit is the publication state of its record, when its type has one.
            let status = tenant
                .schema
                .objects
                .get(&record.object)
                .and_then(|def| def.publication.as_ref())
                .map(|publication| to_plain(record.get(&publication.state_field)));
            json!({
                "object": record.object.as_str(),
                "id": record.id.0,
                "status": status,
                "matched": matched.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                "fields": fields,
            })
        })
        .collect();
    Ok(json!({
        "hits": hits,
        "next": result.next.map(|cursor| cursor.encode()),
        "examined": result.examined,
        "revision": result.revision.0,
        "schema_version": result.schema_version.0,
    }))
}
