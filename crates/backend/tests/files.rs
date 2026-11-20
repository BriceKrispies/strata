//! The file store through the backend: intake (type sniffing and size caps), chunked uploads,
//! scanning, readability that follows the record and its field rule, signed addresses, delivery,
//! the purge of unreferenced files, erasure and tenant isolation. Each test runs on its own
//! tenants and its own file root.

use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{ApiError, App, Config};
use strata_backend::files::address::{sign_file_address, sign_media_address, Address, SignRequest};
use strata_backend::files::deliver::{deliver_file, Delivery, DeliveryRequest};
use strata_backend::files::scan::eicar;
use strata_backend::files::storage;
use strata_backend::files::upload::{
    finish_upload, put_upload_chunk, start_upload, upload_file, UploadRequest,
};
use strata_backend::files::{
    purge_tenant, FileState, CHUNK_BYTES, FILE_KIND, PLATFORM_MAX_BYTES, SINGLE_MAX_BYTES,
    UPLOAD_KIND,
};
use strata_backend::mail::DevSink;
use strata_backend::ops;
use strata_backend::worker::{self, WorkerConfig};
use strata_kernel::ext::TenantSchemaExt;
use strata_kernel::{
    Action, Context, DataSubject, DeltaOp, ExistingData, Expr, FieldDef, FieldId, FieldType,
    Intent, ObjectTypeDef, ObjectTypeId, Output, PersonalDeclaration, PersonalKind, PolicyDef,
    PolicyId, Principal, PrincipalId, RecordId, Resource, RoleDef, RoleId, SchemaChange,
    StateDelta, StateRevision, TenantId, TenantOperationDef, TenantOperationId,
    TenantOperationInput, TenantOperationInvokers, TenantOperationStep, TenantSchema, Timestamp,
    Transition, Value,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Commit, Store};
use strata_storage_sqlite::SqliteStore;

/// The time every test starts at.
const T0: i64 = 1_800_000_000_000;

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant id")
}

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
}

fn role(name: &str) -> RoleId {
    RoleId::new(name).expect("role id")
}

/// An application on `store` whose files live in a directory of this test alone.
fn app_on(store: Arc<dyn Store>, test: &str) -> Arc<App> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().expect("address"),
        service_token: "test-service-token-0123456789abcdef".into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    let app = App::new(cfg, store);
    let files =
        std::env::temp_dir().join(format!("strata-files-{}-{test}-{}", std::process::id(), T0));
    let _ = std::fs::remove_dir_all(&files);
    assert!(app.files.set_root(files), "the file root is set once");
    app
}

fn memory_app(test: &str) -> Arc<App> {
    app_on(Arc::new(MemoryStore::new()), test)
}

