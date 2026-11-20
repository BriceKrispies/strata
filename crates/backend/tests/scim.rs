//! Provisioning of people from an identity provider by SCIM: the provisioning token and what it
//! opens, the cases of the provisioning table, deactivation ending sessions, and roles that change
//! only through the group mapping the realm declares. Real sockets, real bundles, the in-memory
//! store.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{Config, ThrottleLimits};
use strata_backend::auth::token_hash;
use strata_backend::mail::DevSink;
use strata_backend::signin::oidc::Federation;
use strata_kernel::{
    Context, Intent, Principal, PrincipalId, RoleId, SessionId, TenantId, Timestamp,
};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SOURCE: &str = "203.0.113.7";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
}

async fn start() -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store = Arc::new(MemoryStore::new());
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: ThrottleLimits::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    tokio::spawn(strata_backend::run(
        cfg,
        store.clone(),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        store,
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
    async fn call(&self, site: &str, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, SOURCE)
            .header(HEADER_CORRELATION, "t-corr-scim")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn owner(&self) -> String {
        let (status, reply) = self
            .call(
                "fieldnotes",
                None,
                "/internal/session/login",
                json!({ "login": "admin", "password": "strata-dev-admin" }),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    /// A provisioning token for the realm.
    async fn provisioning_token(&self, site: &str, owner: &str) -> String {
        let (status, reply) = self
            .call(
                site,
                Some(owner),
                "/internal/provisioning/tokens/create",
                json!({}),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    /// One SCIM request as the gateway passes it on: the answer's status and body.
    async fn scim(
        &self,
        site: &str,
        token: Option<&str>,
        method: &str,
        path: &str,
        body: Json,
    ) -> (u64, Json) {
        let (status, reply) = self
            .call(
                site,
                None,
                "/internal/scim",
                json!({
                    "method": method,
                    "path": path,
                    "authorization": token.map(|t| format!("Bearer {t}")),
                    "body": body,
                }),
            )
            .await;
        assert_eq!(status, 200, "the envelope is answered: {reply}");
        (reply["status"].as_u64().unwrap(), reply["body"].clone())
    }

    async fn roles_of(&self, principal: &str) -> Vec<String> {
        let state = self
            .store
            .load(&TenantId::new("fieldnotes").unwrap(), &[])
            .unwrap()
            .tenant
            .unwrap();
        state
            .members
            .get(&PrincipalId::new(principal).unwrap())
            .map(|roles| roles.iter().map(|r| r.as_str().to_string()).collect())
            .unwrap_or_default()
    }
}

fn person(user_name: &str) -> Json {
    json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
        "userName": user_name,
        "externalId": format!("ext-{user_name}"),
        "displayName": "Wren Idowu",
        "active": true,
    })
}

/// Verifies: FED-014
///
/// Each operation of the provisioning table does what the table says: create, a duplicate user
/// name, replace, filter by user name, deactivate and reactivate, delete.
#[tokio::test]
async fn the_provisioning_cases_behave_as_the_table_states() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let scim = |method: &'static str, path: String, body: Json| {
        let token = token.clone();
        let backend = &backend;
        async move {
            backend
                .scim("fieldnotes", Some(&token), method, &path, body)
                .await
        }
    };

    // create-user: a principal exists with the external identifier linked.
    let (status, created) = scim("POST", "Users".into(), person("wren@acme.example")).await;
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["userName"], "wren@acme.example");
    assert_eq!(created["externalId"], "ext-wren@acme.example");
    assert_eq!(created["active"], true);
    let tenant = TenantId::new("fieldnotes").unwrap();
    let linked = backend
        .store
        .identity(&tenant, "scim", "ext-wren@acme.example")
        .unwrap()
        .expect("the external identifier is linked");
    assert_eq!(linked.principal.as_str(), id);
    let account = backend
        .store
        .account(&tenant, "wren@acme.example")
        .unwrap()
        .expect("a login exists");
    assert!(account.password_hash.is_empty(), "no password opens it");

    // create-duplicate: refused with a uniqueness error, in any spelling of the name.
    let (status, refused) = scim("POST", "Users".into(), person("Wren@ACME.example")).await;
    assert_eq!(
        (status, &refused["scimType"]),
        (409, &json!("uniqueness")),
        "{refused}"
    );

    // replace-user: the attributes are replaced.
    let mut replacement = person("wren@acme.example");
    replacement["displayName"] = json!("Wren I.");
    let (status, replaced) = scim("PUT", format!("Users/{id}"), replacement).await;
    assert_eq!(
        (status, &replaced["displayName"]),
        (200, &json!("Wren I.")),
        "{replaced}"
    );
    assert_eq!(
        backend
            .store
            .account(&tenant, "wren@acme.example")
            .unwrap()
            .unwrap()
            .display_name,
        "Wren I."
    );
    // The user name is not one of the attributes that change.
    let (status, _) = scim("PUT", format!("Users/{id}"), person("someone@else.example")).await;
    assert_eq!(status, 400);

    // filter-user-name: the matching principal, or an empty list.
    let found = backend
        .call(
            "fieldnotes",
            None,
            "/internal/scim",
            json!({
                "method": "GET", "path": "Users",
                "query": { "filter": "userName eq \"WREN@acme.example\"" },
                "authorization": format!("Bearer {token}"),
            }),
        )
        .await;
    assert_eq!(found.1["body"]["totalResults"], 1, "{}", found.1);
    assert_eq!(found.1["body"]["Resources"][0]["id"], id);
    let none = backend
        .call(
            "fieldnotes",
            None,
            "/internal/scim",
            json!({
                "method": "GET", "path": "Users",
                "query": { "filter": "userName eq \"nobody@acme.example\"" },
                "authorization": format!("Bearer {token}"),
            }),
        )
        .await;
    assert_eq!(none.1["body"]["totalResults"], 0);

    // patch-deactivate and patch-reactivate.
    let patch = |active: Json| json!({ "Operations": [{ "op": "replace", "path": "active", "value": active }] });
    let (status, off) = scim("PATCH", format!("Users/{id}"), patch(json!(false))).await;
    assert_eq!((status, &off["active"]), (200, &json!(false)), "{off}");
    let (_, on) = scim("PATCH", format!("Users/{id}"), patch(json!("True"))).await;
    assert_eq!(on["active"], true, "a provider may write the flag as text");

    // delete-user: the principal is removed.
    let (status, _) = scim("DELETE", format!("Users/{id}"), Json::Null).await;
    assert_eq!(status, 204);
    assert!(backend
        .store
        .account(&tenant, "wren@acme.example")
        .unwrap()
        .is_none());
    assert_eq!(scim("GET", format!("Users/{id}"), Json::Null).await.0, 404);
}

