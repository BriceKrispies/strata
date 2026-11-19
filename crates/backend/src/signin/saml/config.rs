//! The SAML identity providers of each tenant.
//!
//! Provider configuration is platform configuration, as it is for OpenID Connect: one JSON file,
//! named by `STRATA_SAML_CONFIG`, that maps a tenant to its providers. A provider decides who may
//! sign in, so its certificate and the entity identifier it signs as are the operator's to set and
//! not the tenant's own people's. There is no secret in the file: the certificate is public.
//!
//! ```json
//! { "tenants": { "fieldnotes": [ {
//!     "id": "acme", "label": "Acme",
//!     "idp_entity_id": "https://idp.acme.example/metadata",
//!     "sso_url": "https://idp.acme.example/sso",
//!     "certificates": ["-----BEGIN CERTIFICATE-----..."],
//!     "domains": ["acme.example"], "create_on_first_sign_in": true, "default_role": "author"
//! } ] } }
//! ```
//!
//! `certificates` may hold the current and the next certificate of the provider together, for a
//! rotation. `certificate_files` names files that hold them, and is read when the process starts.
//!
//! Implements: FED-005, FED-254, H-050

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use strata_kernel::TenantId;

/// The longest certificate that is read.
const MAX_CERT_BYTES: usize = 16 * 1024;
/// The most certificates of one provider.
const MAX_CERTS: usize = 4;

/// One SAML identity provider of one tenant.
#[derive(Clone, Debug)]
pub struct SamlProvider {
    /// The name in the tenant's configuration and in the stored identities: lower-case letters,
    /// digits and hyphens.
    pub id: String,
    /// What the sign-in page calls it.
    pub label: String,
    /// A provider that is not enabled is offered nowhere and accepts nothing, and binds no domain
    /// (FED-011).
    pub enabled: bool,
    /// The issuer, exactly as the provider's assertions spell it (FED-254).
    pub idp_entity_id: String,
    /// Where the browser is sent with the authentication request.
    pub sso_url: String,
    /// The certificates the provider signs its assertions with, in PEM (FED-005). An assertion
    /// signed by anything else is not accepted.
    pub certificates: Vec<String>,
    /// The entity identifier this service presents to the provider, and the audience an assertion
    /// must name (FED-008). When absent it is the address of the service provider's metadata at
    /// the host the sign-in started at.
    pub sp_entity_id: Option<String>,
    /// The attribute that holds the person's email address.
    pub email_attribute: String,
    /// The attribute that holds the person's name.
    pub name_attribute: String,
    /// Whether the provider vouches for the email address it sends. Off unless the operator says
    /// the provider checks addresses: only then does an address bind a domain (FED-009) or link
    /// an account (FED-257).
    pub email_verified: bool,
    /// How far a clock may differ from this process's, in seconds.
    pub skew_secs: i64,
    /// Email domains the provider decides for: password and emailed-link sign-in is refused for
    /// an address in one of them while the provider is enabled (FED-009).
    pub domains: Vec<String>,
    /// Whether a first sign-in creates a principal (FED-011).
    pub create_on_first_sign_in: bool,
    /// The one role a principal created at a first sign-in receives (FED-258).
    pub default_role: Option<String>,
    /// Whether a verified email address links to the account whose login it is, with no sign-in
    /// to that account first (FED-257).
    pub link_verified_email: bool,
}

impl SamlProvider {
    /// The entity identifier presented at `origin`: the configured one, or the address of the
    /// provider's metadata at that origin.
    pub fn entity_id(&self, origin: &str) -> String {
        self.sp_entity_id
            .clone()
            .unwrap_or_else(|| format!("{origin}/federation/saml/metadata/{}", self.id))
    }
}

/// A provider as the configuration file spells it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderFile {
    id: String,
    label: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
    idp_entity_id: String,
    sso_url: String,
    #[serde(default)]
    certificates: Vec<String>,
    #[serde(default)]
    certificate_files: Vec<String>,
    sp_entity_id: Option<String>,
    email_attribute: Option<String>,
    name_attribute: Option<String>,
    #[serde(default)]
    email_verified: bool,
    skew_secs: Option<i64>,
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

/// Whether an address may be sent the browser: HTTPS, with no credentials in it, and in
/// development plain HTTP too, so that a provider on this machine can be used.
pub fn sso_ok(address: &str, dev: bool) -> bool {
    let Some((scheme, rest)) = address.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme == "https" || (dev && scheme == "http");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    scheme_ok
        && !authority.is_empty()
        && !authority.contains('@')
        && address
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'<' && b != b'>')
}

