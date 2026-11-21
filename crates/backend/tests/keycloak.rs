//! Sign-in at a real OpenID Connect provider: Keycloak, started by `STRATA_IDP=1 node
//! scripts/containers.mjs up` (deploy/compose.local-idp.yaml, realms in deploy/keycloak/realms).
//!
//! The backend runs in this process with the provider the realm files describe. The test is the
//! browser: it starts the sign-in, follows the redirects to Keycloak, posts Keycloak's login form
//! with a cookie jar of its own, takes the code from the redirect back to the site and delivers the
//! callback, as the gateway would. Nothing about Keycloak is simulated.
//!
//! `STRATA_KEYCLOAK_URL` is the address Keycloak is reached at, `http://keycloak.localhost:8180` in
//! the container profile. It is also the address in Keycloak's issuer, because the profile fixes
//! Keycloak's host name: the test must reach it by that name. Browsers send every `*.localhost`
//! name to this machine, but not every system resolver does (Windows does not), so the test and
//! the backend's fetcher both resolve `*.localhost` to the loopback address themselves.

use jsonwebtoken::Algorithm;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_backend::mail::DevSink;
use strata_backend::outbound::gate::Resolve;
use strata_backend::outbound::transport::{HttpTransport, SystemResolver};
use strata_backend::outbound::{Gate, Policy};
use strata_backend::signin::oidc::{Federation, Http, ProviderConfig, Secret};
use strata_kernel::TenantId;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;

const TOKEN: &str = "test-service-token-0123456789abcdef";
/// An address of the site that realm `strata` has registered as a redirect address of the client.
const SITE: &str = "http://127.0.0.1:4100";
const CALLBACK: &str = "http://127.0.0.1:4100/federation/oidc/callback";
const SOURCE: &str = "203.0.113.7";
/// The development value in deploy/keycloak/realms/realm-strata.json.
const CLIENT_SECRET: &str = "strata-dev-keycloak-secret";
/// What every refusal of the method says, whatever was wrong.
const REFUSAL: &str = "The sign-in with the identity provider was not accepted.";

/// Resolves `*.localhost` to the loopback address, as a browser does, and asks the system for
/// every other name.
struct LocalhostResolver;

impl Resolve for LocalhostResolver {
    fn resolve(&self, host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String> {
        if host == "localhost" || host.ends_with(".localhost") {
            Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
        } else {
            SystemResolver.resolve(host, timeout)
        }
    }
}

/// The host and port of `STRATA_KEYCLOAK_URL`, for the test's own client.
fn keycloak_host() -> (String, u16) {
    let url = reqwest::Url::parse(&keycloak_url()).expect("STRATA_KEYCLOAK_URL is a URL");
    (
        url.host_str().unwrap_or_default().to_string(),
        url.port_or_known_default().unwrap_or(80),
    )
}

/// The browser's cookies: names and values, which is all Keycloak's flow needs.
#[derive(Default)]
struct Jar(BTreeMap<String, String>);

impl Jar {
    fn take(&mut self, headers: &reqwest::header::HeaderMap) {
        for line in headers.get_all(reqwest::header::SET_COOKIE) {
            let Ok(line) = line.to_str() else { continue };
            let pair = line.split(';').next().unwrap_or_default();
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let expired = line.to_ascii_lowercase().contains("max-age=0");
            if value.is_empty() || expired {
                self.0.remove(name.trim());
            } else {
                self.0
                    .insert(name.trim().to_string(), value.trim().to_string());
            }
        }
    }

    fn header(&self) -> String {
        self.0
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The address Keycloak is reached at, or a failure that names the variable.
fn keycloak_url() -> String {
    match std::env::var("STRATA_KEYCLOAK_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
        _ => panic!(
            "STRATA_KEYCLOAK_URL is not set. Start Keycloak with `STRATA_IDP=1 node scripts/containers.mjs up` \
             and set STRATA_KEYCLOAK_URL=http://keycloak.localhost:8180"
        ),
    }
}

/// A form as `application/x-www-form-urlencoded` text.
fn form_text(form: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse("https://form.invalid/").unwrap();
    url.query_pairs_mut().extend_pairs(form);
    url.query().unwrap_or("").to_string()
}

/// The text of an HTML attribute value as the page wrote it.
fn unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&#x3d;", "=")
        .replace("&#61;", "=")
        .replace("&quot;", "\"")
}

/// Where Keycloak's login form posts to.
fn login_action(page: &str) -> Option<String> {
    let at = page.find("id=\"kc-form-login\"")?;
    let start = page[..at].rfind("<form")?;
    let end = at + page[at..].find('>')?;
    let tag = &page[start..end];
    let from = tag.find("action=\"")? + "action=\"".len();
    let length = tag[from..].find('"')?;
    Some(unescape(&tag[from..from + length]))
}

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start_backend(provider: ProviderConfig) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sink = Arc::new(DevSink::memory());
    let federation = Federation::new(
        BTreeMap::from([(TenantId::new("fieldnotes").unwrap(), vec![provider])]),
        // Keycloak is on this machine over plain HTTP: the development exception (FED-2644).
        Arc::new(Http::with_gate(Gate::new(
            Arc::new(LocalhostResolver),
            Arc::new(HttpTransport),
            Policy {
                development: true,
                total_timeout: Duration::from_secs(10),
                max_bytes: 256 * 1024,
                max_redirects: 0,
                ..Policy::default()
            },
        ))),
    );
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: sink,
        federation: Arc::new(federation),
        factors: Default::default(),
        limits: Default::default(),
    };
    tokio::spawn(strata_backend::run(
        cfg,
        Arc::new(MemoryStore::new()),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
    };
    for _ in 0..200 {
        if backend
            .http
            .get(format!("{}/internal/health", backend.base))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return backend;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("backend did not start");
}

impl Backend {
    /// A call as the gateway makes it for a browser at the site.
    async fn call(&self, path: &str, body: Json) -> (u16, Json) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, "fieldnotes")
            .header(HEADER_SOURCE, SOURCE)
            .header(HEADER_CORRELATION, "t-corr-keycloak")
            .header(HEADER_ORIGIN, SITE)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }
}

