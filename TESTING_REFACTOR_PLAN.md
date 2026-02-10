# Testing Refactor Plan (Revised)

## Problems

### 1. Smoke test iterates over Python files, not templates

`crates/djls-semantic/tests/template_library_smoke.rs` uses `datatest-stable` to iterate over Python files
matching `templatetags/*.py` and `template/default*.py`. For each `.py` file, it:

1. Walks *up* to find the corpus entry directory.
2. Extracts rules from *all* Python files in that entry.
3. Validates *all* templates in that entry.

This means every corpus entry gets tested N times where N is its number of Python
files. The numbers are bad:

- 314 `.py` files trigger 314 test runs across only 44 unique entries
- `pretix` has 32 py targets → its 451 templates get validated 32 times identically
- `wagtail-7.3` has 11 py targets → its 485 templates validated 11 times
- `horizon` has 14 py targets → its 467 templates validated 14 times

The smoke test should be driven by templates (the real validation inputs) and should
build extraction specs once per corpus entry.

### 2. Version comparison test is redundant and misleading

`test_django_versions_extraction` in `djls-python/tests/corpus.rs` snapshots a
*summary* (counts + tag names) for `defaulttags.py` across Django versions. Problems:

- Only tests one file (`defaulttags.py`) — ignores `defaultfilters.py`, `loader_tags.py`, `i18n.py`, etc.
- Shows 13 `tag_rules` but the file has 23 registered tags — the 10 missing ones produce `block_specs` only (or neither), which the summary silently hides
- The per-file extraction snapshots already cover every Django version's `defaulttags.py` with full detail
- The summary adds no information that the per-file snapshots don't already have

### 3. Extraction snapshot test reimplements corpus filtering

`extraction_snapshots` in `djls-python/tests/corpus.rs` uses `insta::glob!("**/*.py")`
with a hand-rolled `is_extraction_target()` predicate. `Corpus::extraction_targets()`
already does this. The two implementations have diverged.

### 4. ~8 identical `TestDatabase` implementations in djls-semantic

Every `#[cfg(test)]` module in `djls-semantic/src/` defines its own `TestDatabase`
struct with the same Salsa boilerplate (~50-60 lines each). Plus another copy in
`template_library_smoke.rs`.

All implement the same 4 traits (`salsa::Database`, `djls_source::Db`,
`djls_templates::Db`, `djls_semantic::Db`) with nearly identical code. The only
variations are which fields they store and how `tag_specs()` /
`filter_arity_specs()` / `template_libraries()` return them.

### 5. Test helper functions copy-pasted across modules

`builtin_tag_json()`, `library_tag_json()`, `make_inventory()`, etc. are defined
independently in multiple modules.

### 6. Corpus smoke test and corpus unit test duplicate extraction/validation logic

`template_library_smoke.rs` and the `#[cfg(test)]` corpus tests in
`crates/djls-semantic/src/lib.rs` both implement:

- `extract_and_merge()`
- `build_specs_from_extraction()`
- `is_argument_validation_error()`
- `validate_template()`

with near-identical code.

## Guiding principles

- **Smoke tests (corpus-driven)** should be broad, deterministic, and *quiet when passing*.
  They exist to guard against regressions (especially false positives) at scale.
- **Snapshot tests (snippet-driven)** should be small and precise, and should render
  input + output together so failures are immediately actionable.

## Plan

### Phase 1: Shared test database + helpers in djls-semantic

Create `crates/djls-semantic/src/testing.rs` behind `#[cfg(test)]`:

- `TestDatabase` (single Salsa db implementation used by all djls-semantic unit tests)
- Shared JSON helper builders (`builtin_tag_json`, `library_tag_json`, etc.)
- Shared validation helpers (`collect_errors`, etc.)

Important: this module is compiled only for **unit tests**. The smoke test will be
moved into unit tests as well (Phase 2), so `#[cfg(test)]` is sufficient.

Sketch:

```rust
#[cfg(test)]
pub(crate) mod testing {
    #[salsa::db]
    #[derive(Clone)]
    pub(crate) struct TestDatabase {
        storage: salsa::Storage<Self>,
        fs: Arc<Mutex<InMemoryFileSystem>>,
        specs: TagSpecs,
        arity_specs: FilterAritySpecs,
        template_libraries: TemplateLibraries,
    }

    impl TestDatabase {
        pub(crate) fn new() -> Self { /* defaults */ }
        pub(crate) fn with_specs(self, specs: TagSpecs) -> Self { /* ... */ }
        pub(crate) fn with_arity_specs(self, arities: FilterAritySpecs) -> Self { /* ... */ }
        pub(crate) fn with_template_libraries(self, libs: TemplateLibraries) -> Self { /* ... */ }
        pub(crate) fn add_file(&self, path: &str, content: &str) { /* ... */ }
        pub(crate) fn create_file(&self, path: &Utf8Path) -> File { /* ... */ }
    }
}
```

Then update every unit test module in `djls-semantic/src/` to use
`crate::testing::TestDatabase` and delete the local copies.