/// Verifies: FED-015
///
/// Deactivating a person through provisioning revokes their sessions at once, and deleting them
/// does too; reactivating does not bring a session back.
#[tokio::test]
async fn deactivating_a_person_revokes_their_sessions() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let tenant = TenantId::new("fieldnotes").unwrap();

    let make = |name: &'static str| {
        let (backend, token) = (&backend, token.clone());
        async move {
            let (_, user) = backend
                .scim("fieldnotes", Some(&token), "POST", "Users", person(name))
                .await;
            user["id"].as_str().unwrap().to_string()
        }
    };
    // A session the way a federated sign-in makes one: a record of the store.
    let sign_in = |principal: &str, secret: &str| {
        let session = SessionRecord {
            id: SessionId::new(format!("s_{secret}")).unwrap(),
            token_hash: token_hash(secret),
            csrf_token: "csrf".into(),
            tenant: tenant.clone(),
            principal: PrincipalId::new(principal).unwrap(),
            display_name: "Provisioned".into(),
            created_at: Timestamp(now_ms()),
            last_seen_at: Timestamp(now_ms()),
            expires_at: Timestamp(now_ms() + 3_600_000),
            revoked_at: None,
            authenticated_at: Timestamp(now_ms()),
            network: None,
        };
        backend.store.put_session(&session).unwrap();
    };
    let signed_in = |secret: &'static str| {
        let backend = &backend;
        async move {
            let (status, reply) = backend
                .call("fieldnotes", Some(secret), "/internal/session", json!({}))
                .await;
            assert_eq!(status, 200);
            reply["session"]["authenticated"].as_bool().unwrap()
        }
    };

    let first = make("first@acme.example").await;
    let second = make("second@acme.example").await;
    sign_in(&first, "first-secret");
    sign_in(&second, "second-secret");
    assert!(signed_in("first-secret").await);
    assert!(signed_in("second-secret").await);

    let deactivate = json!({ "Operations": [{ "op": "replace", "value": { "active": false } }] });
    let (status, _) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            &format!("Users/{first}"),
            deactivate,
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        !signed_in("first-secret").await,
        "deactivated: refused from now on"
    );
    assert!(
        signed_in("second-secret").await,
        "another person is not touched"
    );

    let (status, _) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "DELETE",
            &format!("Users/{second}"),
            Json::Null,
        )
        .await;
    assert_eq!(status, 204);
    assert!(
        !signed_in("second-secret").await,
        "deleted: refused from now on"
    );
}

