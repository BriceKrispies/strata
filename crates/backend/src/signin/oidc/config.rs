//! The identity providers of each tenant, and what is fetched from them.
//!
//! **Where the configuration lives.** Provider configuration is platform configuration, not
//! tenant data: one JSON file, named by `STRATA_OIDC_CONFIG`, that maps a tenant to its providers.
//! A tenant's bundle is data its own people can change, and a provider decides who may sign in;
//! the operator, who also holds the client secret, is the one who changes it. The client secret is
//! never in the file: the file names an environment variable (`client_secret_env`), which is read
//! once at start and kept in a value that does not print.
//!
//! **What is fetched.** The provider's metadata (`.well-known/openid-configuration`) and its
//! signing keys, per provider of a tenant and never shared between providers (FED-256). Keys are
//! fetched when first needed and again, once, when a token names a key that is not held (FED-004),
//! no more often than the refresh floor, so that tokens naming made-up keys cannot make this
//! process fetch without end.
//!
//! Implements: FED-004, FED-009, FED-010, FED-256, H-036

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::Algorithm;
use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::{Arc, Mutex};
use strata_kernel::TenantId;

use crate::signin::saml::{config as saml_config, SamlProvider};

/// The most a provider's answer may hold, so that a provider cannot fill this process's memory.
const MAX_BYTES: usize = 256 * 1024;
/// The most keys of a provider that are kept.
const MAX_KEYS: usize = 64;
/// How long a provider's metadata is used before it is fetched again.
const METADATA_TTL_MS: i64 = 3_600_000;
/// The least time between two fetches of a provider's keys that a token with an unknown key may
/// cause.
const REFRESH_FLOOR_MS: i64 = 30_000;

/// A secret that does not print.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// One identity provider of one tenant.
#[derive(Clone, Debug)]
pub struct ProviderConfig {
    /// The name in the tenant's configuration and in the stored identities: lower-case letters,
    /// digits and hyphens.
    pub id: String,
    /// What the sign-in page calls it.
    pub label: String,
    /// A provider that is not enabled is offered nowhere and accepts nothing, and binds no domain
    /// (FED-011).
    pub enabled: bool,
    /// The issuer, exactly as the provider's tokens and metadata spell it.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<Secret>,
    pub scopes: Vec<String>,
    /// The signature methods an identity token may use. `none` and the shared-secret methods are
    /// not in the vocabulary, so no configuration can allow them (FED-255).
    pub algorithms: Vec<Algorithm>,
    /// Email domains the provider decides for: password and emailed-link sign-in is refused for
    /// an address in one of them while the provider is enabled (FED-009).
    pub domains: Vec<String>,
    /// Whether a first sign-in creates a principal (FED-011).
    pub create_on_first_sign_in: bool,
    /// The one role a principal created at a first sign-in receives (FED-258).
    pub default_role: Option<String>,
    /// Whether a provider's verified email address links to the account whose login it is, with no
    /// sign-in to that account first. Off unless the realm decides to trust the provider that far
    /// (FED-257).
    pub link_verified_email: bool,
}

/// A provider as the configuration file spells it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderFile {
    id: String,
    label: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
    issuer: String,
    client_id: String,
    client_secret_env: Option<String>,
    scopes: Option<Vec<String>>,
    algorithms: Option<Vec<String>>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    create_on_first_sign_in: bool,
    default_role: Option<String>,
    #[serde(default)]
    link_verified_email: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    tenants: BTreeMap<String, Vec<ProviderFile>>,
}

/// The signature methods that can be configured: asymmetric ones only.
pub fn algorithm(name: &str) -> Option<Algorithm> {
    match name {
        "RS256" => Some(Algorithm::RS256),
        "ES256" => Some(Algorithm::ES256),
        "EdDSA" => Some(Algorithm::EdDSA),
        _ => None,
    }
}

