# Testing Strategy

Goal: find bugs before users report them, instead of hand-writing cases or waiting for an issue about a specific template, library, or project layout.

## The hard parts

DJLS has four parts that no off-the-shelf tool tests for us:

1. **Extraction**: deriving tag rules, block structure, filter arities, and settings from Python source with the static evaluator in `djls-project`.
2. **Validation**: checking templates against those rules and producing diagnostics in `djls-semantic`.
3. **Project model**: evaluating settings and resolving apps, library names, builtins, and template directories across project layouts and environments.
4. **Incrementality**: staying correct as files change, through Salsa invalidation.

One concern crosses all four: knowing when DJLS does not know. Extraction, resolution, and validation can all end Inconclusive, and Inconclusive must spread exactly as far as it should. Issue #974 was a load of an unknown library that made every later tag Inconclusive, so structural diagnostics went silent.

The rest (LSP plumbing, CLI, config) is conventional and keeps conventional tests.

## Known baseline noise

The corpus contains files `djls check` diagnoses that are not Django templates: TinyMCE `.htm` files under grappelli's `static/`, Jinja2 templates in django-compressor and Django, and Sphinx `layout.html` themes in django-imagekit and pretix. They are deferred until checking follows the configured template backends, including Jinja2. The ecosystem diff shows only changes, so they stay in the baseline and surface only when a change affects how they are diagnosed.

## Oracles

A generated test needs something that knows the right answer.

- **Django.** It is the spec for extraction, validation, and the project model. `tools/django_facts.py` already runs Django to produce ground truth (`tests/fixtures/django-facts/`), and `django_compilation.rs` compares DJLS to it for 194 hand-written templates.
- **CPython.** It is the spec for the static Python evaluator.
- **Shipped code.** Templates in released projects compile, so a DJLS error on one is almost certainly a DJLS bug.
- **DJLS itself.** Metamorphic and consistency properties compare DJLS to itself under transformations, with no external oracle.
- **The previous DJLS release.** Any diagnostic change across the corpus is a behavior change to review.

## The master property

For any project, libraries, and template: **if DJLS reports an error, Django rejects the template.**

- A false positive fails the build.
- A missed diagnostic is a tracked rate, not a failure.

`FALSE_POSITIVES` and `MISSED_DIAGNOSTICS` in `crates/djls-semantic/tests/it/django_compilation.rs` already work this way. Generated inputs turn them into a scoreboard over thousands of cases.

## The board

Status: **exists**, **partial**, or **new**.

### Find wrong answers

