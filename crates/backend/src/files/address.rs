//! Signed addresses of files (contracts sign-media-address and sign-file-address, EDG-012,
//! EDG-1592, D-6588).
//!
//! An address is the path `/files/<file>/<expires>/<scope>/<signature>`:
//!
//! - `file`: the file id;
//! - `expires`: when the address stops working, in milliseconds since the epoch, at most
//!   [`super::ADDRESS_MAX_SECONDS`] (or the tenant's lower limit) after it was signed;
//! - `scope`: `public` when anyone may read the file (its record is public), else `principal`;
//! - `signature`: HMAC-SHA-256 of the tenant, the file, the expiry, the scope and, for
//!   `principal`, the principal it was signed for, under a key of the tenant.
//!
//! The principal is not written in the address: delivery computes the signature again with the
//! principal of the session, so an address signed for someone else, for another file, another
//! expiry or another tenant does not verify. The key of a tenant is derived from the platform
//! seal key, which is kept out of the store, and the tenant id, so a copy of a tenant's database
//! signs nothing and an address of one tenant is worthless at another.

use super::{access, not_found, storage, tenant_limits, FileState};
use crate::app::{ApiError, App, LOG};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use strata_kernel::{Context, ObjectTypeId, Principal, PrincipalId, RecordId, TenantId, Timestamp};
use strata_protocol::api::ErrorCode;
use strata_runtime::routes::same_secret;

/// Who an address is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Anyone: the file is readable by an anonymous reader.
    Public,
    /// Only the principal it was signed for, signed in.
    Principal,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Public => "public",
            Scope::Principal => "principal",
        }
    }

    fn parse(text: &str) -> Option<Scope> {
        match text {
            "public" => Some(Scope::Public),
            "principal" => Some(Scope::Principal),
            _ => None,
        }
    }
}

/// An address as delivery receives it, every part checked for its form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub file: String,
    pub expires_at: i64,
    pub scope: Scope,
    pub signature: String,
}

impl Address {
    /// The parts of an address, when each has its form: a file id, a number of at most 15
    /// digits, a scope and 64 hexadecimal digits. Anything else (a path, an encoded slash, a dot)
    /// is not an address.
    pub fn parse(file: &str, expires: &str, scope: &str, signature: &str) -> Option<Address> {
        if !storage::valid_file_id(file)
            || expires.is_empty()
            || expires.len() > 15
            || !expires.bytes().all(|b| b.is_ascii_digit())
            || !storage::valid_file_id(signature)
        {
            return None;
        }
        Some(Address {
            file: file.to_string(),
            expires_at: expires.parse().ok()?,
            scope: Scope::parse(scope)?,
            signature: signature.to_string(),
        })
    }

    /// The path of the address.
    pub fn path(&self) -> String {
        format!(
            "/files/{}/{}/{}/{}",
            self.file,
            self.expires_at,
            self.scope.as_str(),
            self.signature
        )
    }
}

/// HMAC-SHA-256 (RFC 2104) for a key of at most one block.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    let n = key.len().min(64);
    block[..n].copy_from_slice(&key[..n]);
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// The address key of a tenant (secret `file-address-key`).
fn tenant_key(app: &App, tenant: &TenantId) -> [u8; 32] {
    hmac(
        &app.cfg.factors.seal_key.0,
        format!("strata/file-address-key/v1\0{tenant}").as_bytes(),
    )
}

/// The signature of an address for `principal` (`None` for a public one).
fn signature(
    app: &App,
    tenant: &TenantId,
    file: &str,
    expires_at: i64,
    scope: Scope,
    principal: Option<&PrincipalId>,
) -> String {
    let message = format!(
        "strata/file-address/v1\0{tenant}\0{file}\0{expires_at}\0{}\0{}",
        scope.as_str(),
        principal.map_or("", |p| p.as_str())
    );
    storage::hex(&hmac(&tenant_key(app, tenant), message.as_bytes()))
}

/// Sign an address for a stored file of a tenant (contract sign-file-address): the in-process
/// signing, without the checks of who may read the file, which [`sign_media_address`] makes.
///
/// Implements: CT-144
pub fn sign_file_address(
    app: &App,
    tenant: &TenantId,
    file: &str,
    expires_at: Timestamp,
    principal: Option<&PrincipalId>,
) -> Result<Address, ApiError> {
    if storage::get_meta(app, tenant, file)?.is_none() {
        return Err(not_found());
    }
    let scope = if principal.is_some() {
        Scope::Principal
    } else {
        Scope::Public
    };
    Ok(Address {
        file: file.to_string(),
        expires_at: expires_at.0,
        scope,
        signature: signature(app, tenant, file, expires_at.0, scope, principal),
    })
}

