//! Checks of the working-set harness: the scenario files, the report's determinism and the
//! coherence check, including that it catches the deliberately broken `stale` wrapper.

use std::collections::BTreeSet;
use std::path::PathBuf;
use strata_ws_bench::corpus::NotesLoader;
use strata_ws_bench::generation::RevisionView;
use strata_ws_bench::modes::Mode;
use strata_ws_bench::report::{Latency, Report};
use strata_ws_bench::rng::{Rng, Zipf};
use strata_ws_bench::runner::{run, CoherenceLevel, Config};
use strata_ws_bench::scenario::{strip_jsonc, OpKind, Scenario};
use strata_ws_bench::stores::StoreSpec;

fn repo(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn scenarios() -> Vec<Scenario> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(repo("tests/scenarios")).expect("tests/scenarios") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonc") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read scenario");
        found.push(Scenario::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    }
    found
}

/// The objects, queries and module names the corpus declares.
fn corpus_names() -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let (mut objects, mut queries, mut modules) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in std::fs::read_dir(repo("tests/corpus/enterprise")).expect("corpus") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonc") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read corpus file");
        let json: serde_json::Value =
            serde_json::from_str(&strip_jsonc(&text)).expect("corpus json");
        modules.insert(
            json["product"]["id"]
                .as_str()
                .expect("product id")
                .to_string(),
        );
        for (section, into) in [("objects", &mut objects), ("queries", &mut queries)] {
            for item in json[section].as_array().into_iter().flatten() {
                into.insert(item["id"].as_str().expect("id").to_string());
            }
        }
    }
    (objects, queries, modules)
}

#[test]
fn the_scenario_files_name_real_corpus_modules_objects_and_queries() {
    let (objects, queries, modules) = corpus_names();
    let all = scenarios();
    assert!(
        all.len() >= 8,
        "at least eight scenarios, found {}",
        all.len()
    );
    for scenario in &all {
        for module in &scenario.modules {
            assert!(
                modules.contains(module),
                "{}: no module {module}",
                scenario.name
            );
        }
        for entry in &scenario.mix {
            if let Some(object) = &entry.object {
                assert!(
                    objects.contains(object),
                    "{}: no object {object}",
                    scenario.name
                );
            }
            if let Some(query) = &entry.query {
                assert!(
                    queries.contains(query),
                    "{}: no query {query}",
                    scenario.name
                );
            }
        }
        for fault in &scenario.faults {
            if let Some(object) = &fault.object {
                assert!(
                    objects.contains(object),
                    "{}: no object {object}",
                    scenario.name
                );
            }
        }
    }
}

#[test]
fn the_scenarios_cover_the_required_products_and_faults() {
    let all = scenarios();
    let names: BTreeSet<&str> = all.iter().map(|s| s.name.as_str()).collect();
    for required in [
        "editorial-publishing-day",
        "commerce-flash-sale",
        "support-desk-surge",
        "marketing-segment-send",
        "analytics-dashboards",
        "erasure-mid-workload",
        "restore-mid-workload",
        "schema-change-mid-workload",
    ] {
        assert!(names.contains(required), "missing scenario {required}");
    }
    let touches = |module: &str| all.iter().any(|s| s.modules.iter().any(|m| m == module));
    assert!(touches("approvals"), "approvals are in a scenario");
    let fault = |kind: strata_ws_bench::scenario::FaultKind| {
        all.iter().any(|s| s.faults.iter().any(|f| f.kind == kind))
    };
    use strata_ws_bench::scenario::FaultKind::*;
    assert!(fault(Erasure) && fault(Restore) && fault(SchemaChange) && fault(ForeignCommit));
    assert!(fault(Reopen));
    assert!(all.iter().any(|s| s.small_tenants.is_some()));
    assert!(all
        .iter()
        .any(|s| s.mix.iter().any(|m| m.op == OpKind::Delete)));
}

#[test]
fn a_misspelt_key_is_refused() {
    let text = r#"{ "name": "x", "descripton": "typo" }"#;
    assert!(Scenario::parse(text).is_err());
}

#[test]
fn jsonc_comments_and_trailing_commas_are_stripped() {
    let cleaned = strip_jsonc("// note\n{ \"a\": [1, 2,], \"b\": \"x,]\", }\n");
    let value: serde_json::Value = serde_json::from_str(&cleaned).expect("json");
    assert_eq!(value["a"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["b"], "x,]");
}

#[test]
fn zipf_ranks_are_skewed_toward_the_front_and_uniform_at_zero() {
    let mut rng = Rng(7);
    let skewed = Zipf::new(100, 1.2);
    let front = (0..5_000).filter(|_| skewed.sample(&mut rng) < 5).count();
    assert!(front > 2_500, "rank < 5 drew {front} of 5000");
    let flat = Zipf::new(100, 0.0);
    let front = (0..5_000).filter(|_| flat.sample(&mut rng) < 5).count();
    assert!((100..500).contains(&front), "uniform drew {front}");
}

#[test]
fn percentiles_are_read_off_the_sorted_samples() {
    let mut samples: Vec<u64> = (1..=100).rev().collect();
    let latency = Latency::of(&mut samples);
    assert_eq!(
        (
            latency.p50_ns,
            latency.p95_ns,
            latency.p99_ns,
            latency.max_ns
        ),
        (50, 95, 99, 100)
    );
}

/// A small scenario over the stand-in tenant (`note`, `person`), with the given faults.
fn tiny(faults: &str) -> Scenario {
    let text = format!(
        r#"{{
        "name": "tiny", "description": "unit", "modules": ["content"],
        "scale": 60, "length": 400, "clients": 3, "skew": 1.1,
        "mix": [
            {{ "op": "get", "object": "note", "weight": 10, "actor": "editor" }},
            {{ "op": "by-field", "object": "note", "weight": 3, "actor": "editor" }},
            {{ "op": "page", "object": "note", "weight": 2, "page_size": 10 }},
            {{ "op": "scan", "object": "person", "weight": 1 }},
            {{ "op": "header", "weight": 2 }},
            {{ "op": "update", "object": "note", "weight": 2, "actor": "editor", "contended": true }},
            {{ "op": "create", "object": "note", "weight": 1, "actor": "editor" }},
            {{ "op": "delete", "object": "note", "weight": 1, "actor": "editor" }}
        ],
        "faults": {faults},
        "measures": ["coherence.divergences"],
        "refutation": {{ "criterion": "unit", "result": "unit" }}
    }}"#
    );
    Scenario::parse(&text).expect("tiny scenario")
}

