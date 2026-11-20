//! Subject keys: the keys personal fields are sealed under, and their destruction at an erasure
//! (D-350, PRV-358, PRV-362, SEC-001, SEC-003; the sealing design D-7720 to D-7728).
//!
//! The hierarchy, as far as it exists without the external key service:
//!
//! - **Process key.** Held by the key service. Today that is [`InProcessKeyService`], which holds
//!   the platform's sealing key (`FactorPolicy::seal_key`, with the previous keys of a rotation).
//!   The [`KeyService`] trait is the boundary the real key service (component `key-service`)
//!   replaces: it only wraps and unwraps tenant keys.
//! - **Tenant key.** One per tenant, made on the first sealing, kept in the tenant database
//!   wrapped by the key service (identity document `tenant-key`). Subject keys are wrapped under
//!   it, and the blind hashes of searchable fields are keyed by a key derived from it.
//! - **Subject key.** One per data subject of a tenant, made on first use, kept in the tenant
//!   database wrapped under the tenant key (identity document `subject-key`, by key id, and
//!   `subject-key-of`, from the principal to its live key id). Destroying it removes the wrapped
//!   key and the principal and keeps the key id and the time (rec-subject-key); every value sealed
//!   under it, in every copy the platform reads, is unreadable from then on.
//!
//! A field value is sealed with AES-256-GCM under a key derived from the subject key, with a nonce
//! derived (HMAC-SHA-256) from another derived key, the binding and the value: equal values of one
//! subject and field seal to equal tokens, which is what lets a replay reproduce the sealed hashes
//! of the journal. The binding (associated data) names the tenant, the key id, the object type and
//! the field (or the audit scope and the member) and the blind hashes, so a token moved to another
//! tenant or another field does not open.

use crate::app::SealKey;
use crate::sealing;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::{PrincipalId, TenantId, Timestamp};
use strata_protocol::log::now_ms;
use strata_storage_api::sealed::{CipherSession, FieldCipher};
use strata_storage_api::{Store, StoreError, StoreResult};

/// The identity document of a tenant's key, under the key [`TENANT_KEY_NAME`].
pub const TENANT_KEY_KIND: &str = "tenant-key";
const TENANT_KEY_NAME: &str = "current";
/// The identity documents of subject keys, by key id.
pub const SUBJECT_KEY_KIND: &str = "subject-key";
/// The identity documents that name the live key of a principal, by principal id.
pub const SUBJECT_INDEX_KIND: &str = "subject-key-of";

const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

/// The boundary of the key service: it wraps and unwraps the key of a tenant and holds what does.
/// The external key service (component `key-service`, SEC-003) replaces the in-process one.
pub trait KeyService: Send + Sync {
    /// Wrap the key of `tenant` whose id is `key_id`.
    fn wrap(&self, tenant: &TenantId, key_id: &str, key: &[u8; 32]) -> Result<String, String>;
    /// Unwrap it, or `None` when this service holds no key that opens it for this tenant and id.
    fn unwrap(&self, tenant: &TenantId, key_id: &str, wrapped: &str) -> Option<[u8; 32]>;
}

/// The key service inside the platform process: tenant keys wrapped under the platform's sealing
/// key, which is kept out of the store.
pub struct InProcessKeyService {
    current: SealKey,
    previous: Vec<SealKey>,
}

impl InProcessKeyService {
    pub fn new(current: SealKey, previous: Vec<SealKey>) -> Self {
        InProcessKeyService { current, previous }
    }
}

fn tenant_binding(tenant: &TenantId, key_id: &str) -> Vec<u8> {
    format!("strata/tenant-key/v1\0{tenant}\0{key_id}").into_bytes()
}

impl KeyService for InProcessKeyService {
    fn wrap(&self, tenant: &TenantId, key_id: &str, key: &[u8; 32]) -> Result<String, String> {
        sealing::seal_with(&self.current, &tenant_binding(tenant, key_id), key)
            .map_err(|error| error.describe())
    }

    fn unwrap(&self, tenant: &TenantId, key_id: &str, wrapped: &str) -> Option<[u8; 32]> {
        let keys: Vec<&SealKey> = std::iter::once(&self.current)
            .chain(self.previous.iter())
            .collect();
        let bytes = sealing::open_among(&keys, &tenant_binding(tenant, key_id), wrapped)?;
        <[u8; 32]>::try_from(bytes.as_slice()).ok()
    }
}