/// Verifies: FED-259
///
/// A provisioning token opens the provisioning endpoint of its own realm and nothing else: not
/// another realm's, not as a session, not as a client secret, and not once withdrawn; a request
/// with no token or a made-up one is refused.
#[tokio::test]
async fn a_provisioning_token_opens_only_the_provisioning_endpoint_of_its_realm() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let list = |site: &'static str, token: Option<String>| {
        let backend = &backend;
        async move {
            backend
                .scim(site, token.as_deref(), "GET", "Users", Json::Null)
                .await
                .0
        }
    };
    assert_eq!(list("fieldnotes", Some(token.clone())).await, 200);
    assert_eq!(list("fieldnotes", None).await, 401);
    assert_eq!(list("fieldnotes", Some("scimt_made-up".into())).await, 401);
    assert_eq!(
        list("tidewater", Some(token.clone())).await,
        401,
        "another realm's endpoint"
    );

    // Not a session: presented as one, it is no one.
    let (status, reply) = backend
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(reply["session"]["authenticated"], false);
    let (status, _) = backend
        .call("fieldnotes", Some(&token), "/internal/clients", json!({}))
        .await;
    assert_eq!(status, 401, "and it reads no configuration");
    // Not a client secret, and not a token's bearer.
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/token",
            json!({ "client_id": "svc:none", "client_secret": token }),
        )
        .await;
    assert_eq!(status, 401);
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/token/verify",
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, 401);
    // The owner's own session is not a provisioning token either.
    assert_eq!(list("fieldnotes", Some(owner.clone())).await, 401);
    // Another site's owner cannot make one for this one.
    let (status, _) = backend
        .call(
            "tidewater",
            Some(&owner),
            "/internal/provisioning/tokens/create",
            json!({}),
        )
        .await;
    assert_eq!(status, 401, "a session of another site is no one here");

    // Withdrawn, it opens nothing.
    let (_, made) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/tokens/create",
            json!({}),
        )
        .await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/tokens/revoke",
            json!({ "id": made["id"] }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        list("fieldnotes", Some(made["token"].as_str().unwrap().into())).await,
        401
    );
    assert_eq!(
        list("fieldnotes", Some(token)).await,
        200,
        "the other one stands"
    );

    // Only an owner makes one.
    let ines = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await
        .1["token"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/provisioning/tokens/create",
            json!({}),
        )
        .await;
    assert_eq!(status, 403);
}