/// Whether an address verifies for the principal of `ctx` at its time: the signature is the
/// tenant's for this file, expiry and scope, for the session's principal when the scope is
/// `principal`, and the expiry has not passed.
pub fn verifies(app: &App, ctx: &Context, address: &Address) -> bool {
    if address.expires_at <= ctx.now.0 {
        return false;
    }
    let principal = match (address.scope, &ctx.principal) {
        (Scope::Public, _) => None,
        (Scope::Principal, Principal::User(principal)) => Some(principal),
        (Scope::Principal, _) => return false,
    };
    let expected = signature(
        app,
        &ctx.tenant,
        &address.file,
        address.expires_at,
        address.scope,
        principal,
    );
    same_secret(&address.signature, &expected)
}

/// A request to sign an address (contract sign-media-address).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SignRequest {
    pub file: String,
    /// The principal the address is for; the caller when absent.
    #[serde(default)]
    pub principal: Option<String>,
    /// When the address stops working, in milliseconds; the longest allowed when absent.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// A record that refers to the file and that the caller may read: the readability of the
    /// file is decided from it alone (D-6587).
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub record: Option<u64>,
    /// A tenant the request names: it must be the one the request is made at (TEN-1140).
    #[serde(default)]
    pub tenant: Option<String>,
}

/// Sign an address of a file for a principal (contract sign-media-address, D-6588). The caller
/// and the principal named must both be able to read the file, and the file must be clean; the
/// expiry is at most an hour (or the tenant's lower limit) from now. A file that is public (an
/// anonymous reader may read it) gets a public address, which needs no session at delivery.
///
/// Implements: CT-106, EDG-012, EDG-1592
pub fn sign_media_address(
    app: &App,
    ctx: &Context,
    request: &SignRequest,
) -> Result<Json, ApiError> {
    let Principal::User(caller) = &ctx.principal else {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ));
    };
    if let Some(named) = request.tenant.as_deref() {
        if named != ctx.tenant.as_str() {
            app.audit(
                "files.tenant_mismatch",
                &ctx.correlation,
                json!({ "tenant": ctx.tenant.as_str() }),
            );
            let mut error = ApiError::new(
                ErrorCode::Forbidden,
                "The request names another tenant than the one it is made at.",
            );
            error.0.kind = Some("tenant_mismatch".to_string());
            return Err(error);
        }
    }
    let meta = storage::get_meta(app, &ctx.tenant, &request.file)?.ok_or_else(not_found)?;
    let through = match (&request.object, request.record) {
        (Some(object), Some(id)) => Some((
            ObjectTypeId::new(object.as_str()).map_err(|_| not_found())?,
            RecordId(id),
        )),
        (None, None) => None,
        _ => {
            return Err(super::invalid(
                "Name a record by its object type and its id, or name none.",
            ))
        }
    };
    let through_ref = through.as_ref().map(|(object, id)| (object, *id));
    if !access::may_read(app, ctx, &meta, through_ref)? {
        return Err(not_found());
    }
    match meta.state {
        FileState::Clean => {}
        FileState::Scanning => {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "The file has not been released yet: its scan has not finished.",
            ))
        }
        FileState::Rejected => {
            return Err(super::invalid("The file was rejected by the upload scan."))
        }
    }
    let principal = match request.principal.as_deref() {
        None => caller.clone(),
        Some(name) => PrincipalId::new(name).map_err(|_| not_found())?,
    };
    if principal != *caller {
        let theirs = access::as_principal(ctx, Principal::User(principal.clone()));
        if !access::may_read(app, &theirs, &meta, through_ref)? {
            return Err(not_found());
        }
    }
    let public = access::may_read(
        app,
        &access::as_principal(ctx, Principal::Anonymous),
        &meta,
        through_ref,
    )?;
    let limits = tenant_limits(app, &ctx.tenant);
    let longest = ctx.now.0 + i64::from(limits.address_max_seconds) * 1000;
    let expires_at = request.expires_at.unwrap_or(longest);
    if expires_at <= ctx.now.0 || expires_at > longest {
        return Err(super::invalid(format!(
            "An address expires after now and at most {} seconds from now.",
            limits.address_max_seconds
        )));
    }
    let address = sign_file_address(
        app,
        &ctx.tenant,
        &meta.file,
        Timestamp(expires_at),
        (!public).then_some(&principal),
    )?;
    LOG.info(
        "files.address_signed",
        &ctx.correlation,
        json!({
            "tenant": ctx.tenant.as_str(),
            "file": meta.file,
            "scope": address.scope.as_str(),
            "expires_at": expires_at,
        }),
    );
    Ok(json!({
        "address": address.path(),
        "expires_at": expires_at,
        "public": public,
    }))
}