/// The document of a tenant key.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TenantKeyDoc {
    key_id: String,
    wrapped: String,
    created_at: i64,
}

/// The document of a subject key (rec-subject-key). A destroyed key keeps its id and the time.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SubjectKeyDoc {
    key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrapped: Option<String>,
    /// `live` or `destroyed` (subject-key-state).
    state: String,
    created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destroyed_at: Option<i64>,
}

/// The document that names the live key of a principal.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SubjectIndexDoc {
    key_id: String,
}

fn corrupt(what: &str) -> impl Fn(serde_json::Error) -> StoreError + '_ {
    move |error| StoreError::Corrupt(format!("{what}: {error}"))
}

fn random<const N: usize>() -> StoreResult<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| StoreError::Unavailable(format!("no randomness: {error}")))?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// HMAC-SHA-256 of the concatenation of `parts` (RFC 2104).
fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// A key derived from `key` for one use.
fn derive(key: &[u8; 32], label: &str) -> [u8; 32] {
    hmac(key, &[label.as_bytes()])
}

/// AES-256-GCM with a random nonce, for wrapping one key under another.
fn wrap_under(key: &[u8; 32], binding: &[u8], secret: &[u8]) -> StoreResult<String> {
    let nonce = random::<NONCE_BYTES>()?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|error| StoreError::Unavailable(format!("wrapping a key: {error}")))?;
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: secret,
                aad: binding,
            },
        )
        .map_err(|error| StoreError::Unavailable(format!("wrapping a key: {error}")))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    Ok(URL_SAFE_NO_PAD.encode(out))
}

fn open_under(key: &[u8; 32], binding: &[u8], text: &str) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    if bytes.len() < NONCE_BYTES + TAG_BYTES {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(NONCE_BYTES);
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: binding,
            },
        )
        .ok()
}

fn subject_binding(tenant: &TenantId, key_id: &str) -> Vec<u8> {
    format!("strata/subject-key/v1\0{tenant}\0{key_id}").into_bytes()
}

fn field_binding(
    tenant: &TenantId,
    key_id: &str,
    scope: &str,
    name: &str,
    blinds: &str,
) -> Vec<u8> {
    format!("strata/sealed-field/v1\0{tenant}\0{key_id}\0{scope}\0{name}\0{blinds}").into_bytes()
}

/// The subject keys of every tenant, through the key service: the [`FieldCipher`] of the sealed
/// store.
pub struct SubjectKeys {
    service: Arc<dyn KeyService>,
}

impl SubjectKeys {
    pub fn new(service: Arc<dyn KeyService>) -> Self {
        SubjectKeys { service }
    }

    /// When sealing began for a tenant: the time its key was made, or `None` when nothing of the
    /// tenant has been sealed yet.
    pub fn sealing_since(&self, store: &dyn Store, tenant: &TenantId) -> StoreResult<Option<i64>> {
        match store.doc_get(tenant, TENANT_KEY_KIND, TENANT_KEY_NAME)? {
            Some(text) => {
                let doc: TenantKeyDoc =
                    serde_json::from_str(&text).map_err(corrupt("tenant key"))?;
                Ok(Some(doc.created_at))
            }
            None => Ok(None),
        }
    }

    /// Destroy every live key of `subject` in `tenant` (D-350, CT-155): the wrapped key and the
    /// principal are removed from its document and the key id and the time are kept; the link
    /// from the principal to its key goes. Returns the ids of the keys destroyed, none when the
    /// subject had none. A later write that names the principal makes a new key.
    pub fn destroy(
        &self,
        store: &dyn Store,
        tenant: &TenantId,
        subject: &PrincipalId,
        at: Timestamp,
    ) -> StoreResult<Vec<String>> {
        let mut destroyed = Vec::new();
        for (key_id, text) in store.doc_list(tenant, SUBJECT_KEY_KIND)? {
            let Ok(doc) = serde_json::from_str::<SubjectKeyDoc>(&text) else {
                continue;
            };
            if doc.state != "live" || doc.subject.as_deref() != Some(subject.as_str()) {
                continue;
            }
            let gone = SubjectKeyDoc {
                key_id: doc.key_id.clone(),
                subject: None,
                wrapped: None,
                state: "destroyed".to_string(),
                created_at: doc.created_at,
                destroyed_at: Some(at.0),
            };
            let text = serde_json::to_string(&gone).map_err(corrupt("subject key"))?;
            store.doc_put(tenant, SUBJECT_KEY_KIND, &key_id, &text, false)?;
            destroyed.push(doc.key_id);
        }
        store.doc_remove(tenant, SUBJECT_INDEX_KIND, subject.as_str())?;
        Ok(destroyed)
    }
}