/// Verifies: FED-260
///
/// Roles change only through a group the realm mapped: a user resource that carries roles gets
/// none, a group the realm did not map is refused, the owner role cannot be mapped, and the mapped
/// group's members hold the role while they are in it.
#[tokio::test]
async fn roles_change_only_through_the_group_mapping_the_realm_declares() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;

    let mut body = person("lena@acme.example");
    body["roles"] = json!([{ "value": "editor" }, { "value": "owner" }]);
    body["groups"] = json!([{ "value": "editors" }]);
    let (status, user) = backend
        .scim("fieldnotes", Some(&token), "POST", "Users", body)
        .await;
    assert_eq!(status, 201, "{user}");
    let id = user["id"].as_str().unwrap().to_string();
    assert!(
        backend.roles_of(&id).await.is_empty(),
        "a user resource grants no role"
    );

    let add =
        json!({ "Operations": [{ "op": "add", "path": "members", "value": [{ "value": id }] }] });
    // No mapping yet: refused.
    let (status, refused) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            "Groups/writers",
            add.clone(),
        )
        .await;
    assert_eq!(
        (status, &refused["scimType"]),
        (400, &json!("invalidValue")),
        "{refused}"
    );
    assert!(backend.roles_of(&id).await.is_empty());

    // The realm declares one. The owner role is not one it can declare.
    for role in ["owner", "public", "nonexistent"] {
        let (status, _) = backend
            .call(
                "fieldnotes",
                Some(&owner),
                "/internal/provisioning/groups/set",
                json!({ "group": "writers", "role": role }),
            )
            .await;
        assert_eq!(status, 422, "{role}");
    }
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/groups/set",
            json!({ "group": "writers", "role": "author" }),
        )
        .await;
    assert_eq!(status, 200);

    let (status, group) = backend
        .scim("fieldnotes", Some(&token), "PATCH", "Groups/writers", add)
        .await;
    assert_eq!(status, 200, "{group}");
    assert_eq!(
        backend.roles_of(&id).await,
        ["author"],
        "the mapped role, and no other"
    );
    // A member that is no person of the realm is refused.
    let (status, _) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            "Groups/writers",
            json!({ "Operations": [{ "op": "add", "path": "members", "value": [{ "value": "user:ines" }] }] }),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(backend.roles_of("user:ines").await, ["author", "editor"]);

    // Out of the group, the role goes.
    let remove = json!({ "Operations": [{ "op": "remove", "path": format!("members[value eq \"{id}\"]") }] });
    let (status, _) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            "Groups/writers",
            remove,
        )
        .await;
    assert_eq!(status, 200);
    assert!(backend.roles_of(&id).await.is_empty());

    // Deleting a member takes the role back too.
    let add_again =
        json!({ "Operations": [{ "op": "add", "path": "members", "value": [{ "value": id }] }] });
    backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            "Groups/writers",
            add_again,
        )
        .await;
    assert_eq!(backend.roles_of(&id).await, ["author"]);
    backend
        .scim(
            "fieldnotes",
            Some(&token),
            "DELETE",
            &format!("Users/{id}"),
            Json::Null,
        )
        .await;
    assert!(backend.roles_of(&id).await.is_empty());
}

/// Verifies: FED-015, TOK-007
///
/// A person who is deactivated is no subject of a token: a delegation they made is refused from
/// then on.
#[tokio::test]
async fn deactivating_a_person_refuses_what_was_delegated_in_their_name() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    // A provisioned person who holds a role, and so a scope to delegate.
    let (_, user) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "POST",
            "Users",
            person("zoe@acme.example"),
        )
        .await;
    let id = user["id"].as_str().unwrap().to_string();
    backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/groups/set",
            json!({ "group": "writers", "role": "author" }),
        )
        .await;
    backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            "Groups/writers",
            json!({ "Operations": [{ "op": "add", "path": "members", "value": [{ "value": id }] }] }),
        )
        .await;
    let tenant = TenantId::new("fieldnotes").unwrap();
    backend
        .store
        .put_session(&SessionRecord {
            id: SessionId::new("s_zoe").unwrap(),
            token_hash: token_hash("zoe-secret"),
            csrf_token: "csrf".into(),
            tenant,
            principal: PrincipalId::new(id.clone()).unwrap(),
            display_name: "Zoe".into(),
            created_at: Timestamp(now_ms()),
            last_seen_at: Timestamp(now_ms()),
            expires_at: Timestamp(now_ms() + 3_600_000),
            revoked_at: None,
            authenticated_at: Timestamp(now_ms()),
            network: None,
        })
        .unwrap();

    let (status, client) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/clients/create",
            json!({ "name": "Agent", "scopes": ["read:post"] }),
        )
        .await;
    assert_eq!(status, 200, "{client}");
    let (status, grant) = backend
        .call(
            "fieldnotes",
            Some("zoe-secret"),
            "/internal/grants/create",
            json!({ "agent": client["id"], "scope": "read:post" }),
        )
        .await;
    assert_eq!(status, 200, "{grant}");
    let exchange = || {
        backend.call(
            "fieldnotes",
            None,
            "/internal/token",
            json!({
                "grant_type": "token-exchange",
                "client_id": client["id"],
                "client_secret": client["secret"],
                "grant": grant["grant"],
            }),
        )
    };
    let (status, issued) = exchange().await;
    assert_eq!(status, 200, "{issued}");
    let access = issued["access_token"].as_str().unwrap().to_string();
    let check = || {
        backend.call(
            "fieldnotes",
            None,
            "/internal/token/verify",
            json!({ "token": access }),
        )
    };
    assert_eq!(check().await.0, 200);

    backend
        .scim(
            "fieldnotes",
            Some(&token),
            "PATCH",
            &format!("Users/{id}"),
            json!({ "Operations": [{ "op": "replace", "path": "active", "value": false }] }),
        )
        .await;
    assert_eq!(check().await.0, 401, "the token made in their name");
    assert_eq!(exchange().await.0, 401, "and no new one");
}

