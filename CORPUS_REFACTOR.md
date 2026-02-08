# Corpus Refactor Plan

## Context

The `djls-corpus` crate was refactored to unify predicates, use camino paths, add a `FileKind` enum, and deduplicate helpers (`synced_dirs`, `find_latest_django`, `extract_file`). But the refactor stopped short — it over-engineered predicate sharing and under-engineered the actual domain model. This document captures what still needs to happen.

## Current State (after initial refactor)

- `FileKind` enum + `enumerate_files(root, kind)` replaces two functions
- Shared predicates between `sync.rs` and `enumerate.rs`
- `synced_dirs`, `find_latest_django`, `extract_file` moved from test files into corpus lib
- Camino paths throughout
- Workspace deps fixed
- HTTP client threaded through sync functions

## What's Wrong

### 1. No `Corpus` struct — primitive obsession

`find_corpus_root()` returns a bare `Utf8PathBuf`. Every caller passes it around to free functions. This is the primitive obsession anti-pattern (m15). The invariant "this directory exists and has corpus structure" should be encoded in a type (m05-type-driven).

**Should be:**
```rust
pub struct Corpus { root: Utf8PathBuf }

impl Corpus {
    pub fn discover() -> Option<Self>;
    pub fn root(&self) -> &Utf8Path;
    pub fn latest_django(&self) -> Option<Utf8PathBuf>;
    pub fn synced_dirs(&self, relative: &str) -> Vec<Utf8PathBuf>;
    pub fn extraction_targets(&self) -> Vec<Utf8PathBuf>;
    pub fn templates(&self) -> Vec<Utf8PathBuf>;
    pub fn extract_file(&self, path: &Utf8Path) -> Option<ExtractionResult>;
    pub fn extract_all(&self) -> Vec<(Utf8PathBuf, ExtractionResult)>;
    pub fn build_specs(&self, dir: &Utf8Path) -> (TagSpecs, FilterAritySpecs);
    pub fn module_path(&self, file: &Utf8Path) -> String;
}
```

Callers become declarative:
```rust
let corpus = Corpus::discover()?;
let files = corpus.extraction_targets();
let django = corpus.latest_django()?;
```

### 2. Too much deferred to callers

Corpus layout knowledge is still scattered across test files. Building `TagSpecs` from extraction, finding Django dirs, iterating version dirs — all this should be `Corpus` methods. Tests should call high-level corpus operations, not re-implement navigation.

### 3. CLI doesn't use clap

The binary uses bare `args.get(1)` matching on two subcommands. Clap is already a workspace dep. Should have proper `--help`, and potentially `--manifest`, `--root` flags.

### 4. `corpus.rs` and `golden.rs` in djls-extraction are the same idea

- `corpus.rs` — runs extraction on every corpus file, checks no panics + results exist
- `golden.rs` — runs extraction on specific Django modules, snapshots results

These should be one test module. The golden tests are just corpus tests that snapshot specific entries. The 18 nearly-identical `test_*_full_snapshot` functions each do `find_corpus_root → find_latest_django → read one file → extract → snapshot`. Should be a single parameterized loop, or just enumerate everything and snapshot it all.

### 5. Hardcoded per-file tests add no value

`test_defaulttags_full_snapshot`, `test_i18n_full_snapshot`, etc. are boilerplate. Enumerate the corpus files, extract all, snapshot all. The per-module assertions (`test_defaulttags_for_tag_rules`, `test_defaulttags_if_tag`, etc.) test specific extraction correctness and have value, but they should be driven by data, not copy-pasted functions.

### 6. Server corpus tests belong in semantic (or corpus)

`crates/djls-server/tests/corpus_templates.rs` validates templates against extracted rules end-to-end. It doesn't test server functionality — it tests the extraction → semantic validation pipeline. It builds a `CorpusTestDatabase` implementing Salsa `Db` traits and calls `validate_nodelist`. That's a `djls-semantic` concern.

`djls-semantic/src/lib.rs` already has similar corpus tests with its own test database. These should be consolidated. With a `Corpus` struct that owns `build_specs()`, the test database could live in corpus itself (behind the `extraction` feature gate).

### 7. Deps structure

`djls-corpus` is never linked into the final `djls` binary — it's only used from test code (`[dev-dependencies]`) and its own CLI binary. The current dep structure is fine for this, but consider:

- The `extraction` feature pulls `djls-extraction` + Ruff into the library. This is only used by dev-dep consumers, so the weight is acceptable.
- The sync-specific deps (`reqwest`, `flate2`, `tar`) are needed by the binary but also get compiled when tests pull in `djls-corpus`. This is unavoidable without splitting the crate, which isn't worth it.

## Implementation Order

1. **Add `Corpus` struct** with discovery + all navigation/enumeration/extraction methods
2. **Use clap** for the CLI binary
3. **Merge `corpus.rs` + `golden.rs`** in djls-extraction into one data-driven test module
4. **Move server corpus tests** to djls-semantic or djls-corpus
5. **Eliminate hardcoded per-file test functions** — replace with enumeration + snapshot
6. **Remove free functions** that are now `Corpus` methods (keep as private helpers if needed)

## Design Principles (from loaded but ignored skills)

- **m05-type-driven**: Encode "valid corpus directory" in a type, not a bare PathBuf
- **m09-domain**: Corpus is a value object identified by root path; methods enforce invariants
- **m15-anti-pattern**: Free functions with shared root parameter = primitive obsession; struct methods fix this