Optional follow-up:
- Consider extracting shared test utilities into a dedicated workspace crate
  (e.g. `djls-testing`, `djls-testutils`, or `djls-test`) so other crates
  (`djls-python`, `djls-project`, `djls-server`, etc.) can reuse a single
  canonical snapshot renderer / test DB helpers without copy-paste.
  This can stay as `#[cfg(test)]`-only code or be a normal crate depending on
  whether we want integration tests to depend on it.

### Phase 2: Rewrite the corpus smoke test (move to unit tests, template-driven)

The smoke test does not need to be a Cargo integration test.

- Delete `crates/djls-semantic/tests/template_library_smoke.rs`
- Remove `datatest-stable` usage and the `[[test]] harness = false` entry
- Replace with a normal `#[test]` inside `djls-semantic` unit tests

The new smoke test is:

- **Input discovery is templates**: find all templates under the corpus root
- **Work is grouped by corpus entry**: build specs once per entry, validate all
  templates under that entry

Pseudo-code:

```rust
#[test]
fn corpus_templates_have_no_argument_false_positives() {
    let corpus = Corpus::require();

    // Discover templates first (the real inputs).
    let templates = corpus.templates_in(corpus.root());

    // Group templates by entry dir (packages/<entry> or repos/<entry>).
    let by_entry: HashMap<Utf8PathBuf, Vec<Utf8PathBuf>> = group_by_entry(&corpus, templates);

    let mut failures = Vec::new();

    for (entry_dir, templates) in by_entry {
        if templates.is_empty() {
            continue;
        }

        let (specs, arities) = build_entry_specs(&corpus, &entry_dir);

        for template_path in templates {
            let Ok(content) = std::fs::read_to_string(template_path.as_std_path()) else {
                continue;
            };

            let errors = validate_template(&content, &specs, &arities);
            if !errors.is_empty() {
                failures.push(FailureEntry { path: template_path, errors: format_errors(&errors) });
            }
        }
    }

    assert!(failures.is_empty(), "Corpus templates have false positives:\n{}", format_failures(&failures));
}
```

Notes:

- This achieves the “template-driven” requirement without the pathological
  “rebuild specs per template” cost.
- This test remains “quiet when passing”.
- When failing, it produces a short, deterministic failure summary.

### Phase 3: Add small corpus helpers (optional)

This phase is optional. The smoke test can derive entry dirs locally.

If we want to standardize this, add to `djls-corpus`:

- `Corpus::entry_dir_for_path(path: &Utf8Path) -> Option<Utf8PathBuf>`
- `Corpus::is_django_entry(entry_dir: &Utf8Path) -> bool`

These helpers should be used by the smoke test and any future corpus-driven
checks.

### Phase 4: Consolidate corpus extraction/merge/validation helpers

Move the duplicated logic into `crates/djls-semantic/src/testing.rs` (unit-test
only), so the smoke test and other corpus-based tests share exactly one
implementation:

- `extract_and_merge(corpus, dir, specs, arities)`
- `build_entry_specs(corpus, entry_dir) -> (TagSpecs, FilterAritySpecs)`
- `validate_template(content, specs, arities) -> Vec<ValidationError>`
- `is_argument_validation_error(err)`

This removes the duplicate copies in `djls-semantic/src/lib.rs` tests.

### Phase 5: Clean up djls-python corpus tests

- Delete `test_django_versions_extraction` (redundant).
- Update `extraction_snapshots` to use `Corpus::extraction_targets()`.

If snapshot naming changes, accept one-time churn:

```bash
cargo insta test --accept --unreferenced delete
```

### Phase 6: Input-output diagnostic snapshots for snippet tests

For snippet tests (not the corpus smoke test), introduce a renderer that
includes the template source and rendered diagnostics together in the snapshot.

- Put renderer code in `crates/djls-semantic/src/testing.rs` (unit-test only)
- Use `djls_source::LineIndex` to map byte offsets → line/col

Later enhancement ideas (for prettier snapshots):
- Prefer a Ruff/ty-style **markdown snapshot format**: template source first, then rendered diagnostics.
- Explore text renderers:
  - `annotate-snippets` (preferred; matches Ruff/ty)
  - `miette`
  - `ariadne`

Add helper:

```rust
pub(crate) fn snapshot_validate(source: &str) {
    let errors = validate_snippet(source);
    let rendered = render_diagnostic_snapshot("test.html", source, &errors);
    insta::assert_snapshot!(rendered);
}
```

Then convert assertion-heavy tests in:

- `lib.rs` tests
- `filters/validation.rs`
- `loads/validation.rs`

from hand-written `matches!` assertions to `snapshot_validate(...)`.

## Order of execution

1. Phase 1 — create shared `testing.rs` + migrate one unit test module at a time
2. Phase 2 — move/rewrite the smoke test as a unit test; remove `datatest-stable`
3. Phase 4 — consolidate corpus helpers (extract/build/validate) into `testing.rs`
4. Phase 5 — simplify `djls-python` corpus snapshot tests
5. Phase 6 — implement diagnostic renderer + adopt snapshot helpers
6. Phase 3 — (optional) add corpus helper APIs to `djls-corpus` if useful elsewhere

Each phase is intended to be a standalone commit.