| Strategy | Finds | Oracle | Cost | Status |
|---|---|---|---|---|
| Django differential | Extraction, validation, and project-model answers that disagree with Django | Django | Medium | Partial: hand-written inputs only |
| CPython differential | Static evaluator results that disagree with Python (past fixes: #779, #780, #781, #964) | CPython | Medium | New |
| Ecosystem diff per PR | Any diagnostic change across the corpus, posted as a PR diff for review | Base branch build | Low | Exists: `just ecosystem`, `tools/ecosystem.py`, `.github/workflows/ecosystem.yml` |
| Shipped templates compile | False positives on real projects | Shipped code | Low | Partial: `corpus_validation.rs` checks argument errors with projectless specs |
| Metamorphic properties | Unsound results where no oracle exists | DJLS | Low to medium | New |
| Cross-feature consistency | Completion offers a tag that validation calls unknown; hover and diagnostics disagree; `djls check` and the LSP disagree on one file | DJLS | Low | New |
| Django version matrix | Behavior that differs between Django versions | Each bundled Django | Low | Partial: `just testall` |

Metamorphic properties to start with:

- Extraction is invariant under refactoring: renaming variables, extracting a helper, inverting an `if`, or aliasing `register` yields the same rules or less precise ones, never contradictory ones.
- Making a library unknown can only shrink the error set.
- Inserting an unrelated line, including a malformed tag, does not remove other diagnostics.
- A text prefix shifts spans and changes nothing else.
- Reordering unrelated settings changes nothing.

### Find inputs nobody wrote

| Strategy | Finds | Cost | Status |
|---|---|---|---|
| Grammar generators (Hypothesis or proptest) | Combinations of idioms and layouts: compile-function shapes, tag invocations, template mutations, project layouts | Medium: the main investment | New |
| Corpus mutation | Realistic near-misses from edited real libraries and templates | Low | New |
| Automatic corpus growth | Unseen libraries and layouts mined from top Django packages on PyPI and GitHub `templatetags/` | Low to medium | New |
| LLM-written adversarial libraries | Plausible styles the grammar does not cover; an agent writes them and Django judges them | Low | New |
| Coverage-guided fuzzing (cargo-fuzz, structured inputs) | Crashes, hangs, and runaway growth in the evaluator and template parser (past fix: #778) | Low | New |

Generator targets:

- **Extraction**: `split_contents`, `len()` checks, `pop`, `parse(("end…",))`, keyword arguments, simple and inclusion tags, helpers, class-based tags.
- **Validation**: templates valid by construction, then one mutation each: drop a closer, misnest, wrong argument count, unknown load, malformed tag.
- **Project model**: app shapes, namespace packages, `libraries` and `builtins` options, duplicate library names, `DIRS` order, `APP_DIRS`, settings split across star imports. The oracle is `django.setup()` followed by reading `engine.template_libraries` and template resolution.

### Test the tests

| Strategy | Finds | Cost | Status |
|---|---|---|---|
| Mutation testing (cargo-mutants) on extraction and validation | Code no test checks, and tests that never fail | Low | Exists: `just mutants -p <crate> -f <file>`, run by hand |
| Debug assertions under fuzzing | Broken internal invariants such as out-of-bounds spans or inconsistent scope state | Low | New |

### Correctness over time and under stress

| Strategy | Finds | Cost | Status |
|---|---|---|---|
| Incremental equals from scratch | Stale Salsa results (past fixes: #278, #411, #652, #665) | Low | New |
| Stateful session model | Ordering bugs between open, change, reload, cancel, and shutdown (past fixes: #434, #675, #914, #945) | Medium | New |
| Chaos inputs | Panics or false positives on half-written, unreadable, non-UTF-8, or symlinked files, settings syntax errors, a missing interpreter. Expected: no panic, no false positive, Inconclusive. | Low | New |
| Performance properties | Pathological inputs, found with time and memory budgets under fuzzing | Low | Partial: benchmarks exist |
| Formatter idempotence and round-trip | `format(format(x)) == format(x)`, and only whitespace tokens change | Low | New |
| Document edits and position encoding | Random edit sequences match a plain string splice; offset and position round-trip in every encoding | Low | Partial: example tests in `document.rs` and `line.rs` |

### Formal methods

- **TLA+**: does not fit. The hard parts are pure functions of their inputs, not protocols. The one protocol, reload and diagnostic publishing, is better covered by the stateful session model, which runs the real code.
- **Kani**: could prove small pure functions such as position-encoding conversion. Low priority.
- **Architecture tests**: `django_knowledge_guard` and `just hawk` already exist.

## How it runs

- **Every PR, Rust only**: metamorphic properties, incremental equals from scratch, chaos inputs, formatter and position properties, with small budgets inside `cargo test`.
- **Nightly or on demand, with Python**: generate a batch of projects, run `django_facts.py` and DJLS once per batch, and diff. Running a process per case is too slow, since a debug `djls check -` takes about 1.9 s. Hypothesis fits the generators because the oracle is Python and it shrinks failures well.
- **Findings**: every shrunk disagreement becomes a permanent case in an mdtest or in `tests/compilation/templates`, or an entry in the scoreboard lists.

## Development workflow

Supporting a new idiom means adding it to a generator and running the suite. Django supplies the expected answer, disagreements shrink to a minimal library and template, and the agreement rate per idiom shows progress.

Tests that generated suites can replace:

- Many hand-written evaluator cases in `djls-project`, once the CPython differential exists.
- Many invalidation cases, once incremental equals from scratch exists.
- Most per-tag mdtests in `crates/djls-semantic/resources/mdtest/tags/`, keeping a few per tag as documentation.
- The example tests for document edits and position encoding.

Keep the mdtests (they are the spec and the docs), corpus snapshots, and bundled-Django extraction snapshots.

## Order

1. Ecosystem diff per PR and mutation testing: cheap, and they reuse the corpus and the existing suite.
2. Incremental equals from scratch, and the CPython differential: the most frequent bug classes so far.
3. Django differential with generated and mutated inputs: the largest payoff and the largest build.
4. Everything else on the board, by cost.