/// What Keycloak sent the browser back to the site with.
struct Answer {
    state: String,
    code: String,
}

/// Go to `start` as a browser and sign in at Keycloak's login page: follow redirects, post the
/// login form with the cookies Keycloak set, and stop at the redirect that leaves for the site.
async fn browse(start: &str, user: &str, password: &str) -> Answer {
    let (host, port) = keycloak_host();
    let browser = reqwest::Client::builder()
        .resolve(
            &host,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        )
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let mut jar = Jar::default();
    let mut url = reqwest::Url::parse(start).unwrap();
    let mut post: Option<String> = None;
    for _ in 0..16 {
        let mut request = match post.take() {
            Some(body) => browser
                .post(url.clone())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body),
            None => browser.get(url.clone()),
        };
        if !jar.0.is_empty() {
            request = request.header("cookie", jar.header());
        }
        let response = request.send().await.unwrap_or_else(|e| {
            panic!("{url}: {e}. Is Keycloak running, and does STRATA_KEYCLOAK_URL name it by the host name of its issuer?")
        });
        jar.take(response.headers());
        let status = response.status();
        if status.is_redirection() {
            let location = response.headers()["location"].to_str().unwrap().to_string();
            let next = url.join(&location).unwrap();
            if next.as_str().starts_with(CALLBACK) {
                let find = |name: &str| {
                    next.query_pairs()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.into_owned())
                };
                if let Some(error) = find("error") {
                    panic!("Keycloak sent the browser back with error={error}: {next}");
                }
                return Answer {
                    state: find("state").expect("the redirect back carries a state"),
                    code: find("code").expect("the redirect back carries a code"),
                };
            }
            url = next;
            continue;
        }
        let page = response.text().await.unwrap();
        assert!(status.is_success(), "{url}: {status}");
        let Some(action) = login_action(&page) else {
            let head: String = page.chars().take(600).collect();
            panic!("{url}: expected Keycloak's login form, found: {head}");
        };
        url = url.join(&action).unwrap();
        post = Some(form_text(&[
            ("username", user),
            ("password", password),
            ("credentialId", ""),
        ]));
    }
    panic!("Keycloak did not send the browser back to the site");
}

#[tokio::test]
async fn a_person_signs_in_through_keycloak_and_the_callback_answers_once() {
    let keycloak = keycloak_url();
    let backend = start_backend(ProviderConfig {
        id: "keycloak".into(),
        label: "Keycloak OIDC".into(),
        enabled: true,
        issuer: format!("{keycloak}/realms/strata"),
        client_id: "strata".into(),
        client_secret: Some(Secret::new(CLIENT_SECRET)),
        scopes: vec!["openid".into(), "email".into(), "profile".into()],
        algorithms: vec![Algorithm::RS256],
        domains: vec!["keycloak.example".into()],
        create_on_first_sign_in: true,
        default_role: Some("author".into()),
        link_verified_email: false,
    })
    .await;

    // The start: the sign-in page's request. The reply is Keycloak's authorisation address, with the
    // state, the nonce and the proof key's challenge, and the binding nonce for the browser's cookie.
    let (status, started) = backend
        .call(
            "/internal/sign-in/oidc/start",
            json!({ "provider": "keycloak" }),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let binding = started["binding"].as_str().unwrap().to_string();
    let redirect = started["redirect"].as_str().unwrap();
    assert!(
        redirect.starts_with(&format!(
            "{keycloak}/realms/strata/protocol/openid-connect/auth?"
        )),
        "{redirect}"
    );
    assert!(
        redirect.contains("code_challenge_method=S256"),
        "{redirect}"
    );

    // The browser's part: Keycloak's login page, and the redirect back to the site.
    let answer = browse(redirect, "olive", "strata-dev-olive").await;
    let callback = json!({ "state": answer.state, "code": answer.code, "binding": binding });

    // The callback: a session for a principal the site made for this provider's subject.
    let (status, signed_in) = backend
        .call("/internal/session/sign-in/oidc", callback.clone())
        .await;
    assert_eq!(status, 200, "{signed_in}");
    let principal = signed_in["session"]["principal"].as_str().unwrap();
    assert!(principal.starts_with("user:oidc-"), "{principal}");
    let roles: Vec<&str> = signed_in["session"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Json::as_str)
        .collect();
    // The default role of a first sign-in, beside the public role every session holds.
    assert_eq!(roles, ["author", "public"], "{signed_in}");
    assert_eq!(
        signed_in["session"]["display_name"], "Olive Oidc",
        "{signed_in}"
    );
    assert!(
        signed_in["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "{signed_in}"
    );

    // A callback that was answered once is not answered again.
    let (status, again) = backend
        .call("/internal/session/sign-in/oidc", callback)
        .await;
    assert_eq!(status, 401, "{again}");
    assert_eq!(again["message"], REFUSAL);
}