impl FieldCipher for SubjectKeys {
    fn session<'a>(
        &'a self,
        store: &'a dyn Store,
        tenant: &'a TenantId,
    ) -> Box<dyn CipherSession + 'a> {
        Box::new(KeySession {
            service: self.service.as_ref(),
            store,
            tenant,
            tenant_key: None,
            by_id: BTreeMap::new(),
            by_subject: BTreeMap::new(),
        })
    }
}

/// The keys of one tenant for one store call. Nothing outlives it.
struct KeySession<'a> {
    service: &'a dyn KeyService,
    store: &'a dyn Store,
    tenant: &'a TenantId,
    /// The tenant key once looked up: `Some(None)` when the tenant has none.
    tenant_key: Option<Option<[u8; 32]>>,
    /// Subject keys by id, once looked up: `None` when destroyed or unknown.
    by_id: BTreeMap<String, Option<[u8; 32]>>,
    /// The live key of each subject sealed for in this call.
    by_subject: BTreeMap<PrincipalId, (String, [u8; 32])>,
}

impl KeySession<'_> {
    fn read_tenant_key(&self) -> StoreResult<Option<[u8; 32]>> {
        let Some(text) = self
            .store
            .doc_get(self.tenant, TENANT_KEY_KIND, TENANT_KEY_NAME)?
        else {
            return Ok(None);
        };
        let doc: TenantKeyDoc = serde_json::from_str(&text).map_err(corrupt("tenant key"))?;
        match self.service.unwrap(self.tenant, &doc.key_id, &doc.wrapped) {
            Some(key) => Ok(Some(key)),
            None => Err(StoreError::Unavailable(format!(
                "the key of tenant {} does not open with the keys this process holds",
                self.tenant
            ))),
        }
    }

    fn tenant_key(&mut self, create: bool) -> StoreResult<Option<[u8; 32]>> {
        if let Some(Some(key)) = self.tenant_key {
            return Ok(Some(key));
        }
        if self.tenant_key.is_none() {
            let found = self.read_tenant_key()?;
            self.tenant_key = Some(found);
            if found.is_some() {
                return Ok(found);
            }
        }
        if !create {
            return Ok(None);
        }
        let key = random::<32>()?;
        let key_id = hex(&random::<8>()?);
        let wrapped = self
            .service
            .wrap(self.tenant, &key_id, &key)
            .map_err(StoreError::Unavailable)?;
        let doc = TenantKeyDoc {
            key_id,
            wrapped,
            created_at: now_ms(),
        };
        let text = serde_json::to_string(&doc).map_err(corrupt("tenant key"))?;
        let kept =
            self.store
                .doc_put(self.tenant, TENANT_KEY_KIND, TENANT_KEY_NAME, &text, true)?;
        // Another writer made the key first: theirs is the tenant's.
        let key = if kept {
            Some(key)
        } else {
            self.read_tenant_key()?
        };
        self.tenant_key = Some(key);
        Ok(key)
    }

    fn key_by_id(&mut self, key_id: &str) -> StoreResult<Option<[u8; 32]>> {
        if let Some(found) = self.by_id.get(key_id) {
            return Ok(*found);
        }
        let found = match self.store.doc_get(self.tenant, SUBJECT_KEY_KIND, key_id)? {
            None => None,
            Some(text) => {
                let doc: SubjectKeyDoc =
                    serde_json::from_str(&text).map_err(corrupt("subject key"))?;
                match (doc.state.as_str(), doc.wrapped) {
                    ("live", Some(wrapped)) => match self.tenant_key(false)? {
                        Some(tenant_key) => {
                            open_under(&tenant_key, &subject_binding(self.tenant, key_id), &wrapped)
                                .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                        }
                        None => None,
                    },
                    _ => None,
                }
            }
        };
        self.by_id.insert(key_id.to_string(), found);
        Ok(found)
    }

    fn live_key_of(&mut self, subject: &PrincipalId) -> StoreResult<Option<(String, [u8; 32])>> {
        let Some(text) = self
            .store
            .doc_get(self.tenant, SUBJECT_INDEX_KIND, subject.as_str())?
        else {
            return Ok(None);
        };
        let index: SubjectIndexDoc =
            serde_json::from_str(&text).map_err(corrupt("subject key index"))?;
        Ok(self
            .key_by_id(&index.key_id)?
            .map(|key| (index.key_id, key)))
    }

    fn subject_key(
        &mut self,
        subject: &PrincipalId,
        create: bool,
    ) -> StoreResult<Option<(String, [u8; 32])>> {
        if let Some(found) = self.by_subject.get(subject) {
            return Ok(Some(found.clone()));
        }
        let mut found = self.live_key_of(subject)?;
        if found.is_none() && create {
            let Some(tenant_key) = self.tenant_key(true)? else {
                return Err(StoreError::Unavailable("no tenant key was made".into()));
            };
            let key = random::<32>()?;
            let key_id = hex(&random::<16>()?);
            let doc = SubjectKeyDoc {
                key_id: key_id.clone(),
                subject: Some(subject.as_str().to_string()),
                wrapped: Some(wrap_under(
                    &tenant_key,
                    &subject_binding(self.tenant, &key_id),
                    &key,
                )?),
                state: "live".to_string(),
                created_at: now_ms(),
                destroyed_at: None,
            };
            let text = serde_json::to_string(&doc).map_err(corrupt("subject key"))?;
            self.store
                .doc_put(self.tenant, SUBJECT_KEY_KIND, &key_id, &text, true)?;
            let index = serde_json::to_string(&SubjectIndexDoc {
                key_id: key_id.clone(),
            })
            .map_err(corrupt("subject key index"))?;
            if self.store.doc_put(
                self.tenant,
                SUBJECT_INDEX_KIND,
                subject.as_str(),
                &index,
                true,
            )? {
                self.by_id.insert(key_id.clone(), Some(key));
                found = Some((key_id, key));
            } else {
                // Another writer made the subject's key first: ours is never used.
                self.store
                    .doc_remove(self.tenant, SUBJECT_KEY_KIND, &key_id)?;
                found = self.live_key_of(subject)?;
            }
        }
        if let Some(live) = &found {
            self.by_subject.insert(subject.clone(), live.clone());
        }
        Ok(found)
    }
}