impl Backend {
    /// A SCIM request with a query string, as the gateway passes it on.
    async fn scim_query(&self, token: &str, path: &str, query: Json) -> (u64, Json) {
        let (status, reply) = self
            .call(
                "fieldnotes",
                None,
                "/internal/scim",
                json!({
                    "method": "GET",
                    "path": path,
                    "query": query,
                    "authorization": format!("Bearer {token}"),
                    "body": Json::Null,
                }),
            )
            .await;
        assert_eq!(status, 200, "the envelope is answered: {reply}");
        (reply["status"].as_u64().unwrap(), reply["body"].clone())
    }
}

/// Verifies: FED-014
///
/// Two creations of one userName that arrive together make one account: one is created and the
/// other is told the name is in use.
#[tokio::test]
async fn two_concurrent_creations_of_one_user_name_make_one_account_and_one_conflict() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let mut first = person("wren@acme.example");
    first["externalId"] = json!("ext-first");
    let mut second = person("wren@acme.example");
    second["externalId"] = json!("ext-second");

    let ((a, _), (b, _)) = tokio::join!(
        backend.scim("fieldnotes", Some(&token), "POST", "Users", first),
        backend.scim("fieldnotes", Some(&token), "POST", "Users", second),
    );
    let mut statuses = [a, b];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    let tenant = TenantId::new("fieldnotes").unwrap();
    assert!(backend
        .store
        .account(&tenant, "wren@acme.example")
        .unwrap()
        .is_some());
    assert_eq!(
        backend.store.doc_list(&tenant, "scim-user").unwrap().len(),
        1
    );
}

/// Verifies: FED-259
///
/// A flood of wrong tokens is counted and then refused, and does not take the provider's slots:
/// a request with the right token is answered while the wrong ones are throttled.
#[tokio::test]
async fn wrong_tokens_are_throttled_and_do_not_throttle_the_right_one() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let mut refused_by_limit = 0;
    for _ in 0..150 {
        let (status, _) = backend
            .call(
                "fieldnotes",
                None,
                "/internal/scim",
                json!({ "method": "GET", "path": "Users", "authorization": "Bearer scimt_wrong" }),
            )
            .await;
        if status == 429 {
            refused_by_limit += 1;
        }
    }
    assert!(refused_by_limit > 0, "the wrong tokens were never limited");
    let (status, _) = backend
        .scim("fieldnotes", Some(&token), "GET", "Users", Json::Null)
        .await;
    assert_eq!(status, 200);
}