impl ProviderConfig {
    fn from_file(file: ProviderFile) -> Result<Self, String> {
        let name = file.id.clone();
        let bad = |why: &str| format!("provider {name}: {why}");
        let id_ok = !file.id.is_empty()
            && file.id.len() <= 32
            && file
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !id_ok {
            return Err(bad(
                "the id is 1 to 32 lower-case letters, digits and hyphens",
            ));
        }
        if file.issuer.is_empty() || file.client_id.is_empty() {
            return Err(bad("issuer and client_id are required"));
        }
        let client_secret = match file.client_secret_env.as_deref() {
            None => None,
            Some(variable) => match std::env::var(variable) {
                Ok(value) if !value.is_empty() => Some(Secret::new(value)),
                _ => {
                    return Err(bad(&format!(
                        "the environment variable {variable} is not set"
                    )))
                }
            },
        };
        let mut algorithms = Vec::new();
        for method in file.algorithms.unwrap_or_else(|| vec!["RS256".into()]) {
            algorithms.push(
                algorithm(&method)
                    .ok_or_else(|| bad(&format!("signature method {method:?} is not allowed")))?,
            );
        }
        if algorithms.is_empty() {
            return Err(bad("at least one signature method is required"));
        }
        let mut scopes = file
            .scopes
            .unwrap_or_else(|| vec!["openid".into(), "email".into(), "profile".into()]);
        if !scopes.iter().any(|scope| scope == "openid") {
            scopes.insert(0, "openid".into());
        }
        let domains: Vec<String> = file
            .domains
            .iter()
            .map(|domain| domain.trim().to_lowercase())
            .filter(|domain| !domain.is_empty())
            .collect();
        crate::signin::federated::check_admission(
            &domains,
            file.create_on_first_sign_in,
            file.link_verified_email,
            file.default_role.as_deref(),
        )
        .map_err(bad)?;
        Ok(ProviderConfig {
            label: file.label.unwrap_or_else(|| file.id.clone()),
            id: file.id,
            enabled: file.enabled,
            issuer: file.issuer,
            client_id: file.client_id,
            client_secret,
            scopes,
            algorithms,
            domains,
            create_on_first_sign_in: file.create_on_first_sign_in,
            default_role: file.default_role,
            link_verified_email: file.link_verified_email,
        })
    }
}

/// What a provider says about itself.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// Something that fetches from a provider. The one that is used talks HTTP; a test may put another
/// in its place.
pub trait Fetch: Send + Sync {
    fn get_json(&self, url: &str) -> Result<Json, String>;
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Json, String>;
}

/// The HTTP fetcher. It follows no redirect (a provider that redirects is misconfigured, and a
/// redirect is how a request is steered somewhere else), times out and reads a bounded answer.
///
/// The backend's request code is synchronous and runs on a thread meant for blocking, so a call
/// waits for the client's answer on that thread.
pub struct Http {
    client: reqwest::Client,
}

impl Default for Http {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Http { client }
    }
}

async fn read_json(response: reqwest::Response) -> Result<Json, String> {
    if !response.status().is_success() {
        return Err(format!("the provider answered {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err("the provider's answer is too large".into());
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("the provider's answer is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn wait<T>(work: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "no runtime to fetch with".to_string())?;
    handle.block_on(work)
}

impl Fetch for Http {
    fn get_json(&self, url: &str) -> Result<Json, String> {
        wait(async {
            let response = self
                .client
                .get(url)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|e| e.to_string())?;
            read_json(response).await
        })
    }

    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<Json, String> {
        wait(async {
            let response = self
                .client
                .post(url)
                .header("accept", "application/json")
                .form(form)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            read_json(response).await
        })
    }
}

/// What is held about one provider of one tenant.
struct Cached {
    metadata: Metadata,
    metadata_at: i64,
    keys: Option<JwkSet>,
    keys_at: i64,
}

/// Every tenant's providers, and what has been fetched from them.
pub struct Federation {
    providers: BTreeMap<TenantId, Vec<ProviderConfig>>,
    /// The tenants' SAML providers. They share this value with the OpenID Connect ones because
    /// what is decided about a person's address, which domain a provider is bound to, is decided
    /// across both (FED-009).
    saml: BTreeMap<TenantId, Vec<SamlProvider>>,
    pub(super) fetch: Arc<dyn Fetch>,
    cache: Mutex<BTreeMap<(TenantId, String), Cached>>,
    refresh_floor_ms: i64,
}

impl Debug for Federation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Federation")
            .field(
                "providers",
                &self
                    .providers
                    .iter()
                    .map(|(tenant, list)| (tenant.as_str().to_string(), list.len()))
                    .collect::<BTreeMap<_, _>>(),
            )
            .field(
                "saml",
                &self
                    .saml
                    .iter()
                    .map(|(tenant, list)| (tenant.as_str().to_string(), list.len()))
                    .collect::<BTreeMap<_, _>>(),
            )
            .finish()
    }
}

/// Whether an address is on the platform's own side of the network: loopback, private, link-local
/// (where cloud metadata services live) or unspecified.
fn internal_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                // 100.64.0.0/10, shared address space
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unspecified()
                || (a.segments()[0] & 0xfe00) == 0xfc00
                || (a.segments()[0] & 0xffc0) == 0xfe80
                || a.to_ipv4_mapped()
                    .is_some_and(|v4| internal_ip(IpAddr::V4(v4)))
        }
    }
}

