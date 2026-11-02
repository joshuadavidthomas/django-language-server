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

Two parts carry the most risk and get the most investment:

- **The static Python evaluator.** It is hand-written static evaluation of Python that must be fast and correct, and extraction and the project model both stand on it. CPython is a complete oracle for it, so differential testing and fuzzing with time and node budgets come first.
- **The template parser.** It faces unbounded input: every template anyone has written or is halfway through typing. Fuzzing keeps a place on the board even when it is not the current focus. The question to keep asking is which inputs would find bugs we cannot reach by hand.

## Findings so far

What the first strategies found, and what that teaches:

| Strategy | PR | Bugs found |
|---|---|---|
| Ecosystem diff, mutation testing | #986 | Baseline noise only (see below). Mutants found unchecked code in validation, covered by new mdtests in #988. |
| Incremental equals from scratch | #987 | None so far. |
| CPython differential | #989 | Three evaluator bugs: exponential growth of namespace causes at branch joins (hangs on settings shaped like Horizon's); a mutation through one alias of a list or dict not reaching the other alias (fix in #990); loop arms reached only on a later iteration never evaluated (fix in progress). |

Lessons:

- **Comparing against a reference finds bugs; self-consistency checks have not yet.** The CPython differential found three real bugs in its first runs. Weight the next work toward oracles: CPython for the evaluator, Django for validation and the project model, the corpus for shipped code.
- **Generated tests need determinism in CI.** A random failure on an unrelated PR blocks the wrong person. PR runs use a fixed seed and a small case count; a nightly or on-demand run uses fresh seeds and thousands of cases, and each shrunk failure becomes a fixed regression test.
- **Generators must exclude a known bug until its fix lands**, or every run rediscovers it. Each exclusion names the issue or PR that removes it. #989 excludes plain list and dict aliases until #990, and keeps loop bodies straight-line until the loop fix.
- **Reference implementations have their own limits.** Aliased in-place growth inside loops is exponential in CPython, so the harness caps memory and skips cases that hit `MemoryError`.

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
| CPython differential | Static evaluator results that disagree with Python (past fixes: #779, #780, #781, #964) | CPython | Medium | In review: #989 |
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
| Coverage-guided fuzzing (cargo-fuzz, structured inputs) | Crashes, hangs, and runaway growth in the evaluator and template parser (past fix: #778) | Low | New: template parser next |

Template parser fuzz properties, seeded from corpus templates, Django's own template tests, and grammar-generated templates:

- No panic on any input, including non-UTF-8-derived text and unclosed delimiters.
- Every span is in bounds and on a char boundary.
- Nodes are ordered and do not overlap.
- One malformed tag, such as `{%}}` from #974, does not remove other parse errors or nodes.
- Parsing after an edit equals parsing the edited text from scratch.

Evaluator fuzzing runs with time and node budgets, so a hang or runaway growth fails the run instead of stalling it.

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
| Incremental equals from scratch | Stale Salsa results (past fixes: #278, #411, #652, #665) | Low | In review: #987 |
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

- **Every PR, Rust only**: metamorphic properties, incremental equals from scratch, the CPython differential, chaos inputs, formatter and position properties, with fixed seeds and small budgets inside `cargo test`.
- **Nightly or on demand, Rust**: the same properties with fresh seeds and large case counts, plus time-boxed fuzzing of the template parser and evaluator.
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

1. Done or in review: ecosystem diff per PR and mutation testing (#986), incremental equals from scratch (#987), the CPython differential (#989).
2. Now: land the evaluator fixes the CPython differential found, remove its exclusions, and add fixed-seed PR runs with a nightly deep run. Start the template parser fuzzer.
3. Next: Django differential with generated and mutated inputs. It is the oracle for validation and the project model, the largest payoff, and the largest build.
4. Then everything else on the board, by cost.
