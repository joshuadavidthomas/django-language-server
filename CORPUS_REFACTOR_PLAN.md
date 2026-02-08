# Corpus Refactor Plan

## Summary

Replace free functions with a `Corpus` struct, add clap CLI, merge duplicated test files, and consolidate the three separate `CorpusTestDatabase` definitions scattered across crates.

## Current State

### `djls-corpus` lib

Five free functions all taking `&Utf8Path`:

- `find_corpus_root()` → `Option<Utf8PathBuf>`
- `synced_dirs(parent)` → `Vec<Utf8PathBuf>`
- `find_latest_django(corpus_root)` → `Option<Utf8PathBuf>`
- `module_path_from_file(file)` → `String`
- `extract_file(path)` → `Option<ExtractionResult>` (feature-gated)

Plus `enumerate::enumerate_files(root, kind)` and `manifest::Manifest::load()`.

### CLI (`main.rs`)

Bare `args.get(1)` matching on `"sync"` and `"clean"`. No `--help`, no flags.

### Test Duplication

Four separate locations with overlapping corpus test code:

1. **`djls-extraction/tests/corpus.rs`** — 4 tests calling `find_corpus_root()` + free functions
2. **`djls-extraction/tests/golden.rs`** — 18 tests, 8 copy-pasted full-snapshot tests doing `find_corpus_root → find_latest_django → read file → extract → snapshot`
3. **`djls-server/tests/corpus_templates.rs`** — 3 tests with its own `CorpusTestDatabase`, `build_specs_from_extraction()`, `validate_templates_in_dir()`
4. **`djls-semantic/src/lib.rs`** (test module, ~600 lines) — its own `CorpusTestDatabase`, `find_django_source()`, `build_extraction_specs()`, `build_extraction_arities()`, `collect_template_files()`, `collect_validation_errors()`, plus 4 corpus tests

The semantic and server tests define near-identical `CorpusTestDatabase` structs implementing the same Salsa traits.

### The venv `find_django_source()` path

`djls-semantic`'s test module has `find_django_source()` which looks in `.venv/lib/*/site-packages/django`. This predates the corpus — it was the original way to get Django source (commit `2f04e379`, "M8 Phase 6"). The corpus already pins multiple Django versions, making the venv path redundant. Kill it; use corpus everywhere.

## Design Principles

**m05 (type-driven):** "This directory exists and has corpus structure" is an invariant. Validate once at construction, trust forever. Bare `Utf8PathBuf` is primitive obsession.

**m09 (domain):** Corpus is a value object identified by root path. Navigation logic belongs on the type, not scattered across test files.

**m15 (anti-pattern):** Free functions sharing a root parameter = primitive obsession. The `find_corpus_root() → pass to N functions` pattern in every test is the textbook example.

**m12 (lifecycle):** Corpus is cheap to construct (path validation only). No Drop, no pool, no lazy init. Simple validated-at-construction value object.

## Step 1: `Corpus` struct

Add to `djls-corpus/src/lib.rs`:

```rust
pub struct Corpus {
    root: Utf8PathBuf,
}

impl Corpus {
    /// Discover corpus from env var or default location.
    pub fn discover() -> Option<Self>;

    /// Construct from a known path, validating it exists.
    pub fn from_path(root: Utf8PathBuf) -> Option<Self>;

    pub fn root(&self) -> &Utf8Path;

    /// Latest synced Django version directory.
    pub fn latest_django(&self) -> Option<Utf8PathBuf>;

    /// Synced subdirectories under a relative path (e.g. "packages/Django").
    pub fn synced_dirs(&self, relative: &str) -> Vec<Utf8PathBuf>;

    /// All extraction target files in the entire corpus.
    pub fn extraction_targets(&self) -> Vec<Utf8PathBuf>;

    /// All template files in the entire corpus.
    pub fn templates(&self) -> Vec<Utf8PathBuf>;

    /// Enumerate files of a given kind under a specific subdirectory.
    pub fn enumerate_files(&self, dir: &Utf8Path, kind: FileKind) -> Vec<Utf8PathBuf>;

    /// Extract rules from a single file. (feature = "extraction")
    pub fn extract_file(&self, path: &Utf8Path) -> Option<ExtractionResult>;

    /// Extract and merge all extraction targets under a directory. (feature = "extraction")
    pub fn extract_dir(&self, dir: &Utf8Path) -> ExtractionResult;
}
```

`module_path_from_file()` stays as a standalone free function — it's a pure path-to-module conversion that doesn't need corpus context.

`build_specs` stays OUT of `Corpus` — `TagSpecs`/`FilterAritySpecs` live in `djls-semantic` and adding that dep would create coupling. Callers convert `ExtractionResult` to specs themselves.

Remove the old free functions (`find_corpus_root`, `synced_dirs`, `find_latest_django`, `extract_file`) — no deprecation period, just update all callers in the same PR.

## Step 2: Clap CLI

Add `clap` to `djls-corpus/Cargo.toml` deps. Replace bare arg matching:

```rust
#[derive(Parser)]
#[command(name = "djls-corpus", about = "Manage the Django template corpus")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to manifest file
    #[arg(long)]
    manifest: Option<Utf8PathBuf>,

    /// Override corpus root directory
    #[arg(long)]
    root: Option<Utf8PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Download and extract corpus packages/repos
    Sync,
    /// Remove all synced corpus data
    Clean,
}
```

## Step 3: Merge extraction tests

Merge `djls-extraction/tests/corpus.rs` and `djls-extraction/tests/golden.rs` into one file: `djls-extraction/tests/corpus.rs`. Delete `golden.rs`.

### Data-driven snapshots replace copy-paste

The 8 per-module snapshot tests (`test_defaulttags_full_snapshot`, `test_defaultfilters_full_snapshot`, `test_loader_tags_full_snapshot`, `test_i18n_full_snapshot`, `test_static_full_snapshot`, `test_cache_full_snapshot`, `test_l10n_full_snapshot`, `test_tz_full_snapshot`) become one test that enumerates Django core modules and snapshots each. Let insta handle the rename — old snapshot files get cleaned up.

The 2 third-party snapshot tests (`test_wagtail_extraction_snapshot`, `test_allauth_extraction_snapshot`) become one test enumerating all non-Django packages.

### Keep specific-assertion tests

These test specific extraction correctness and stay as individual functions:

- `test_defaulttags_for_tag_rules`
- `test_defaulttags_if_tag`
- `test_defaulttags_url_tag_rules`
- `test_defaulttags_with_tag`
- `test_loader_tags_block_tag`
- `test_defaulttags_tag_count`
- `test_defaultfilters_filter_count`
- `test_for_tag_rules_across_django_versions`

### Resulting test list

```
tests/corpus.rs
  // Corpus-wide
  test_corpus_extraction_no_panics
  test_corpus_extraction_yields_results
  test_corpus_unsupported_patterns_summary

  // Snapshots (data-driven)
  test_django_core_modules_snapshots        (replaces 8 tests)
  test_third_party_packages_snapshots       (replaces 2 tests)
  test_django_versions_extraction           (existing, snapshot)

  // Specific assertions (kept)
  test_defaulttags_tag_count
  test_defaulttags_for_tag_rules
  test_defaulttags_if_tag
  test_defaulttags_url_tag_rules
  test_defaulttags_with_tag
  test_defaultfilters_filter_count
  test_loader_tags_block_tag
  test_for_tag_rules_across_django_versions
```

All using `Corpus::discover()`.

## Step 4: Consolidate semantic/server corpus tests

### Kill `find_django_source()`

The venv-based Django discovery in `djls-semantic`'s test module is legacy from before the corpus existed. The corpus pins multiple Django versions and is the single source of truth. Remove `find_django_source()` and all code paths that depend on it.

### Move server tests into semantic

`djls-server/tests/corpus_templates.rs` tests semantic validation, not server functionality. It builds a `CorpusTestDatabase`, extracts rules, validates templates — that's `djls-semantic`'s domain.

Move these 3 tests into `djls-semantic`'s test module:
- `test_django_shipped_templates_zero_false_positives`
- `test_third_party_templates_zero_arg_false_positives`
- `test_repo_templates_zero_arg_false_positives`

### Deduplicate `CorpusTestDatabase`

Three identical definitions exist today (semantic, server, plus the helpers around them). After consolidation, one definition in `djls-semantic`'s test module.

### Remove `djls-corpus` dev-dep from `djls-server`

Once the tests move out, `djls-server` no longer needs `djls-corpus`.

### Reconcile template filtering

The semantic tests have `collect_template_files()` with extra exclusions (`jinja2/`, `static/`) that `enumerate::FileKind::Template` doesn't have. Add those exclusions to `FileKind::Template` filtering in `enumerate.rs` — they're correct for all consumers (Jinja2 templates aren't Django templates, `static/` dirs contain JS templates).

## Step 5: Clean up enumerate visibility

The `pub(crate)` predicate helpers (`in_pycache`, `has_py_extension`, etc.) stay `pub(crate)` — used by both `enumerate.rs` and `sync.rs`. `enumerate_files` stays `pub` as the implementation behind `Corpus` methods, still usable directly for subdirectory enumeration.

## Implementation Order

| Step | Description | Status |
|------|-------------|--------|
| 1 | `Corpus` struct + methods | ✅ Done |
| 2 | Merge extraction tests, migrate to `Corpus` | ✅ Done |
| 3 | Consolidate semantic/server tests, kill venv path | ✅ Done |
| 4 | Clap CLI | ✅ Done |
| 5 | Enumerate cleanup | ✅ Done |

Build and test after each step. Run `cargo insta test --accept --unreferenced delete` after steps that change snapshot names.

## Snapshot Strategy

Old snapshot files from `golden__*.snap` will become orphaned when the test functions that reference them are deleted. `cargo insta review` does NOT clean these up — it only handles pending `.snap.new` files. After merging the tests:

1. `cargo insta test --accept --unreferenced delete` — runs tests, accepts new snapshots, deletes orphaned `.snap` files in one pass
2. Verify no stale `golden__*.snap` files remain in `crates/djls-extraction/tests/snapshots/`