fn ctx(site: &TenantId, principal: Principal, now: i64) -> Context {
    Context {
        tenant: site.clone(),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: "t-files-1".to_string(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// The context of `<tenant>:<name>`, signed in.
fn as_user(site: &TenantId, name: &str, now: i64) -> Context {
    ctx(
        site,
        Principal::User(who(&format!("{}:{name}", site.as_str()))),
        now,
    )
}

fn as_owner(site: &TenantId, now: i64) -> Context {
    as_user(site, "owner", now)
}

fn file_type(media_types: &[&str], max_bytes: u64) -> FieldType {
    FieldType::File {
        media_types: media_types.iter().map(|t| t.to_string()).collect(),
        max_bytes,
        renditions: Vec::new(),
    }
}

fn field(id: &str, ty: FieldType, read: Option<Expr>, personal: Option<DataSubject>) -> FieldDef {
    let id = FieldId::new(id).expect("field id");
    FieldDef {
        id: id.clone(),
        label: id.as_str().to_string(),
        ty,
        required: false,
        unique: false,
        default: None,
        read,
        personal: personal.map(|subject| PersonalDeclaration {
            kind: PersonalKind::Other,
            subject,
            searchable: false,
        }),
    }
}

fn record_type(id: &str, fields: Vec<FieldDef>) -> ObjectTypeDef {
    ObjectTypeDef {
        searchable: None,
        id: ObjectTypeId::new(id).expect("object id"),
        label: id.to_string(),
        fields: fields.into_iter().map(|f| (f.id.clone(), f)).collect(),
        constraints: Default::default(),
        retention: None,
        publication: None,
        language_field: None,
        translation_group: None,
        unique: None,
    }
}

fn read_policy(schema: &mut TenantSchema, who: &str, object: &str) {
    let id = PolicyId::new(format!("{who}_reads_{object}")).expect("policy id");
    schema.policies.insert(
        id.clone(),
        PolicyDef {
            id,
            role: role(who),
            resource: Resource::Object(ObjectTypeId::new(object).expect("object id")),
            actions: BTreeSet::from([Action::Read]),
            condition: None,
        },
    );
}

/// A tenant with:
/// - `photo`: `title`, `image` (any image, at most 2000000 bytes), `secret` (any image, read only
///   by an editor), `doc` (PDF, at most 1000 bytes); readers and editors read photos;
/// - `profile`: `person` (the subject) and `avatar` (an image, personal to the person);
/// - the principals `<name>:owner`, `<name>:reader` (role reader), `<name>:editor` (role
///   editor) and `<name>:outsider` (no role).
fn found(store: &dyn Store, name: &str) -> TenantId {
    let site = tenant(name);
    execute(
        store,
        &Intent::CreateTenant {
            name: name.to_string(),
            owner: who(&format!("{name}:owner")),
        },
        &ctx(&site, Principal::System, T0 - 10_000),
    )
    .expect("the tenant is created");
    let state = store
        .load_complete(&site)
        .expect("load")
        .tenant
        .expect("the tenant exists");
    let mut schema = TenantSchema::clone(&state.schema);
    let images = || file_type(&["image/*"], 2_000_000);
    schema.objects.insert(
        ObjectTypeId::new("photo").expect("object id"),
        record_type(
            "photo",
            vec![
                field("title", FieldType::Str, None, None),
                field("image", images(), None, None),
                field(
                    "secret",
                    images(),
                    Some(Expr::HasRole(role("editor"))),
                    None,
                ),
                field("doc", file_type(&["application/pdf"], 1000), None, None),
            ],
        ),
    );
    schema.objects.insert(
        ObjectTypeId::new("profile").expect("object id"),
        record_type(
            "profile",
            vec![
                field("person", FieldType::Str, None, None),
                field(
                    "avatar",
                    images(),
                    None,
                    Some(DataSubject::Field {
                        field: FieldId::new("person").expect("field id"),
                    }),
                ),
            ],
        ),
    );
    for name in ["reader", "editor"] {
        schema.roles.insert(
            role(name),
            RoleDef {
                id: role(name),
                label: name.to_string(),
            },
        );
        read_policy(&mut schema, name, "photo");
    }
    // The writer may create and update photos and read none.
    schema.roles.insert(
        role("writer"),
        RoleDef {
            id: role("writer"),
            label: "writer".to_string(),
        },
    );
    let writes = PolicyId::new("writer_writes_photo").expect("policy id");
    schema.policies.insert(
        writes.clone(),
        PolicyDef {
            id: writes,
            role: role("writer"),
            resource: Resource::Object(ObjectTypeId::new("photo").expect("object id")),
            actions: BTreeSet::from([Action::Create, Action::Update]),
            condition: None,
        },
    );
    assert!(schema.problems().is_empty(), "{:?}", schema.problems());
    let delta = StateDelta {
        tenant: site.clone(),
        base_revision: Some(state.revision),
        new_revision: StateRevision(state.revision.0 + 1),
        next_record: state.next_record,
        event_seq: state.event_seq,
        ops: vec![DeltaOp::PutSchema(Arc::new(schema))],
    };
    let transition = Transition {
        delta,
        events: Vec::new(),
        effects: Vec::new(),
        evidence: Vec::new(),
        output: Output::Member,
    };
    let intent = Intent::ChangeSchema {
        changes: Vec::new(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    store
        .commit(&Commit::of(
            &intent,
            &ctx(&site, Principal::System, T0 - 9_000),
            &transition,
        ))
        .expect("the schema is committed");
    for name in ["reader", "editor", "writer"] {
        assign(store, &site, name, name);
    }
    site
}

fn assign(store: &dyn Store, site: &TenantId, person: &str, to: &str) {
    execute(
        store,
        &Intent::AssignRole {
            principal: who(&format!("{}:{person}", site.as_str())),
            role: role(to),
        },
        &as_owner(site, T0 - 8_000),
    )
    .expect("the role is assigned");
}

/// A PNG of the given size whose content ends with `tail`, so each test's files differ.
fn png(width: u32, height: u32, tail: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(tail);
    out
}

fn image_request() -> UploadRequest {
    UploadRequest {
        declared_type: "image/png".into(),
        name: "holiday <b>photo</b>.png".into(),
        ..UploadRequest::default()
    }
}

/// Upload a PNG as the owner and return its file id.
fn upload_png(app: &App, site: &TenantId, tail: &[u8]) -> String {
    upload_file(
        app,
        &as_owner(site, T0),
        &image_request(),
        &png(64, 32, tail),
    )
    .expect("the upload is accepted")
    .file
}

/// Run every due job, as the worker does.
fn run_jobs(app: &App, at: i64) {
    let config = WorkerConfig::from_limits(&app.cfg.limits);
    worker::tick(app, "w1", &config, &|| Timestamp(at));
}

fn state_of(app: &App, site: &TenantId, file: &str) -> Option<FileState> {
    storage::get_meta(app, site, file)
        .expect("metadata")
        .map(|meta| meta.state)
}

fn create_photo(app: &App, site: &TenantId, fields: &[(&str, &str)]) -> RecordId {
    let executed = execute(
        app.store.as_ref(),
        &Intent::CreateRecord {
            object: ObjectTypeId::new("photo").expect("object id"),
            fields: fields
                .iter()
                .map(|(k, v)| {
                    (
                        FieldId::new(*k).expect("field id"),
                        Value::Str(v.to_string()),
                    )
                })
                .collect(),
        },
        &as_owner(site, T0 + 1),
    )
    .expect("the record is created");
    match executed.transition.output {
        Output::Record { id, .. } => id,
        other => panic!("unexpected output {other:?}"),
    }
}

fn sign(app: &App, as_ctx: &Context, file: &str) -> Result<String, ApiError> {
    let signed: Json = sign_media_address(
        app,
        as_ctx,
        &SignRequest {
            file: file.to_string(),
            ..SignRequest::default()
        },
    )?;
    Ok(signed["address"].as_str().expect("an address").to_string())
}

fn request_of(address: &str, range: Option<&str>) -> DeliveryRequest {
    let parts: Vec<&str> = address
        .strip_prefix("/files/")
        .expect("an address path")
        .split('/')
        .collect();
    assert_eq!(parts.len(), 4, "{address}");
    DeliveryRequest {
        file: parts[0].to_string(),
        expires: parts[1].to_string(),
        scope: parts[2].to_string(),
        signature: parts[3].to_string(),
        range: range.map(str::to_string),
    }
}

fn deliver(app: &App, as_ctx: &Context, address: &str) -> Result<Delivery, ApiError> {
    deliver_file(app, as_ctx, &request_of(address, None))
}

fn code(result: Result<impl std::fmt::Debug, ApiError>) -> ErrorCode {
    match result {
        Ok(value) => panic!("expected a refusal, got {value:?}"),
        Err(error) => error.0.code,
    }
}

fn stored_files(app: &App, site: &TenantId) -> usize {
    app.store.doc_list(site, FILE_KIND).expect("list").len()
}

// Verifies: CMS-261
#[test]
fn type_sniffing_refuses_a_mislabelled_file_an_executable_and_markup() {
    let app = memory_app("sniff");
    let site = found(app.store.as_ref(), "sniff");
    let owner = as_owner(&site, T0);
    let refused = |declared: &str, content: &[u8], target: Option<&str>| {
        let request = UploadRequest {
            declared_type: declared.into(),
            name: "x".into(),
            object: target.map(|_| "photo".to_string()),
            field: target.map(str::to_string),
            tenant: None,
        };
        upload_file(&app, &owner, &request, content).expect_err("refused")
    };
    // A PNG declared as a JPEG is refused on what it is.
    let mislabelled = refused("image/jpeg", &png(4, 4, b"a"), None);
    assert_eq!(mislabelled.0.code, ErrorCode::Invalid);
    assert!(
        mislabelled.0.message.contains("image/png"),
        "{mislabelled:?}"
    );
    // An executable is refused whatever it is declared as.
    for declared in ["application/octet-stream", "image/png"] {
        let program = refused(declared, b"MZ\x90\0\x03\0\0\0this program", None);
        assert!(program.0.message.contains("executable"), "{program:?}");
        let elf = refused(declared, b"\x7fELF\x02\x01\x01", None);
        assert!(elf.0.message.contains("executable"), "{elf:?}");
    }
    // Markup that calls itself an image is refused: SVG can carry script.
    let svg = refused(
        "image/svg+xml",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        None,
    );
    assert!(svg.0.message.contains("markup"), "{svg:?}");
    // A PNG for a field that accepts only PDF is refused.
    let wrong_field = refused("image/png", &png(4, 4, b"b"), Some("doc"));
    assert_eq!(wrong_field.0.code, ErrorCode::Invalid);
    // An image whose header promises a decompression bomb is refused before it is decoded.
    let bomb = refused("image/png", &png(100_000, 100_000, b"c"), None);
    assert!(bomb.0.message.contains("decoded"), "{bomb:?}");
    assert_eq!(stored_files(&app, &site), 0, "nothing refused is kept");
    // The name is data: it is kept as given, never interpreted.
    let kept = upload_file(&app, &owner, &image_request(), &png(4, 4, b"d")).expect("accepted");
    let meta = storage::get_meta(&app, &site, &kept.file)
        .expect("metadata")
        .expect("kept");
    assert_eq!(meta.media_type, "image/png");
    assert_eq!(meta.uploads[0].name, "holiday <b>photo</b>.png");
}

// Verifies: CMS-1593
#[test]
fn size_caps_refuse_before_the_content_is_kept() {
    let app = memory_app("caps");
    let site = found(app.store.as_ref(), "caps");
    let owner = as_owner(&site, T0);
    // One call takes at most 1048576 bytes; the content is not read past the cap.
    let large = vec![0u8; SINGLE_MAX_BYTES as usize + 1];
    let single = upload_file(&app, &owner, &image_request(), &large).expect_err("too large");
    assert_eq!(single.0.kind.as_deref(), Some("too_large"));
    // A chunked upload is refused at its start, by the platform maximum and by the field's cap.
    let platform = start_upload(&app, &owner, &image_request(), PLATFORM_MAX_BYTES + 1)
        .expect_err("over the platform maximum");
    assert_eq!(platform.0.kind.as_deref(), Some("too_large"));
    let for_doc = UploadRequest {
        declared_type: "application/pdf".into(),
        name: "a.pdf".into(),
        object: Some("photo".into()),
        field: Some("doc".into()),
        tenant: None,
    };
    let field = start_upload(&app, &owner, &for_doc, 1001).expect_err("over the field's cap");
    assert_eq!(field.0.kind.as_deref(), Some("too_large"));
    assert!(app
        .store
        .doc_list(&site, UPLOAD_KIND)
        .expect("list")
        .is_empty());
    // A chunk longer than its index allows is refused, so the total never exceeds the size.
    let started = start_upload(&app, &owner, &image_request(), 100).expect("started");
    let long = code(put_upload_chunk(
        &app,
        &owner,
        &started.upload,
        0,
        &[0u8; 101],
    ));
    assert_eq!(long, ErrorCode::Invalid);
    let beyond = code(put_upload_chunk(
        &app,
        &owner,
        &started.upload,
        1,
        &[0u8; 1],
    ));
    assert_eq!(beyond, ErrorCode::Invalid);
    assert_eq!(stored_files(&app, &site), 0);
}

#[test]
fn a_chunked_upload_assembles_and_verifies() {
    let app = memory_app("chunks");
    let site = found(app.store.as_ref(), "chunks");
    let owner = as_owner(&site, T0);
    let mut content = png(640, 480, b"chunked");
    content.resize(CHUNK_BYTES as usize + 100, 7);
    let started =
        start_upload(&app, &owner, &image_request(), content.len() as u64).expect("started");
    assert_eq!(started.chunks, 2);
    let (first, second) = content.split_at(CHUNK_BYTES as usize);
    // Out of order, and a repeat with the same content changes nothing.
    put_upload_chunk(&app, &owner, &started.upload, 1, second).expect("second chunk");
    // Finishing before every chunk is there is refused.
    let early = code(finish_upload(&app, &owner, &started.upload));
    assert_eq!(early, ErrorCode::Invalid);
    let again =
        start_upload(&app, &owner, &image_request(), content.len() as u64).expect("started again");
    put_upload_chunk(&app, &owner, &again.upload, 1, second).expect("second chunk");
    let received = put_upload_chunk(&app, &owner, &again.upload, 0, first).expect("first");
    assert_eq!(received, content.len() as u64);
    put_upload_chunk(&app, &owner, &again.upload, 0, first).expect("the same chunk again");
    let mut other = first.to_vec();
    other[100] ^= 1;
    let conflict = code(put_upload_chunk(&app, &owner, &again.upload, 0, &other));
    assert_eq!(conflict, ErrorCode::Conflict);
    // Another principal cannot add to or finish the upload.
    let reader = as_user(&site, "reader", T0);
    assert_eq!(
        code(finish_upload(&app, &reader, &again.upload)),
        ErrorCode::NotFound
    );
    let finished = finish_upload(&app, &owner, &again.upload).expect("finished");
    assert_eq!(
        finished.file,
        strata_kernel::canon::hash_bytes(&content),
        "the file id is the hash of the content as assembled"
    );
    assert_eq!(finished.state, "scanning");
    let (bytes, media_type) = storage::get_file(&app, &site, &finished.file).expect("stored");
    assert_eq!(bytes, content);
    assert_eq!(media_type, "image/png");
    // The upload is over: finishing it again finds nothing.
    assert_eq!(
        code(finish_upload(&app, &owner, &again.upload)),
        ErrorCode::NotFound
    );
}

// Verifies: CMS-260
#[test]
fn an_infected_file_never_becomes_deliverable() {
    let app = memory_app("infected");
    let site = found(app.store.as_ref(), "infected");
    let owner = as_owner(&site, T0);
    let infected = upload_png(&app, &site, &eicar());
    let clean = upload_png(&app, &site, b"nothing to see");
    assert_eq!(state_of(&app, &site, &infected), Some(FileState::Scanning));
    // Not released before the scan: no address is signed, and a forged one delivers nothing.
    assert_eq!(code(sign(&app, &owner, &clean)), ErrorCode::Conflict);
    let forged = sign_file_address(
        &app,
        &site,
        &clean,
        Timestamp(T0 + 60_000),
        Some(&who("infected:owner")),
    )
    .expect("signed in process");
    assert_eq!(
        code(deliver(&app, &owner, &forged.path())),
        ErrorCode::NotFound
    );
    run_jobs(&app, T0 + 1_000);
    assert_eq!(state_of(&app, &site, &infected), Some(FileState::Rejected));
    assert_eq!(state_of(&app, &site, &clean), Some(FileState::Clean));
    // The rejected file's bytes are gone, it cannot be signed or delivered, and uploading it
    // again is refused.
    assert!(!storage::bytes_path(&app, &site, &infected)
        .expect("path")
        .exists());
    assert_eq!(code(sign(&app, &owner, &infected)), ErrorCode::Invalid);
    let forged = sign_file_address(
        &app,
        &site,
        &infected,
        Timestamp(T0 + 60_000),
        Some(&who("infected:owner")),
    )
    .expect("signed in process");
    assert_eq!(
        code(deliver(&app, &owner, &forged.path())),
        ErrorCode::NotFound
    );
    let again = png(64, 32, &eicar());
    assert_eq!(
        code(upload_file(&app, &owner, &image_request(), &again)),
        ErrorCode::Invalid
    );
    // The clean file is delivered, with headers that keep a browser from running it.
    let address = sign(&app, &owner, &clean).expect("signed");
    let delivered = deliver(&app, &owner, &address).expect("delivered");
    assert_eq!(delivered.status, 200);
    assert_eq!(delivered.header("content-type"), Some("image/png"));
    assert_eq!(delivered.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(delivered.header("cache-control"), Some("private, no-store"));
    assert!(delivered
        .header("content-security-policy")
        .is_some_and(|csp| csp.contains("default-src 'none'") && csp.contains("sandbox")));
    assert!(delivered
        .header("content-disposition")
        .is_some_and(|d| d.starts_with("inline")));
}

// Verifies: CMS-1591
// Verifies: EDG-1592
#[test]
fn readability_follows_the_record_and_its_field_rule() {
    let app = memory_app("readers");
    let site = found(app.store.as_ref(), "readers");
    let owner = as_owner(&site, T0 + 2_000);
    let reader = as_user(&site, "reader", T0 + 2_000);
    let editor = as_user(&site, "editor", T0 + 2_000);
    let outsider = as_user(&site, "outsider", T0 + 2_000);
    let open = upload_png(&app, &site, b"open");
    let secret = upload_png(&app, &site, b"secret");
    run_jobs(&app, T0 + 1_000);
    // Unattached, a file is its uploader's alone.
    assert!(sign(&app, &owner, &open).is_ok());
    assert_eq!(code(sign(&app, &reader, &open)), ErrorCode::NotFound);
    // A reader without a capability may not upload.
    assert_eq!(
        code(upload_file(
            &app,
            &reader,
            &image_request(),
            &png(4, 4, b"r")
        )),
        ErrorCode::Forbidden
    );
    create_photo(
        &app,
        &site,
        &[
            ("title", "a"),
            ("image", open.as_str()),
            ("secret", secret.as_str()),
        ],
    );
    // The record's readers read the open field's file; only an editor reads the secret field's.
    assert!(sign(&app, &reader, &open).is_ok());
    assert_eq!(code(sign(&app, &reader, &secret)), ErrorCode::NotFound);
    assert!(sign(&app, &editor, &secret).is_ok());
    assert_eq!(code(sign(&app, &outsider, &open)), ErrorCode::NotFound);
    // An address is not signed for a principal who may not read the file, even by one who may.
    let for_reader = sign_media_address(
        &app,
        &editor,
        &SignRequest {
            file: secret.clone(),
            principal: Some("readers:reader".into()),
            ..SignRequest::default()
        },
    );
    assert_eq!(code(for_reader), ErrorCode::NotFound);
    // Delivery checks again: the editor's address works for the editor, not for a reader who
    // holds it, and stops working when the editor loses the role.
    let address = sign(&app, &editor, &secret).expect("signed");
    assert_eq!(
        deliver(&app, &editor, &address).expect("delivered").status,
        200
    );
    assert_eq!(code(deliver(&app, &reader, &address)), ErrorCode::NotFound);
    execute(
        app.store.as_ref(),
        &Intent::RevokeRole {
            principal: who("readers:editor"),
            role: role("editor"),
        },
        &as_owner(&site, T0 + 2_100),
    )
    .expect("the role is revoked");
    assert_eq!(code(deliver(&app, &editor, &address)), ErrorCode::NotFound);
}

// Verifies: EDG-012
// Verifies: EDG-1592
#[test]
fn a_signed_address_expires_and_binds_its_file_principal_and_tenant() {
    let app = memory_app("addresses");
    let site = found(app.store.as_ref(), "addresses");
    let other_site = found(app.store.as_ref(), "elsewhere");
    let now = T0 + 2_000;
    let owner = as_owner(&site, now);
    let first = upload_png(&app, &site, b"first");
    let second = upload_png(&app, &site, b"second");
    run_jobs(&app, T0 + 1_000);
    let address = sign(&app, &owner, &first).expect("signed");
    assert_eq!(
        deliver(&app, &owner, &address).expect("delivered").status,
        200
    );
    // One range.
    let ranged =
        deliver_file(&app, &owner, &request_of(&address, Some("bytes=0-7"))).expect("a range");
    assert_eq!(ranged.status, 206);
    assert_eq!(ranged.body, b"\x89PNG\r\n\x1a\n");
    // Not for another file: the signature is of the file.
    let mut moved = request_of(&address, None);
    moved.file = second.clone();
    assert_eq!(
        code(deliver_file(&app, &owner, &moved)),
        ErrorCode::NotFound
    );
    // Not for another principal, nor without a session.
    assert_eq!(
        code(deliver(&app, &as_user(&site, "editor", now), &address)),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(deliver(
            &app,
            &ctx(&site, Principal::Anonymous, now),
            &address
        )),
        ErrorCode::NotFound
    );
    // Not at another tenant, even one that holds the same content under the same id.
    let theirs = upload_file(
        &app,
        &as_owner(&other_site, T0),
        &image_request(),
        &png(64, 32, b"first"),
    )
    .expect("the same content at another tenant");
    assert_eq!(theirs.file, first);
    run_jobs(&app, T0 + 1_000);
    let copied = request_of(&address, None);
    assert_eq!(
        code(deliver_file(
            &app,
            &as_user(&other_site, "owner", now),
            &copied
        )),
        ErrorCode::NotFound
    );
    // Not once it has expired, and never for longer than an hour.
    let later = as_owner(&site, now + 3_600_001);
    assert_eq!(code(deliver(&app, &later, &address)), ErrorCode::NotFound);
    let too_long = sign_media_address(
        &app,
        &owner,
        &SignRequest {
            file: first.clone(),
            expires_at: Some(now + 3_600_001),
            ..SignRequest::default()
        },
    );
    assert_eq!(code(too_long), ErrorCode::Invalid);
    let past = sign_media_address(
        &app,
        &owner,
        &SignRequest {
            file: first,
            expires_at: Some(now - 1),
            ..SignRequest::default()
        },
    );
    assert_eq!(code(past), ErrorCode::Invalid);
}

#[test]
fn path_traversal_and_encoded_slashes_in_ids_are_refused() {
    let app = memory_app("paths");
    let site = found(app.store.as_ref(), "paths");
    let owner = as_owner(&site, T0);
    let file = upload_png(&app, &site, b"paths");
    let hex64 = "a".repeat(64);
    let upper = file.to_uppercase();
    let short = file[..63].to_string();
    let nested = format!("{file}/x");
    let encoded = format!("{}%2F{}", &file[..30], &file[33..]);
    let cases: Vec<&str> = vec![
        "../../../../etc/passwd",
        "..%2F..%2Fsecret",
        "..\\..\\secret",
        "aa/../bb",
        "%2e%2e",
        upper.as_str(),
        short.as_str(),
        nested.as_str(),
        encoded.as_str(),
        "",
    ];
    for bad in cases {
        assert!(
            Address::parse(bad, "1", "principal", &hex64).is_none(),
            "{bad}"
        );
        assert!(storage::bytes_path(&app, &site, bad).is_err(), "{bad}");
        assert_eq!(
            code(storage::get_file(&app, &site, bad)),
            ErrorCode::NotFound
        );
        let request = DeliveryRequest {
            file: bad.to_string(),
            expires: (T0 + 1_000).to_string(),
            scope: "principal".into(),
            signature: hex64.clone(),
            range: None,
        };
        assert_eq!(
            code(deliver_file(&app, &owner, &request)),
            ErrorCode::NotFound
        );
        assert_eq!(
            code(put_upload_chunk(&app, &owner, bad, 0, b"x")),
            ErrorCode::NotFound
        );
        assert_eq!(code(finish_upload(&app, &owner, bad)), ErrorCode::NotFound);
    }
    for bad_expiry in ["-1", "1e9", "../1", "1%2F2", "9999999999999999"] {
        assert!(Address::parse(&file, bad_expiry, "principal", &hex64).is_none());
    }
    assert!(Address::parse(&file, "1", "../public", &hex64).is_none());
    assert!(Address::parse(&file, "1", "principal", "../x").is_none());
}

// Verifies: CMS-1591
#[test]
fn an_unreferenced_file_is_purged_and_a_referenced_one_kept() {
    let app = memory_app("purge");
    let site = found(app.store.as_ref(), "purge");
    let loose = upload_png(&app, &site, b"loose");
    let kept = upload_png(&app, &site, b"kept");
    create_photo(&app, &site, &[("image", kept.as_str())]);
    let started = start_upload(&app, &as_owner(&site, T0), &image_request(), 10).expect("started");
    put_upload_chunk(&app, &as_owner(&site, T0), &started.upload, 0, &[1u8; 10]).expect("chunk");
    let day = 24 * 3_600_000;
    assert_eq!(
        purge_tenant(&app, &site, Timestamp(T0 + day - 1)).expect("purged"),
        0
    );
    assert!(state_of(&app, &site, &loose).is_some());
    let removed = purge_tenant(&app, &site, Timestamp(T0 + day)).expect("purged");
    assert_eq!(removed, 2, "the loose file and the unfinished upload");
    assert!(state_of(&app, &site, &loose).is_none());
    assert!(!storage::bytes_path(&app, &site, &loose)
        .expect("path")
        .exists());
    assert!(state_of(&app, &site, &kept).is_some());
    assert!(storage::bytes_path(&app, &site, &kept)
        .expect("path")
        .exists());
    assert!(app
        .store
        .doc_list(&site, UPLOAD_KIND)
        .expect("list")
        .is_empty());
}

// Verifies: CMS-1591
#[test]
fn deleting_the_last_reference_to_a_file_purges_its_bytes_a_day_later() {
    let app = memory_app("lastref");
    let site = found(app.store.as_ref(), "lastref");
    let file = upload_png(&app, &site, b"once attached");
    let record = create_photo(&app, &site, &[("image", file.as_str())]);
    let day = 24 * 3_600_000;
    let seen = T0 + 1_000;
    // While a record refers to it, no pass removes it, however late; the pass notes when it saw
    // the reference.
    assert_eq!(
        purge_tenant(&app, &site, Timestamp(seen)).expect("purged"),
        0
    );
    assert_eq!(
        purge_tenant(&app, &site, Timestamp(seen + 30 * day)).expect("purged"),
        0,
        "a referenced file is kept"
    );
    let noted = storage::get_meta(&app, &site, &file)
        .expect("metadata")
        .expect("the file exists");
    assert!(noted.referenced_at.is_some());
    // The last reference is deleted: the file is not removed at once, but at the retention point
    // ret-file-bytes states, 24 hours after the reference was last seen.
    execute(
        app.store.as_ref(),
        &Intent::DeleteRecord {
            object: ObjectTypeId::new("photo").expect("object id"),
            id: record,
            expected_version: None,
        },
        &as_owner(&site, seen + 31 * day),
    )
    .expect("the record is deleted");
    let last_seen = noted.referenced_at.expect("seen referred to");
    assert_eq!(
        purge_tenant(&app, &site, Timestamp(last_seen + day - 1)).expect("purged"),
        0
    );
    assert!(state_of(&app, &site, &file).is_some());
    assert!(storage::bytes_path(&app, &site, &file)
        .expect("path")
        .exists());
    assert_eq!(
        purge_tenant(&app, &site, Timestamp(last_seen + day)).expect("purged"),
        1
    );
    assert!(state_of(&app, &site, &file).is_none());
    assert!(
        !storage::bytes_path(&app, &site, &file)
            .expect("path")
            .exists(),
        "the bytes are gone"
    );
}

// Verifies: CMS-008
#[test]
fn another_tenant_cannot_fetch_by_id_or_by_a_copied_address() {
    let store: Arc<dyn Store> =
        Arc::new(SqliteStore::open_in_memory(Timestamp(T0)).expect("store"));
    let app = app_on(store, "tenants");
    let site = found(app.store.as_ref(), "home");
    let other_site = found(app.store.as_ref(), "away");
    let file = upload_png(&app, &site, b"home only");
    run_jobs(&app, T0 + 1_000);
    let now = T0 + 2_000;
    let address = sign(&app, &as_owner(&site, now), &file).expect("signed");
    // By id: the other tenant has no such file.
    assert_eq!(
        code(storage::get_file(&app, &other_site, &file)),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(sign(&app, &as_owner(&other_site, now), &file)),
        ErrorCode::NotFound
    );
    // By a copied address, whoever presents it there.
    for name in ["owner", "reader", "outsider"] {
        assert_eq!(
            code(deliver(&app, &as_user(&other_site, name, now), &address)),
            ErrorCode::NotFound,
            "{name}"
        );
    }
    assert_eq!(
        code(deliver(
            &app,
            &ctx(&other_site, Principal::Anonymous, now),
            &address
        )),
        ErrorCode::NotFound
    );
}

// Verifies: PRV-1586
#[test]
fn erasure_removes_the_files_only_the_subjects_personal_fields_refer_to() {
    let app = memory_app("erasure");
    let site = found(app.store.as_ref(), "erasure");
    let owner = as_owner(&site, T0 + 1);
    let own = upload_png(&app, &site, b"ada's avatar");
    let shared = upload_png(&app, &site, b"shared picture");
    for (avatar, person) in [(&own, "erasure:ada"), (&shared, "erasure:ada")] {
        execute(
            app.store.as_ref(),
            &Intent::CreateRecord {
                object: ObjectTypeId::new("profile").expect("object id"),
                fields: [
                    (
                        FieldId::new("person").expect("field id"),
                        Value::Str(person.into()),
                    ),
                    (
                        FieldId::new("avatar").expect("field id"),
                        Value::Str(avatar.to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            },
            &owner,
        )
        .expect("a profile");
    }
    // The shared picture is also a photo's image, which is not personal.
    create_photo(&app, &site, &[("image", shared.as_str())]);
    let receipt =
        strata_backend::erasure::erase_subject(&app, &owner, &who("erasure:ada")).expect("erased");
    assert_eq!(receipt.principal, "erasure:ada");
    assert!(state_of(&app, &site, &own).is_none(), "the avatar is gone");
    assert!(!storage::bytes_path(&app, &site, &own)
        .expect("path")
        .exists());
    assert!(
        state_of(&app, &site, &shared).is_some(),
        "the photo keeps it"
    );
    let noted = app
        .store
        .doc_get(&site, strata_backend::files::ERASURE_KIND, "t-files-1")
        .expect("read")
        .expect("what the erasure removed is kept");
    assert!(noted.contains(&own) && !noted.contains(&shared), "{noted}");
}

/// The file id of a PNG nobody has uploaded.
fn unstored_file() -> String {
    use sha2::{Digest, Sha256};
    storage::hex(&Sha256::digest(png(8, 8, b"never uploaded")))
}

fn photo_fields(fields: &[(&str, &str)]) -> BTreeMap<FieldId, Value> {
    fields
        .iter()
        .map(|(k, v)| {
            (
                FieldId::new(*k).expect("field id"),
                Value::Str(v.to_string()),
            )
        })
        .collect()
}

/// Create a photo through the backend's write path, as `as_ctx`.
fn create_as(app: &App, as_ctx: &Context, fields: &[(&str, &str)]) -> Result<(), ApiError> {
    ops::run_intent(
        app,
        as_ctx,
        &Intent::CreateRecord {
            object: ObjectTypeId::new("photo").expect("object id"),
            fields: photo_fields(fields),
        },
        false,
    )
    .map(|_| ())
}

/// Update a photo through the backend's write path, as `as_ctx`.
fn update_as(
    app: &App,
    as_ctx: &Context,
    id: RecordId,
    fields: &[(&str, &str)],
) -> Result<(), ApiError> {
    ops::run_intent(
        app,
        as_ctx,
        &Intent::UpdateRecord {
            object: ObjectTypeId::new("photo").expect("object id"),
            id,
            fields: photo_fields(fields),
            expected_version: None,
        },
        false,
    )
    .map(|_| ())
}

// Verifies: CMS-2900
#[test]
fn a_record_may_name_only_a_clean_file_the_writer_uploaded_or_may_read() {
    let app = memory_app("attach");
    let site = found(app.store.as_ref(), "attach");
    let writer = as_user(&site, "writer", T0 + 3_000);
    let owner = as_owner(&site, T0 + 3_000);
    let mine = upload_png(&app, &site, b"mine");
    // Still scanning: refused, for the uploader too.
    assert_eq!(state_of(&app, &site, &mine), Some(FileState::Scanning));
    assert_eq!(
        code(create_as(&app, &owner, &[("image", mine.as_str())])),
        ErrorCode::NotFound
    );
    run_jobs(&app, T0 + 1_000);
    assert_eq!(state_of(&app, &site, &mine), Some(FileState::Clean));
    // Another principal's unattached upload, a hash nobody stored and a text that is no hash are
    // refused with one answer.
    let theirs = create_as(&app, &writer, &[("image", mine.as_str())]).expect_err("refused");
    let unstored =
        create_as(&app, &writer, &[("image", unstored_file().as_str())]).expect_err("refused");
    let no_hash = create_as(&app, &writer, &[("image", "../etc/passwd")]).expect_err("refused");
    assert_eq!(theirs.0.code, ErrorCode::NotFound);
    assert_eq!(theirs.0.message, unstored.0.message);
    assert_eq!(theirs.0.message, no_hash.0.message);
    assert_eq!(unstored.0.code, ErrorCode::NotFound);
    assert_eq!(no_hash.0.code, ErrorCode::NotFound);
    // The uploader may name their own clean file.
    assert!(create_as(&app, &owner, &[("title", "a"), ("image", mine.as_str())]).is_ok());
}

// Verifies: CMS-2900
#[test]
fn a_rejected_file_and_another_tenants_file_cannot_be_named() {
    let app = memory_app("attach_other");
    let site = found(app.store.as_ref(), "attach_other");
    let elsewhere = found(app.store.as_ref(), "attach_elsewhere");
    let owner = as_owner(&site, T0 + 3_000);
    let foreign = upload_png(&app, &elsewhere, b"foreign");
    let infected = upload_png(&app, &site, &eicar());
    run_jobs(&app, T0 + 1_000);
    assert_eq!(state_of(&app, &elsewhere, &foreign), Some(FileState::Clean));
    assert_eq!(state_of(&app, &site, &infected), Some(FileState::Rejected));
    // The other tenant's file is not stored here, so even its uploader's namesake is refused.
    assert_eq!(
        code(create_as(&app, &owner, &[("image", foreign.as_str())])),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(create_as(&app, &owner, &[("image", infected.as_str())])),
        ErrorCode::NotFound
    );
}

// Verifies: CMS-2900
#[test]
fn an_unchanged_file_field_is_not_checked_again_and_a_changed_one_is() {
    let app = memory_app("attach_update");
    let site = found(app.store.as_ref(), "attach_update");
    let writer = as_user(&site, "writer", T0 + 3_000);
    let first = upload_png(&app, &site, b"first");
    let second = upload_png(&app, &site, b"second");
    run_jobs(&app, T0 + 1_000);
    let record = create_photo(&app, &site, &[("title", "a"), ("image", first.as_str())]);
    // The writer reads no photo and uploaded neither file: the value already held is not looked at
    // again, a new one is refused.
    assert!(update_as(
        &app,
        &writer,
        record,
        &[("title", "b"), ("image", first.as_str())]
    )
    .is_ok());
    assert_eq!(
        code(update_as(
            &app,
            &writer,
            record,
            &[("image", second.as_str())]
        )),
        ErrorCode::NotFound
    );
    assert!(update_as(
        &app,
        &as_owner(&site, T0 + 3_000),
        record,
        &[("image", second.as_str())]
    )
    .is_ok());
}

/// The operation that creates a photo holding the file it is given.
fn photo_operation() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("add_photo").expect("operation id"),
        label: "Add photo".into(),
        inputs: vec![TenantOperationInput {
            name: "image".into(),
            ty: file_type(&["image/*"], 2_000_000),
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: Vec::new(),
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: ObjectTypeId::new("photo").expect("object id"),
            fields: BTreeMap::from([(
                FieldId::new("image").expect("field id"),
                Expr::Param("image".into()),
            )]),
        }],
    }
}

// Verifies: CMS-2900
#[test]
fn a_tenant_operation_names_files_under_the_same_rule() {
    let app = memory_app("attach_operation");
    let site = found(app.store.as_ref(), "attach_operation");
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineOperation(photo_operation())],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &as_owner(&site, T0 + 2_900),
    )
    .expect("the operation is defined");
    let owned = upload_png(&app, &site, b"op");
    run_jobs(&app, T0 + 1_000);
    let invoke = |as_ctx: &Context, file: &str, key: &str| {
        let mut ctx = as_ctx.clone();
        ctx.idempotency_key = Some(key.to_string());
        ops::run_intent(
            &app,
            &ctx,
            &Intent::InvokeOperation {
                operation: TenantOperationId::new("add_photo").expect("operation id"),
                inputs: BTreeMap::from([("image".to_string(), Value::Str(file.to_string()))]),
                idempotency_key: key.to_string(),
            },
            false,
        )
        .map(|_| ())
    };
    let writer = as_user(&site, "writer", T0 + 3_000);
    let owner = as_owner(&site, T0 + 3_000);
    assert_eq!(
        code(invoke(&writer, &owned, "k-writer")),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(invoke(&owner, &unstored_file(), "k-gone")),
        ErrorCode::NotFound
    );
    assert!(invoke(&owner, &owned, "k-owner").is_ok());
}
