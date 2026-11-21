//! `strata-corpus`: load the enterprise corpus into a tenant, populate it, and hash it.
//!
//! ```text
//! strata-corpus load  [--store <store>] [--tenant <id>] [--replace]
//! strata-corpus build --scale small|medium|large [--seed <n>] [--store <store>] [--tenant <id>]
//!                     [--replace] [--lenient] [--batch <n>] [--catalogue <file>]
//! strata-corpus hash  [--store <store>] [--tenant <id>]
//! strata-corpus check [--product <id>]
//!
//! --store   memory (the default) | sqlite:<control database file> | postgres[:<url>]
//!           postgres without a url reads STRATA_PG_URL.
//! --spec    the specification manifest (default: specifications/spec.jsonc of this checkout)
//! --corpus  the directory of product files (default: tests/corpus/enterprise)
//! ```
//!
//! `check` installs each product on a fresh in-memory tenant of its own, then all of them into one
//! tenant, and prints every problem the kernel reports for each, not only the first.
//! `load` installs the 14 products and checks that the tenant holds every entity they own.
//! `build` does that and then fills the tenant with records from the seed. `hash` prints the hash
//! of an existing tenant, for comparing two runs on a store that outlives the process.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;
use strata_corpus::load::resolve;
use strata_corpus::{
    build_with, compile_corpus, load_corpus, tenant_hash, verify_loaded, CorpusError, CorpusPaths,
    LoadOptions, PopulateOptions, Scale,
};
use strata_kernel::{TenantId, Timestamp};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const USAGE: &str = "usage:
  strata-corpus load  [--store <store>] [--tenant <id>] [--replace]
  strata-corpus build --scale small|medium|large [--seed <n>] [--store <store>] [--tenant <id>]
                      [--replace] [--lenient] [--batch <n>] [--catalogue <file>]
  strata-corpus hash  [--store <store>] [--tenant <id>]
  strata-corpus check [--product <id>]
  --store memory (default) | sqlite:<control database file> | postgres[:<url>]";

struct Args {
    command: String,
    store: String,
    tenant: String,
    scale: Scale,
    seed: u64,
    replace: bool,
    lenient: bool,
    batch: usize,
    catalogue: Option<PathBuf>,
    spec: Option<String>,
    corpus: Option<String>,
    product: Option<String>,
}

fn parse(mut raw: Vec<String>) -> Result<Args, String> {
    if raw.is_empty() {
        return Err("a command is needed".into());
    }
    let command = raw.remove(0);
    let mut args = Args {
        command,
        store: "memory".into(),
        tenant: "enterprise".into(),
        scale: Scale::Small,
        seed: 1,
        replace: false,
        lenient: false,
        batch: 5000,
        catalogue: None,
        spec: None,
        corpus: None,
        product: None,
    };
    let mut scale_given = false;
    let mut it = raw.into_iter();
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--store" => args.store = value("--store")?,
            "--tenant" => args.tenant = value("--tenant")?,
            "--scale" => {
                args.scale = value("--scale")?.parse()?;
                scale_given = true;
            }
            "--seed" => {
                args.seed = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed is a non-negative integer".to_string())?
            }
            "--batch" => {
                args.batch = value("--batch")?
                    .parse()
                    .map_err(|_| "--batch is a non-negative integer".to_string())?
            }
            "--catalogue" => args.catalogue = Some(PathBuf::from(value("--catalogue")?)),
            "--spec" => args.spec = Some(value("--spec")?),
            "--corpus" => args.corpus = Some(value("--corpus")?),
            "--product" => args.product = Some(value("--product")?),
            "--replace" => args.replace = true,
            "--lenient" => args.lenient = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if args.command == "build" && !scale_given {
        return Err("build needs --scale small|medium|large".into());
    }
    Ok(args)
}

/// The store the flag names.
fn open_store(spec: &str) -> Result<Box<dyn Store>, CorpusError> {
    let now = Timestamp(1_767_225_600_000);
    if spec == "memory" {
        return Ok(Box::new(MemoryStore::new()));
    }
    if let Some(path) = spec.strip_prefix("sqlite:") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(CorpusError::from_display)?;
        }
        let store = strata_storage_sqlite::SqliteStore::open(&path, now)
            .map_err(CorpusError::from_display)?;
        return Ok(Box::new(store));
    }
    if spec == "postgres" || spec.starts_with("postgres:") {
        let url = match spec.strip_prefix("postgres:") {
            Some(url) if !url.is_empty() => url.to_string(),
            _ => std::env::var("STRATA_PG_URL").map_err(|_| {
                CorpusError::new(
                    "postgres needs a url: --store postgres:<url>, or set STRATA_PG_URL",
                )
            })?,
        };
        let store = strata_storage_postgres::PostgresStore::connect(&url, now)
            .map_err(CorpusError::from_display)?;
        return Ok(Box::new(store));
    }
    Err(CorpusError::new(format!(
        "{spec:?} is not a store; use memory, sqlite:<file> or postgres[:<url>]"
    )))
}

