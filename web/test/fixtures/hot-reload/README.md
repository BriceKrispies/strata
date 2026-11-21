# Hot reload fixtures

The product family the hot reload scenarios run against, and the tenant interface definition each
version of it gives.

- Product files: `crates/spec-compiler/tests/fixtures/products/hot-reload/` holds
  `service-desk-v1.jsonc` to `service-desk-v6.jsonc` (each file names its change in a comment at
  the top) and `knowledge-base-v1.jsonc`.
- Bundles: `crates/spec-compiler/tests/fixtures/products/expected/hot-reload/<name>.bundle.json`,
  one per product file, as `specc bundle` writes them.
- Golden files: `service-desk-v<n>/manifest.json` and `service-desk-v<n>/parts/<hash>.json` in this
  directory, the manifest and parts of the tenant after knowledge-base v1 and service-desk v1 are
  installed and service-desk is upgraded to v<n>.

`specc bundle` names its output `<product id>.bundle.json`, so every service-desk version would
land on one file name. Write each into its own directory and rename it:

```
for f in service-desk-v1 service-desk-v2 service-desk-v3 service-desk-v4 service-desk-v5 service-desk-v6 knowledge-base-v1; do
  specc bundle --product crates/spec-compiler/tests/fixtures/products/hot-reload/$f.jsonc \
    --spec specifications/spec.jsonc --out target/hot-reload-bundle
  mkdir -p crates/spec-compiler/tests/fixtures/products/expected/hot-reload
  mv target/hot-reload-bundle/service-desk.bundle.json crates/spec-compiler/tests/fixtures/products/expected/hot-reload/$f.bundle.json 2>/dev/null \
    || mv target/hot-reload-bundle/knowledge-base.bundle.json crates/spec-compiler/tests/fixtures/products/expected/hot-reload/knowledge-base.bundle.json
done
```

(The test reads `knowledge-base.bundle.json` and `service-desk-v<n>.bundle.json`.)

Then write the golden files:

```
STRATA_BLESS=1 cargo test -p strata-substrate --test substrate_it hot_reload_fixtures::
```

Without `STRATA_BLESS=1` the same test compares every file byte for byte and fails on a difference
or on a part file the definition no longer has.