impl CipherSession for KeySession<'_> {
    /// Implements: H-019
    fn seal_one(
        &mut self,
        subject: &PrincipalId,
        scope: &str,
        name: &str,
        plain: &str,
        blinds: &str,
        create: bool,
    ) -> StoreResult<Option<(String, String)>> {
        let Some((key_id, key)) = self.subject_key(subject, create)? else {
            return Ok(None);
        };
        let binding = field_binding(self.tenant, &key_id, scope, name, blinds);
        let length = (binding.len() as u64).to_be_bytes();
        let synthetic = hmac(
            &derive(&key, "strata/sealed-field/nonce/v1"),
            &[&length[..], binding.as_slice(), plain.as_bytes()],
        );
        let nonce = &synthetic[..NONCE_BYTES];
        let cipher = Aes256Gcm::new_from_slice(&derive(&key, "strata/sealed-field/cipher/v1"))
            .map_err(|error| StoreError::Unavailable(format!("sealing a field: {error}")))?;
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plain.as_bytes(),
                    aad: &binding,
                },
            )
            .map_err(|error| StoreError::Unavailable(format!("sealing a field: {error}")))?;
        let mut payload = nonce.to_vec();
        payload.extend_from_slice(&sealed);
        Ok(Some((key_id, URL_SAFE_NO_PAD.encode(payload))))
    }

    /// Implements: H-020
    fn open_one(
        &mut self,
        scope: &str,
        name: &str,
        key_id: &str,
        blinds: &str,
        payload: &str,
    ) -> StoreResult<Option<String>> {
        let Some(key) = self.key_by_id(key_id)? else {
            return Ok(None);
        };
        let binding = field_binding(self.tenant, key_id, scope, name, blinds);
        Ok(open_under(
            &derive(&key, "strata/sealed-field/cipher/v1"),
            &binding,
            payload,
        )
        .and_then(|bytes| String::from_utf8(bytes).ok()))
    }

    fn blind(
        &mut self,
        scope: &str,
        name: &str,
        plain: &str,
        create: bool,
    ) -> StoreResult<Option<String>> {
        let Some(tenant_key) = self.tenant_key(create)? else {
            return Ok(None);
        };
        let bound = format!("{}\0{scope}\0{name}\0{plain}", self.tenant);
        let hash = hmac(
            &derive(&tenant_key, "strata/blind-index/v1"),
            &[bound.as_bytes()],
        );
        Ok(Some(hex(&hash[..16])))
    }
}