fn run(args: Args) -> Result<(), CorpusError> {
    let store = open_store(&args.store)?;
    let store: &dyn Store = store.as_ref();
    let mut paths = CorpusPaths::default();
    if let Some(spec) = &args.spec {
        paths.spec = resolve(spec);
    }
    if let Some(dir) = &args.corpus {
        paths.dir = resolve(dir);
    }
    let load = LoadOptions {
        tenant: args.tenant.clone(),
        replace: args.replace,
        ..LoadOptions::default()
    };

    if args.command == "check" {
        let mut products = compile_corpus(&paths)?;
        if let Some(only) = &args.product {
            products.retain(|p| &p.id == only);
            if products.is_empty() {
                return Err(CorpusError::new(format!(
                    "no product {only:?} in the corpus"
                )));
            }
        }
        let reports = strata_corpus::check_corpus(&products, &load, &mut |r| {
            if r.problems.is_empty() {
                println!("{:<9} {:<14} ok", r.pass, r.product);
            } else {
                println!(
                    "{:<9} {:<14} {} problem(s)",
                    r.pass,
                    r.product,
                    r.problems.len()
                );
                for problem in &r.problems {
                    println!("    {problem}");
                }
            }
        })?;
        let total: usize = reports.iter().map(|r| r.problems.len()).sum();
        println!("{total} problem(s) in all");
        if total > 0 {
            return Err(CorpusError::new("the corpus does not install cleanly"));
        }
        return Ok(());
    }

    if args.command == "hash" {
        let tenant = TenantId::new(args.tenant.clone()).map_err(CorpusError::from_display)?;
        println!("{}", tenant_hash(store, &tenant)?);
        return Ok(());
    }

    let started = Instant::now();
    let products = compile_corpus(&paths)?;
    println!(
        "compiled {} products with specc in {:.1}s",
        products.len(),
        started.elapsed().as_secs_f64()
    );

    if args.command == "load" {
        let started = Instant::now();
        let loaded = load_corpus(store, &load, &products)?;
        let verification = verify_loaded(store, &loaded.tenant, &products)?;
        report_load(&loaded, &verification, started.elapsed().as_secs_f64());
        if !verification.missing.is_empty() {
            return Err(CorpusError::new(format!(
                "{} owned entities are missing, for example {}",
                verification.missing.len(),
                verification.missing[..verification.missing.len().min(5)].join(", ")
            )));
        }
        return Ok(());
    }

    let mut options = PopulateOptions::new(args.scale, args.seed);
    options.lenient = args.lenient;
    options.batch = args.batch;
    let started = Instant::now();
    let mut last = String::new();
    let built = build_with(store, &products, &load, &options, &mut |p| {
        if p.done == p.total && p.object != last {
            println!(
                "  {:<42} {:>8} records  ({} commits so far, {:.1}s)",
                p.object,
                p.total,
                p.commits,
                started.elapsed().as_secs_f64()
            );
            last = p.object.clone();
        }
    })?;
    let elapsed = started.elapsed().as_secs_f64();
    report_load(&built.loaded, &built.verification, 0.0);
    let catalogue = &built.catalogue;
    println!(
        "populated {} at scale {} with seed {}: {} records, {} commits, {} people, {:.1}s",
        built.tenant(),
        catalogue.scale,
        catalogue.seed,
        catalogue.records,
        catalogue.commits,
        catalogue.people,
        elapsed
    );
    let mut by_module: std::collections::BTreeMap<&str, u64> = Default::default();
    for t in catalogue.types.values() {
        *by_module.entry(t.module.as_str()).or_default() += t.ids.len() as u64;
    }
    for (module, count) in by_module {
        println!("  module {module:<14} {count:>8} records");
    }
    for (object, (count, why)) in &catalogue.rejections {
        println!("  REFUSED {object}: {count} records, first: {why}");
    }
    println!("tenant hash {}", built.hash);
    if let Some(file) = &args.catalogue {
        let text = serde_json::to_string_pretty(&catalogue.to_json())
            .map_err(CorpusError::from_display)?;
        std::fs::write(file, text).map_err(CorpusError::from_display)?;
        println!("catalogue written to {}", file.display());
    }
    Ok(())
}

fn report_load(
    loaded: &strata_corpus::LoadedCorpus,
    verification: &strata_corpus::Verification,
    seconds: f64,
) {
    println!(
        "installed {} products into tenant {}: {} entities owned, schema version {}, {} commits{}",
        loaded.products.len(),
        loaded.tenant,
        loaded.entities(),
        loaded.schema_version,
        loaded.commits,
        if seconds > 0.0 {
            format!(", {seconds:.1}s")
        } else {
            String::new()
        }
    );
    for p in &loaded.products {
        println!(
            "  {:<14} v{} {:>5} entities {:>2} segments (schema version {})",
            p.product, p.version, p.entities, p.segments, p.schema_version
        );
    }
    let kinds: Vec<String> = verification
        .by_kind
        .iter()
        .map(|(kind, n)| format!("{kind} {n}"))
        .collect();
    println!(
        "verified {} owned entities present ({} missing, {} not checkable): {}",
        verification.owned,
        verification.missing.len(),
        verification.unchecked,
        kinds.join(", ")
    );
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("strata-corpus: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if !["load", "build", "hash", "check"].contains(&args.command.as_str()) {
        eprintln!("strata-corpus: unknown command {:?}\n{USAGE}", args.command);
        return ExitCode::from(2);
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("strata-corpus: {error}");
            ExitCode::FAILURE
        }
    }
}
