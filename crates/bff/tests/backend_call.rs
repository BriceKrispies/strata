//! The BFF's call to the backend's internal API, against a stand-in backend that answers as told.

use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use strata_bff::routes::Visit;
use strata_bff::{token, Bff, Config};
use strata_kernel::TenantId;

fn bff_for(backend_url: String) -> Bff {
    Bff {
        cfg: Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            backend: backend_url,
            service_token: "service-token-0123456789abcdef".into(),
            sites: Config::parse_sites("127.0.0.1=fieldnotes").unwrap(),
            origins: vec![],
            hosts: Default::default(),
            dist: PathBuf::new(),
            dev: true,
            token_secret: None,
            trusted_proxies: vec![],
            limits: Default::default(),
        },
        admission: strata_bff::limits::Admission::new(&Default::default()),
        http: reqwest::Client::new(),
        sockets: AtomicI64::new(0),
        in_flight: AtomicI64::new(0),
        index: None,
        tokens: token::Issuer::new().unwrap(),
        directory: strata_bff::hosts::Directory::new(Default::default()),
    }
}

/// Verifies: GWY-1409
/// The address a SAML provider is given is the same whatever order the origins were listed in.
#[test]
fn the_published_origin_of_a_site_does_not_depend_on_the_order_of_the_list() {
    let site = TenantId::new("fieldnotes").unwrap();
    let mut bff = bff_for("http://127.0.0.1:1".into());
    let listed = |order: &[&str]| order.iter().map(|o| o.to_string()).collect::<Vec<_>>();
    bff.cfg.origins = listed(&[
        "http://127.0.0.1:5173",
        "https://127.0.0.1",
        "http://127.0.0.1:4100",
    ]);
    let first = strata_bff::routes::primary_origin(&bff, &site);
    bff.cfg.origins = listed(&[
        "http://127.0.0.1:4100",
        "http://127.0.0.1:5173",
        "https://127.0.0.1",
    ]);
    assert_eq!(first, strata_bff::routes::primary_origin(&bff, &site));
    assert_eq!(first.as_deref(), Some("https://127.0.0.1"), "https first");
    bff.cfg.origins = listed(&["http://127.0.0.1:5173", "http://127.0.0.1:4100"]);
    assert_eq!(
        strata_bff::routes::primary_origin(&bff, &site).as_deref(),
        Some("http://127.0.0.1:4100")
    );
}

fn visit(session: Option<&str>) -> Visit {
    Visit {
        site: TenantId::new("fieldnotes").unwrap(),
        token: session.map(str::to_string),
        correlation: "c-visit-7".into(),
        source: None,
        origin: None,
        idempotency_key: None,
    }
}