fn play(scenario: &Scenario, mode: Mode, seed: u64, store: StoreSpec) -> Report {
    let mut config = Config::new(store, mode, seed);
    config.coherence = CoherenceLevel::Full;
    run(scenario, &config, &NotesLoader, &RevisionView).expect("run")
}

#[test]
fn the_same_seed_gives_the_same_sequence_and_answers() {
    let scenario = tiny(r#"[{ "kind": "foreign-commit", "every": 50, "object": "note" }]"#);
    let a = play(&scenario, Mode::None, 11, StoreSpec::Memory);
    let b = play(&scenario, Mode::None, 11, StoreSpec::Memory);
    assert_eq!(a.determinism.sequence_digest, b.determinism.sequence_digest);
    assert_eq!(a.determinism.answer_digest, b.determinism.answer_digest);
    assert_eq!(
        serde_json::to_string(&a.ops).unwrap(),
        serde_json::to_string(&b.ops).unwrap()
    );
    let other = play(&scenario, Mode::None, 12, StoreSpec::Memory);
    assert_ne!(
        a.determinism.sequence_digest,
        other.determinism.sequence_digest
    );
}

#[test]
fn a_coherent_mode_gives_the_answers_of_no_wrapper() {
    let scenario = tiny(r#"[{ "kind": "foreign-commit", "every": 50, "object": "note" }]"#);
    let none = play(&scenario, Mode::None, 5, StoreSpec::Memory);
    for mode in [Mode::Header, Mode::Full] {
        let report = play(&scenario, mode, 5, StoreSpec::Memory);
        assert_eq!(
            report.determinism.answer_digest,
            none.determinism.answer_digest
        );
        assert_eq!(report.coherence.divergences, 0);
        assert!(
            report.wrapper.stats.placeholder,
            "{} is a placeholder",
            mode.name()
        );
    }
    assert_eq!(none.coherence.divergences, 0, "{:?}", none.coherence.first);
    assert!(none.coherence.checked > 300);
}

#[test]
fn the_check_catches_the_stale_wrapper_after_a_foreign_write() {
    let scenario = tiny(r#"[{ "kind": "foreign-commit", "every": 40, "object": "note" }]"#);
    let stale = play(&scenario, Mode::Stale, 5, StoreSpec::Memory);
    assert!(
        stale.coherence.divergences > 0,
        "the stale wrapper went undetected: {:?}",
        stale.coherence
    );
    assert!(!stale.coherence.first.is_empty());
    assert!(stale
        .wrapper
        .hit_rate_by_count
        .is_some_and(|rate| rate > 0.0));
    let none = play(&scenario, Mode::None, 5, StoreSpec::Memory);
    assert_eq!(none.coherence.divergences, 0);
}

#[test]
fn the_stale_wrapper_is_caught_after_a_restore_and_a_schema_change() {
    for faults in [
        r#"[{ "kind": "restore", "at": 0.5, "object": "note" }]"#,
        r#"[{ "kind": "schema-change", "at": 0.5 }]"#,
    ] {
        let scenario = tiny(faults);
        let stale = play(&scenario, Mode::Stale, 3, StoreSpec::Memory);
        assert!(
            stale.coherence.post_fault_divergences > 0,
            "{faults}: {:?}",
            stale.coherence
        );
        let none = play(&scenario, Mode::None, 3, StoreSpec::Memory);
        assert_eq!(
            none.coherence.divergences, 0,
            "{faults}: {:?}",
            none.coherence.first
        );
    }
}

#[test]
fn the_check_runs_over_sqlite_with_a_second_handle() {
    let scenario = tiny(r#"[{ "kind": "foreign-commit", "every": 40, "object": "note" }]"#);
    let none = play(&scenario, Mode::None, 9, StoreSpec::Sqlite(None));
    assert_eq!(none.coherence.divergences, 0, "{:?}", none.coherence.first);
    assert!(none.store_side.schema_decodes.is_some());
    let stale = play(&scenario, Mode::Stale, 9, StoreSpec::Sqlite(None));
    assert!(stale.coherence.divergences > 0);
}

#[test]
fn sqlcipher_is_refused_with_a_reason() {
    let refused = StoreSpec::parse("sqlcipher").expect_err("no sqlcipher in this build");
    assert!(refused.contains("SQLCipher"));
}