/// Whether a host is one a tenant may make this process connect to: not an address on the
/// platform's own network, and not a name that is only ever one (FED-261, D-702).
fn outward_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return !internal_ip(ip);
    }
    let name = host.to_ascii_lowercase();
    name.contains('.')
        && name != "localhost"
        && !name.ends_with(".localhost")
        && !name.ends_with(".local")
        && !name.ends_with(".internal")
}

/// Whether an address may be fetched from: HTTPS, with no credentials in it, and a host outside
/// the platform's own network. In development plain HTTP and any host are allowed too, so that a
/// provider on this machine can be used (FED-261).
///
/// Implements: FED-261
fn endpoint_ok(address: &str, dev: bool) -> bool {
    let Ok(url) = reqwest::Url::parse(address) else {
        return false;
    };
    let scheme = url.scheme() == "https" || (dev && url.scheme() == "http");
    scheme
        && url.host_str().is_some_and(|host| dev || outward_host(host))
        && url.username().is_empty()
        && url.password().is_none()
}

/// The keys of a set that can be read; one that cannot is left out and does not spoil the rest.
fn parse_keys(value: &Json) -> JwkSet {
    let keys: Vec<Jwk> = value
        .get("keys")
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|key| serde_json::from_value::<Jwk>(key.clone()).ok())
                .take(MAX_KEYS)
                .collect()
        })
        .unwrap_or_default();
    JwkSet { keys }
}

impl Federation {
    /// No tenant has a provider.
    pub fn none() -> Self {
        Federation::new(BTreeMap::new(), Arc::new(Http::default()))
    }

    pub fn new(providers: BTreeMap<TenantId, Vec<ProviderConfig>>, fetch: Arc<dyn Fetch>) -> Self {
        Federation {
            providers,
            saml: BTreeMap::new(),
            fetch,
            cache: Mutex::new(BTreeMap::new()),
            refresh_floor_ms: REFRESH_FLOOR_MS,
        }
    }

