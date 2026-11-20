//! Delivery of a file at a signed address (contract deliver-file, D-6588, EDG-1592).
//!
//! Every check is made at delivery, so a right that was revoked stops a live address: the
//! address verifies for the session's principal (or is public), it has not expired, the file
//! exists for this tenant and is clean, and the principal (or an anonymous reader, for a public
//! address) may still read it. A failed check answers not-found, whichever it was.
//!
//! The response carries the media type the content confirmed, `X-Content-Type-Options:
//! nosniff`, a policy that runs nothing (`default-src 'none'; sandbox`), `Content-Disposition:
//! attachment` for every type that can carry script (PDF), and cache headers: a private file is
//! `private, no-store`, so no shared cache holds it (EDG-1592); a public file may be held by a
//! shared cache until its address expires, since its address is derived from its content
//! (`copy-edge-public-media`). One byte range is served when asked for.

use super::address::{self, Address, Scope};
use super::{access, not_found, sniff, storage, FileState};
use crate::app::{ApiError, App};
use serde::Deserialize;
use strata_kernel::{Context, Principal};

/// A delivery request: the parts of the address and the `Range` header, when one was sent.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeliveryRequest {
    pub file: String,
    pub expires: String,
    pub scope: String,
    pub signature: String,
    #[serde(default)]
    pub range: Option<String>,
}

/// A delivered file: the status, the headers and the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Delivery {
    /// The value of a header.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(have, _)| have.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The byte range `bytes=<a>-<b>`, `bytes=<a>-` or `bytes=-<n>` of a file of `size` bytes, as
/// `(start, length)`. `None` when no single range is asked for (the whole file is sent);
/// `Some(Err(()))` when the range cannot be satisfied.
fn range_of(header: Option<&str>, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let number = |text: &str| -> Option<u64> {
        (!text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    let (start, end) = match (first.trim(), last.trim()) {
        ("", suffix) => {
            let n = number(suffix)?;
            if n == 0 {
                return Some(Err(()));
            }
            (size.saturating_sub(n), size.saturating_sub(1))
        }
        (from, "") => (number(from)?, size.saturating_sub(1)),
        (from, to) => (number(from)?, number(to)?.min(size.saturating_sub(1))),
    };
    if size == 0 || start >= size || end < start {
        return Some(Err(()));
    }
    Some(Ok((start, end - start + 1)))
}

/// Deliver a file at a signed address, as the principal of `ctx` (the session's, or anonymous).
///
/// Implements: CT-205, EDG-012, EDG-1592, CMS-260
pub fn deliver_file(
    app: &App,
    ctx: &Context,
    request: &DeliveryRequest,
) -> Result<Delivery, ApiError> {
    let address = Address::parse(
        &request.file,
        &request.expires,
        &request.scope,
        &request.signature,
    )
    .ok_or_else(not_found)?;
    if !address::verifies(app, ctx, &address) {
        return Err(not_found());
    }
    let meta = storage::get_meta(app, &ctx.tenant, &address.file)?.ok_or_else(not_found)?;
    if meta.state != FileState::Clean {
        return Err(not_found());
    }
    let reader = match address.scope {
        Scope::Public => access::as_principal(ctx, Principal::Anonymous),
        Scope::Principal => ctx.clone(),
    };
    if !access::may_read(app, &reader, &meta, None)? {
        return Err(not_found());
    }
    let path = storage::bytes_path(app, &ctx.tenant, &meta.file)?;
    let size = std::fs::metadata(&path).map_err(|_| not_found())?.len();
    let kind = sniff::accepted(&meta.media_type);
    let inline = kind.is_some_and(|k| k.inline);
    let extension = kind.map_or("bin", |k| k.extension);
    let mut headers: Vec<(&'static str, String)> = vec![
        ("content-type", meta.media_type.clone()),
        ("x-content-type-options", "nosniff".into()),
        (
            "content-security-policy",
            "default-src 'none'; sandbox".into(),
        ),
        ("cross-origin-resource-policy", "same-origin".into()),
        ("referrer-policy", "no-referrer".into()),
        ("accept-ranges", "bytes".into()),
        ("etag", format!("\"{}\"", meta.file)),
        (
            "content-disposition",
            format!(
                "{}; filename=\"{}.{extension}\"",
                if inline { "inline" } else { "attachment" },
                &meta.file[..16]
            ),
        ),
    ];
    match address.scope {
        Scope::Public => {
            let seconds = ((address.expires_at - ctx.now.0) / 1000).max(0);
            headers.push((
                "cache-control",
                format!("public, max-age={seconds}, immutable"),
            ));
        }
        Scope::Principal => headers.push(("cache-control", "private, no-store".into())),
    }
    match range_of(request.range.as_deref(), size) {
        None => {
            let body = storage::read_range(&path, 0, size)?;
            headers.push(("content-length", body.len().to_string()));
            Ok(Delivery {
                status: 200,
                headers,
                body,
            })
        }
        Some(Ok((start, length))) => {
            let body = storage::read_range(&path, start, length)?;
            headers.push((
                "content-range",
                format!("bytes {start}-{}/{size}", start + length - 1),
            ));
            headers.push(("content-length", body.len().to_string()));
            Ok(Delivery {
                status: 206,
                headers,
                body,
            })
        }
        Some(Err(())) => {
            headers.push(("content-range", format!("bytes */{size}")));
            headers.push(("content-length", "0".into()));
            Ok(Delivery {
                status: 416,
                headers,
                body: Vec::new(),
            })
        }
    }
}