impl SamlProvider {
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
        if file.idp_entity_id.trim().is_empty() || file.idp_entity_id.len() > 512 {
            return Err(bad("idp_entity_id is required"));
        }
        // The address is checked against the strictest rule here; development allows plain HTTP
        // at the time of the request, where the configuration is known.
        if !sso_ok(&file.sso_url, true) {
            return Err(bad("sso_url is not an address the browser may be sent to"));
        }
        let mut certificates = file.certificates;
        for path in &file.certificate_files {
            certificates
                .push(std::fs::read_to_string(path).map_err(|e| bad(&format!("{path}: {e}")))?);
        }
        if certificates.is_empty() || certificates.len() > MAX_CERTS {
            return Err(bad(&format!(
                "between 1 and {MAX_CERTS} certificates are required"
            )));
        }
        for pem in &certificates {
            if pem.len() > MAX_CERT_BYTES {
                return Err(bad("a certificate is too long"));
            }
            bergshamra_keys::loader::load_x509_cert_pem(pem.as_bytes())
                .map_err(|e| bad(&format!("a certificate cannot be read: {e}")))?;
        }
        if let Some(entity) = &file.sp_entity_id {
            if entity.trim().is_empty() || entity.len() > 512 {
                return Err(bad("sp_entity_id is empty or too long"));
            }
        }
        let skew_secs = file.skew_secs.unwrap_or(60);
        if !(0..=300).contains(&skew_secs) {
            return Err(bad("skew_secs is between 0 and 300"));
        }
        Ok(SamlProvider {
            label: file.label.unwrap_or_else(|| file.id.clone()),
            id: file.id,
            enabled: file.enabled,
            idp_entity_id: file.idp_entity_id,
            sso_url: file.sso_url,
            certificates,
            sp_entity_id: file.sp_entity_id,
            email_attribute: file.email_attribute.unwrap_or_else(|| "email".into()),
            name_attribute: file.name_attribute.unwrap_or_else(|| "name".into()),
            email_verified: file.email_verified,
            skew_secs,
            domains: file
                .domains
                .iter()
                .map(|domain| domain.trim().to_lowercase())
                .filter(|domain| !domain.is_empty())
                .collect(),
            create_on_first_sign_in: file.create_on_first_sign_in,
            default_role: file.default_role,
            link_verified_email: file.link_verified_email,
        })
    }
}

/// Every tenant's SAML providers, from the platform configuration file.
pub fn from_file(path: &Path) -> Result<BTreeMap<TenantId, Vec<SamlProvider>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: ConfigFile =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut providers = BTreeMap::new();
    for (tenant, list) in file.tenants {
        let tenant_id = TenantId::new(tenant.clone()).map_err(|e| format!("{tenant}: {e}"))?;
        let mut seen: Vec<String> = Vec::new();
        let mut parsed = Vec::new();
        for provider in list {
            let provider = SamlProvider::from_file(provider)
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
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_on_address_is_https_with_no_credentials_and_development_allows_http() {
        assert!(sso_ok("https://idp.acme.example/sso", false));
        assert!(sso_ok("https://idp.acme.example/sso?tenant=1", false));
        assert!(!sso_ok("http://idp.acme.example/sso", false));
        assert!(sso_ok("http://127.0.0.1:4291/sso", true));
        assert!(!sso_ok("https://user:pw@idp.acme.example/sso", true));
        assert!(!sso_ok("https://idp.acme.example/s so", true));
        assert!(!sso_ok("javascript:alert(1)", true));
        assert!(!sso_ok("https:///sso", true));
    }

    #[test]
    fn the_entity_identifier_defaults_to_the_metadata_address_at_the_host() {
        let mut provider = SamlProvider {
            id: "acme".into(),
            label: "Acme".into(),
            enabled: true,
            idp_entity_id: "https://idp.acme.example".into(),
            sso_url: "https://idp.acme.example/sso".into(),
            certificates: Vec::new(),
            sp_entity_id: None,
            email_attribute: "email".into(),
            name_attribute: "name".into(),
            email_verified: false,
            skew_secs: 60,
            domains: Vec::new(),
            create_on_first_sign_in: false,
            default_role: None,
            link_verified_email: false,
        };
        assert_eq!(
            provider.entity_id("https://fieldnotes.example"),
            "https://fieldnotes.example/federation/saml/metadata/acme"
        );
        provider.sp_entity_id = Some("urn:strata:fieldnotes".into());
        assert_eq!(
            provider.entity_id("https://x.example"),
            "urn:strata:fieldnotes"
        );
    }
}