    /// Providers that fetch over HTTP, from the platform configuration file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: ConfigFile =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut providers = BTreeMap::new();
        for (tenant, list) in file.tenants {
            let tenant_id = TenantId::new(tenant.clone()).map_err(|e| format!("{tenant}: {e}"))?;
            let mut seen: Vec<String> = Vec::new();
            let mut parsed = Vec::new();
            for provider in list {
                let provider = ProviderConfig::from_file(provider)
                    .map_err(|e| format!("{}: {tenant}: {e}", path.display()))?;
                if seen.contains(&provider.id) {
                    return Err(format!(
                        "{}: {tenant}: provider {} appears twice",
                        path.display(),
                        provider.id
                    ));
                }
                seen.push(provider.id.clone());
                parsed.push(provider);
            }
            providers.insert(tenant_id, parsed);
        }
        Ok(Federation::new(providers, Arc::new(Http::default())))
    }

    /// The same providers and these SAML providers of each tenant.
    pub fn with_saml(mut self, saml: BTreeMap<TenantId, Vec<SamlProvider>>) -> Self {
        self.saml = saml;
        self
    }

    /// The same providers and the SAML providers of the platform configuration file.
    pub fn with_saml_file(self, path: &Path) -> Result<Self, String> {
        Ok(self.with_saml(saml_config::from_file(path)?))
    }

    /// The least time between two fetches of a provider's keys that unknown key names cause.
    pub fn with_refresh_floor(mut self, ms: i64) -> Self {
        self.refresh_floor_ms = ms;
        self
    }

    pub fn providers(&self, tenant: &TenantId) -> &[ProviderConfig] {
        self.providers.get(tenant).map_or(&[], Vec::as_slice)
    }

    pub fn provider(&self, tenant: &TenantId, id: &str) -> Option<&ProviderConfig> {
        self.providers(tenant).iter().find(|p| p.id == id)
    }

    pub fn saml_providers(&self, tenant: &TenantId) -> &[SamlProvider] {
        self.saml.get(tenant).map_or(&[], Vec::as_slice)
    }

    pub fn saml_provider(&self, tenant: &TenantId, id: &str) -> Option<&SamlProvider> {
        self.saml_providers(tenant).iter().find(|p| p.id == id)
    }

    /// Whether an enabled provider of the tenant decides for the domain of this address
    /// (FED-009). An address with no domain is not bound.
    ///
    /// Implements: FED-009
    pub fn domain_bound(&self, tenant: &TenantId, address: &str) -> bool {
        let Some((_, domain)) = address.trim().rsplit_once('@') else {
            return false;
        };
        let domain = domain.to_lowercase();
        self.providers(tenant)
            .iter()
            .any(|p| p.enabled && p.domains.iter().any(|bound| *bound == domain))
            || self
                .saml_providers(tenant)
                .iter()
                .any(|p| p.enabled && p.domains.iter().any(|bound| *bound == domain))
    }

    fn slot(&self, tenant: &TenantId, provider: &ProviderConfig) -> (TenantId, String) {
        (tenant.clone(), provider.id.clone())
    }

    /// The provider's metadata: from what is held while it is fresh, otherwise fetched. A provider
    /// that does not answer leaves the last metadata in use, and a provider with none held is an
    /// error.
    pub fn metadata(
        &self,
        tenant: &TenantId,
        provider: &ProviderConfig,
        now_ms: i64,
        dev: bool,
    ) -> Result<Metadata, String> {
        let slot = self.slot(tenant, provider);
        let held = {
            let cache = self.cache.lock().map_err(|_| "the cache is poisoned")?;
            cache
                .get(&slot)
                .map(|cached| (cached.metadata.clone(), cached.metadata_at))
        };
        if let Some((metadata, at)) = &held {
            if now_ms - at < METADATA_TTL_MS {
                return Ok(metadata.clone());
            }
        }
        match self.discover(provider, dev) {
            Ok(metadata) => {
                let mut cache = self.cache.lock().map_err(|_| "the cache is poisoned")?;
                match cache.get_mut(&slot) {
                    Some(cached) => {
                        // Keys named by new metadata come from a new address.
                        if cached.metadata.jwks_uri != metadata.jwks_uri {
                            cached.keys = None;
                        }
                        cached.metadata = metadata.clone();
                        cached.metadata_at = now_ms;
                    }
                    None => {
                        cache.insert(
                            slot,
                            Cached {
                                metadata: metadata.clone(),
                                metadata_at: now_ms,
                                keys: None,
                                keys_at: 0,
                            },
                        );
                    }
                }
                Ok(metadata)
            }
            Err(error) => held.map(|(metadata, _)| metadata).ok_or(error),
        }
    }

    fn discover(&self, provider: &ProviderConfig, dev: bool) -> Result<Metadata, String> {
        let address = format!(
            "{}/.well-known/openid-configuration",
            provider.issuer.trim_end_matches('/')
        );
        if !endpoint_ok(&address, dev) {
            return Err("the issuer is not an address that may be fetched".into());
        }
        let document = self.fetch.get_json(&address)?;
        let text = |name: &str| {
            document
                .get(name)
                .and_then(Json::as_str)
                .map(str::to_string)
                .filter(|value| endpoint_ok(value, dev))
        };
        // The issuer the metadata names is the one configured, exactly: a provider that names
        // another has been put in the place of it.
        if document.get("issuer").and_then(Json::as_str) != Some(provider.issuer.as_str()) {
            return Err("the metadata names another issuer".into());
        }
        Ok(Metadata {
            authorization_endpoint: text("authorization_endpoint")
                .ok_or("the metadata has no usable authorization_endpoint")?,
            token_endpoint: text("token_endpoint")
                .ok_or("the metadata has no usable token_endpoint")?,
            jwks_uri: text("jwks_uri").ok_or("the metadata has no usable jwks_uri")?,
        })
    }

    /// The provider's signing keys. `refresh` says that a token named a key that is not held: the
    /// keys are fetched again, unless they were fetched less than the refresh floor ago, in which
    /// case what is held is returned and the token will not verify (FED-004).
    ///
    /// Implements: FED-004, FED-256
    pub fn keys(
        &self,
        tenant: &TenantId,
        provider: &ProviderConfig,
        metadata: &Metadata,
        now_ms: i64,
        refresh: bool,
    ) -> Result<JwkSet, String> {
        let slot = self.slot(tenant, provider);
        {
            let cache = self.cache.lock().map_err(|_| "the cache is poisoned")?;
            if let Some(cached) = cache.get(&slot) {
                if let Some(keys) = &cached.keys {
                    let fresh = now_ms - cached.keys_at < self.refresh_floor_ms;
                    if !refresh || fresh {
                        return Ok(keys.clone());
                    }
                }
            }
        }
        let keys = parse_keys(&self.fetch.get_json(&metadata.jwks_uri)?);
        let mut cache = self.cache.lock().map_err(|_| "the cache is poisoned")?;
        if let Some(cached) = cache.get_mut(&slot) {
            cached.keys = Some(keys.clone());
            cached.keys_at = now_ms;
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(domains: &[&str], enabled: bool) -> ProviderConfig {
        ProviderConfig {
            id: "acme".into(),
            label: "Acme".into(),
            enabled,
            issuer: "https://id.acme.example".into(),
            client_id: "client".into(),
            client_secret: Some(Secret::new("hunter2")),
            scopes: vec!["openid".into()],
            algorithms: vec![Algorithm::RS256],
            domains: domains.iter().map(|d| d.to_string()).collect(),
            create_on_first_sign_in: false,
            default_role: None,
            link_verified_email: false,
        }
    }

    fn federation(tenant: &TenantId, enabled: bool) -> Federation {
        Federation::new(
            BTreeMap::from([(tenant.clone(), vec![provider(&["acme.example"], enabled)])]),
            Arc::new(Http::default()),
        )
    }

    /// Verifies: FED-009
    #[test]
    fn only_an_enabled_provider_binds_its_domains() {
        let tenant = TenantId::new("fieldnotes").unwrap();
        let on = federation(&tenant, true);
        assert!(on.domain_bound(&tenant, "ana@acme.example"));
        assert!(on.domain_bound(&tenant, "Ana@ACME.example"));
        assert!(!on.domain_bound(&tenant, "ana@acme.example.evil"));
        assert!(!on.domain_bound(&tenant, "ana@mail.acme.example"));
        assert!(!on.domain_bound(&tenant, "ines"));
        assert!(!federation(&tenant, false).domain_bound(&tenant, "ana@acme.example"));
        assert!(!on.domain_bound(&TenantId::new("tidewater").unwrap(), "ana@acme.example"));
    }

    #[test]
    fn a_secret_does_not_print_and_endpoints_must_be_https_outside_development() {
        let shown = format!("{:?}", provider(&[], true));
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(endpoint_ok("https://id.acme.example/keys", false));
        assert!(!endpoint_ok("http://id.acme.example/keys", false));
        assert!(endpoint_ok("http://127.0.0.1:9/keys", true));
        assert!(!endpoint_ok("https://user:pw@id.acme.example/keys", true));
        assert!(!endpoint_ok("file:///etc/passwd", true));
        // FED-261: the platform's own network is not a provider's address.
        for internal in [
            "https://127.0.0.1/keys",
            "https://10.0.0.5/keys",
            "https://192.168.1.1/keys",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/keys",
            "https://[fd00::1]/keys",
            "https://[::ffff:10.0.0.1]/keys",
            "https://localhost/keys",
            "https://metadata.internal/keys",
            "https://printer.local/keys",
            "https://intranet/keys",
        ] {
            assert!(!endpoint_ok(internal, false), "{internal}");
            assert!(endpoint_ok(internal, true), "{internal} in development");
        }
        assert!(endpoint_ok("https://203.0.113.9/keys", false));
        assert_eq!(algorithm("none"), None);
        assert_eq!(algorithm("HS256"), None);
        assert_eq!(algorithm("EdDSA"), Some(Algorithm::EdDSA));
    }
}