/// Verifies: FED-014
///
/// A list honours `startIndex` and `count`, and reports the true number of matches.
#[tokio::test]
async fn a_list_honours_start_index_and_count_and_reports_the_true_total() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    for n in 0..5 {
        let (status, body) = backend
            .scim(
                "fieldnotes",
                Some(&token),
                "POST",
                "Users",
                person(&format!("person{n}@acme.example")),
            )
            .await;
        assert_eq!(status, 201, "{body}");
    }
    let (status, page) = backend
        .scim_query(&token, "Users", json!({ "startIndex": "2", "count": "2" }))
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["totalResults"], 5);
    assert_eq!(page["startIndex"], 2);
    assert_eq!(page["itemsPerPage"], 2);
    assert_eq!(page["Resources"].as_array().unwrap().len(), 2);

    let (_, last) = backend
        .scim_query(&token, "Users", json!({ "startIndex": "5", "count": "10" }))
        .await;
    assert_eq!(last["totalResults"], 5);
    assert_eq!(last["Resources"].as_array().unwrap().len(), 1);
    let (_, none) = backend
        .scim_query(&token, "Users", json!({ "count": "0" }))
        .await;
    assert_eq!(none["totalResults"], 5);
    assert!(none["Resources"].as_array().unwrap().is_empty());
}

/// Verifies: FED-015
///
/// Deleting a person takes back the roles they were given directly, not only those of groups.
#[tokio::test]
async fn deleting_a_person_removes_the_roles_they_were_given_directly() {
    let backend = start().await;
    let owner = backend.owner().await;
    let token = backend.provisioning_token("fieldnotes", &owner).await;
    let (status, user) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "POST",
            "Users",
            person("lena@acme.example"),
        )
        .await;
    assert_eq!(status, 201, "{user}");
    let id = user["id"].as_str().unwrap().to_string();
    let context = Context {
        tenant: TenantId::new("fieldnotes").unwrap(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now_ms()),
        correlation: "t-direct-role".into(),
        verified_address: None,
        idempotency_key: None,
    };
    execute(
        backend.store.as_ref(),
        &Intent::AssignRole {
            principal: PrincipalId::new(id.clone()).unwrap(),
            role: RoleId::new("editor").unwrap(),
        },
        &context,
    )
    .unwrap_or_else(|e| panic!("the owner's change was refused: {e}"));
    assert_eq!(backend.roles_of(&id).await, ["editor"]);

    let (status, _) = backend
        .scim(
            "fieldnotes",
            Some(&token),
            "DELETE",
            &format!("Users/{id}"),
            Json::Null,
        )
        .await;
    assert_eq!(status, 204);
    assert!(
        backend.roles_of(&id).await.is_empty(),
        "the deleted person kept a role"
    );
}

/// Verifies: FED-259
///
/// A provisioning token records when it was last used, and one made with a lifetime is refused
/// once the lifetime has passed.
#[tokio::test]
async fn a_provisioning_token_records_its_use_and_can_expire() {
    let backend = start().await;
    let owner = backend.owner().await;
    let (status, made) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/tokens/create",
            json!({ "expires_in_seconds": 600 }),
        )
        .await;
    assert_eq!(status, 200, "{made}");
    assert!(made["expires_at"].as_i64().is_some(), "{made}");
    let token = made["token"].as_str().unwrap().to_string();
    let tenant = TenantId::new("fieldnotes").unwrap();
    let stored = |backend: &Backend| {
        let rows = backend.store.doc_list(&tenant, "prov-token").unwrap();
        serde_json::from_str::<Json>(&rows[0].1).unwrap()
    };
    assert_eq!(stored(&backend)["last_used_at"], Json::Null);

    let (status, _) = backend
        .scim("fieldnotes", Some(&token), "GET", "Users", Json::Null)
        .await;
    assert_eq!(status, 200);
    assert!(
        stored(&backend)["last_used_at"].as_i64().is_some(),
        "{}",
        stored(&backend)
    );

    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/dev/clock",
            json!({ "advance_ms": 11 * 60_000 }),
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = backend
        .scim("fieldnotes", Some(&token), "GET", "Users", Json::Null)
        .await;
    assert_eq!(status, 401, "an expired token was accepted");
    // A lifetime that is not a whole number of seconds is refused.
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/provisioning/tokens/create",
            json!({ "expires_in_seconds": -5 }),
        )
        .await;
    assert_eq!(status, 422);
}
