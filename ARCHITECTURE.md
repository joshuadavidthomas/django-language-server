# Architecture

This document describes the high-level architecture of Django Language Server (`djls`). It's meant to help you get oriented in the codebase — where things live, how they connect, and why they're shaped the way they are.

The structure and general vibe of this document is inspired by [rust-analyzer's ARCHITECTURE.md](https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/architecture.md), which is an excellent example of this kind of guide.

For contribution guidelines, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Overview

Django Language Server is an LSP server written in Rust that provides IDE features for Django templates. The editor sends template source code; the server combines it with Project Facts — installed apps, Template Libraries, Tag Definitions, and Filter Definitions — and returns diagnostics, completions, folding ranges, and navigation results.

[Salsa](https://github.com/salsa-rs/salsa) drives incremental computation: when a file changes, only the affected queries recompute. The architecture borrows from [Ruff/ty](https://github.com/astral-sh/ruff/tree/main/crates/ty): layered database traits, a single concrete database type that owns all state, and a session model for LSP operations.

The server depends on two distinct kinds of knowledge:

1. What tags and filters exist — which Template Libraries are installed, which Tag Definitions and Filter Definitions they export, and what syntax those definitions accept. This comes from the Python side of the Django project.
2. What the template says — its syntax, its structure, and whether it uses those tags and filters correctly. This comes from parsing the template source.

Separate subsystems produce each kind of knowledge. Both feed into the same Salsa database, where they meet during semantic analysis.

## Entry Points

`crates/djls/src/main.rs` parses CLI arguments and either starts the LSP server or runs the `djls check` command. There's not much to see here — the interesting stuff is deeper in.

If you're already familiar with LSP, `crates/djls-server/src/server.rs` is a good starting point. It implements all the LSP request handlers (`did_open`, `completion`, `folding_range`, `goto_definition`, etc.) and shows how requests flow through the system.

If you want to understand how templates get parsed, start in `crates/djls-templates/src/` — the lexer and a hand-written recursive descent parser live there.

If you're curious about how the server validates tags without a Django runtime, look at `crates/djls-project/src/templates/tags/analysis/` — it extracts validation rules from Python templatetag source purely through static analysis. If anyone's tried this before, they and their project didn't make it out with their sanity intact because I've never come across one (and who says I'll make it out with mine?). It's early and rough, but it's one of the more interesting parts of the project.

If you're interested in how parsing, extraction, and Project Facts come together to give templates *meaning*, start with `crates/djls-semantic/src/lib.rs` — that's where "is this tag valid here?" actually gets answered.

## Code Map

All crates live in `crates/`.

Throughout the code map you'll see **Architecture Invariant** callouts. These are constraints we maintain deliberately — things that are true about the code on purpose and that we'd like to keep true.

### `crates/djls`

The CLI application. It parses command-line arguments, starts the LSP server for `djls serve`, and owns the terminal validation and rendering kernel used by `djls check`. For project-backed checks, the CLI applies settings and Project Facts, discovers the input Templates, calls the `djls-ide` one-shot preparation seam, and only then clones the database into parallel validation workers. That preparation primes intrinsic Template Library products before building the shared Template index; the stdin path uses the same seam. The package exposes its check kernel to benchmarks so they measure production behavior without moving terminal policy into an IDE or semantic crate. This is also the only crate that carries the release version; internal library crates use `0.0.0`.

### `crates/djls-server`

The LSP server. This is the crate that wires everything together at runtime.

`Session` owns the `DjangoDatabase`, open-document state, and intrinsic-product readiness. Open documents live in server-local buffers, and an overlay filesystem exposes those buffers through the `djls-source` filesystem seam before falling back to disk. Mutations briefly take the session's `tokio::Mutex`, update the overlay before applying Salsa source changes, and schedule any required Project work.

Project-aware requests use generation-checked `SessionSnapshot`s rather than computing under the session lock. A request waits on a race-safe readiness watch, locks the session long enough to verify the observed generation and clone a snapshot atomically, releases the lock, and computes on a blocking worker. Salsa cancellation restarts this sequence at readiness waiting. Syntax-only operations, currently formatting, may use an ungated snapshot.

A single coalescing reload worker runs full Project reloads and intrinsic-only re-primes without overlap; queued full reloads dominate re-primes. Changes to Project Facts or covered Python sources advance the desired generation and make it unready. Before coverage is known, every synchronized Python edit is treated conservatively as Project work. Only successful priming for the current generation publishes readiness and coverage; stale or cancelled work cannot release requests, and failures publish an explicit failed state instead of allowing lazy intrinsic computation.

**Architecture Invariant:** `djls-server` is the only crate that speaks the LSP protocol — handling requests, managing the session, and transporting progress/notifications.

### `crates/djls-db`

The concrete Salsa database. `DjangoDatabase` owns Salsa storage plus runtime infrastructure: the overlay-backed file reader, the source-file registry, the current `Project` input handle, and settings. It implements the database traits defined by the lower crates. Both the LSP server and the `djls check` CLI use it.

This crate should stay boring. It wires traits to concrete state and handles local mutation such as settings updates and file creation. Django Discovery workflows live in `djls-project`; IDE cache warm-up lives in `djls-ide`; CLI and LSP orchestration live in `djls` and `djls-server`.

### `crates/djls-project`

The Project Facts layer. This crate owns mechanical facts about a Django project: the `Project` Salsa input, Python interpreter and search-path discovery, module resolution, Django settings extraction, template directories, Template Libraries, discovered template files, template origins/resolution, template tag/filter extraction, model graph extraction, and Django Discovery phases with their domain progress metadata. Its internals are organized by Django domain: `settings/` handles settings-source extraction and source resolution, `templates/` handles template origins, directories, Template Libraries, registrations, tag rules, filter arities, and tag/filter definitions, and `models/` handles Django model graph extraction.

`djls-project` depends on `djls-source` for filesystem/source access, but it does not depend on `djls-semantic`. That one-way seam lets semantic analysis consume observed source facts without project discovery needing to know about template validation, scoping, or diagnostics. Each Template Library has an interned `(Option<File>, PythonModuleName)` identity and equality-bearing definition, Tag, and Filter facts. `TemplateLibraries` assembles backend-correlated catalog evidence and shared definition-name indexes; it does not hold one project-global semantic Tag or Filter result.

#### Static Python import evaluation

One evaluator entrypoint handles both ordinary and `from` import statements. The resolver returns an ordered root-to-leaf chain of source modules and namespace packages; one loader evaluates project-code components through the cycle-enabled module query, records dependencies and typed outcomes, and attaches loaded children only after successful resolution. First-party and extra-root source is evaluated, while site-packages and editable-root modules retain identity but remain open external namespaces whose bodies are never executed.

A module value carries only stable source-or-namespace identity. Its intrinsic lexical namespace remains in `PythonModuleValues`; explicitly loaded child attachments and object-scoped cycle/external uncertainty live in a separate private module-object product. This split lets settings-facing value projections backdate independently from import topology while module attribute reads still compose intrinsic values, loaded children, and open causes.

Named `from` imports project an existing module member first, then try the exact package child through the same chain loader only where that member may be absent. Remaining absence becomes typed missing-member uncertainty. Star imports classify each feasible `__all__` alternative independently: exact static lists and tuples select their named members in order, closed absence selects current public intrinsic or attached names, and dynamic or malformed alternatives preserve possible imports while opening the namespace. Exact `__all__` may load listed children through the shared loader; absent or dynamic `__all__` never scans the filesystem.

The supported boundary is static `import a`, `import a as x`, `import a.b`, `import a.b as x`, named `from a import x`, and `from a import *` under that bounded export policy. Module attribute reads are supported; module attribute writes and mutations, dynamic import hooks, `importlib`, runtime `sys.modules`/`sys.path` changes, and external module execution remain deliberately unsupported and conservative.

### `crates/djls-templates`

The template parser — a hand-written recursive descent parser for Django's template syntax. It's inspired by Django's own parser but designed around IDE needs: error recovery, partial results, and position tracking rather than template rendering.

It lexes and parses template source into a flat `NodeList` — the same representation Django's own template engine uses internally. Each `Node` is a `Tag`, `Variable`, `Comment`, `Text`, or `Error`. The parser always produces a node list; parse errors become `Node::Error` entries in the list and are also emitted through a Salsa accumulator (`TemplateErrorAccumulator`).

**Architecture Invariant:** the parser never fails. It produces `(Vec<Node>, Vec<ParseError>)` rather than `Result<NodeList, Error>`. This is critical for an IDE: users are *always* in the middle of typing something invalid, and the rest of the pipeline needs to keep working around the errors.

**Architecture Invariant:** this crate knows nothing about Django semantics. It can't tell whether `{% load i18n %}` refers to a real Template Library, whether `{% if %}` has the right number of arguments, or whether a filter exists. It handles only template *syntax*: delimiters, tag structure, filter chains, token boundaries. This separation matters because it means you can use the parser for syntax highlighting or other syntax-only tooling without needing Project Facts at all.

### `crates/djls-semantic`

Project meaning. This is where observed Project Facts meet the parsed template, and where most of the interesting template analysis happens.

Each Template Library contributes two independently backdatable semantic products: `LibraryTagSpecs`, which fuses extracted Tag Rules and Block Specs with builtin and configured fallback meaning, and `LibraryFilterSpecs`, which carries extracted Filter Arity. The products are keyed by Template Library identity, so changing one library or one fact category does not rebuild project-global semantic inventories.

For a project-backed Template, semantic analysis builds one tracked `TemplateAnalysisProjection`. A fixed-point loop resolves only Tag and Filter occurrences in the source, discovers effective loader occurrences by `TagRole`, and converges the ordered loaded-library state with a sparse occurrence grammar. Final fact collection resolves each Tag or Filter once per semantic `(visible load prefix, symbol name)` and reuses that contextual result across repeated occurrences, while retaining occurrence-specific structure, arguments, spans, and source identity. The explicit projectless structure seam instead builds its sparse grammar and `TemplateTree` directly in one pass; it does not construct project-correlated Tag or Filter facts. The converged project-backed product correlates:

- the `TemplateTree` and captured closing occurrences;
- ordered Loaded Libraries;
- sparse Tag facts, including effective specs and availability; and
- sparse Filter facts, including availability and arity.

The project-level semantic grammar vocabulary indexes only closer and intermediate spellings to possible opening-definition identities. It supports orphan classification without constructing a complete per-Template grammar. Open or disagreeing alternatives remain inconclusive.

`TemplateValidator` consumes the converged projection directly. It checks load scoping, arguments, Filter Arity, `{% if %}` expressions, and `{% extends %}` positioning without rebuilding grammar, Loaded Libraries, or symbol indexes. Opaque contents never enter the active occurrence stream. Structural diagnostics are emitted only from the converged pass, so fixed-point retries cannot duplicate them.

`TemplateTree` is not intended to be a lossless syntax tree. Parser-owned details that do not affect structure, such as exact parse errors, remain available from the original `NodeList`. Validation errors go through `ValidationErrorAccumulator`. `collect_template_diagnostics` is the configuration-independent collection boundary for syntax and validation errors; output adapters decide filtering, ordering, severity, and representation.

This crate also owns Template-reference relationships: deciding which Tag occurrences create template-domain references after `djls-project` has resolved Template origins.

**Architecture Invariant:** `djls-project` observes source; `djls-semantic` decides project meaning. Ruff-backed Python parsing/extraction and Django template-origin resolution stay in `djls-project`; semantic fusion, template-reference relationships, availability, validity, and diagnostics stay in `djls-semantic`.

**Architecture Invariant:** static extraction never imports Django or runs Python. It parses Python source as text with the same Ruff parser that powers the Ruff linter. If a templatetag file is syntactically valid Python, we can analyze it. We don't need a working Django installation, a virtual environment, or even a Python interpreter.

**Architecture Invariant:** extraction currently only captures constraints on *static template syntax* — argument counts and literal keyword positions knowable at parse time. Many templatetag functions also validate *runtime values* (type checks, truthiness checks on resolved variables), but those guards depend on what template variables resolve to during rendering, which the server cannot currently determine. If type inference is added in the future ([#424](https://github.com/joshuadavidthomas/django-language-server/issues/424)), some of these runtime guards may become statically evaluable — possibly as a separate analysis layer, or as an extension of the extraction pipeline itself.

Project configuration, Python environment discovery, module resolution, and source-derived Django facts live in `crates/djls-project/`. `Project` is a Salsa input holding the project root, interpreter path, Django settings module, resolver-owned `SearchPaths`, and manual tag-spec configuration. `djls-project` also owns the `TemplateLibraries` catalog derived from Django settings, source files, and installed packages. `TemplateEnvironment` is a borrowed view over that shared catalog plus compact feasible-backend selections for one Template; deriving an environment does not clone selected libraries or definitions. Files outside configured Template roots intentionally use project-inventory scope. Imperative Django Discovery functions synchronize external project state into Salsa inputs; tracked Project and Semantic queries derive equality-bearing products from those inputs.

### `crates/djls-ide`

IDE features: completions, diagnostics, folding ranges, snippets, goto definition, find references, and cache warm-up for responsive startup. This crate owns two synchronous production preparation seams. Intrinsic priming walks active Template Library identities, evaluates per-library Project and Semantic products plus the grammar vocabulary, and returns the exact Python source coverage used by server readiness; it performs no per-Template parsing, projection, or validation. One-shot project Template analysis composes that intrinsic priming with shared Template indexing and reports whether a Project was available; callers need no preparation internals. The crate also translates internal domain knowledge into LSP-shaped output that editors can consume.

**Architecture Invariant:** `djls-ide` is the translation layer. Everything below it — `djls-semantic`, `djls-templates`, `djls-source` — is LSP-unaware. `djls-server` should call `djls-ide` for IDE behavior and reach into lower crates only for runtime seams such as project reload and Django Discovery orchestration.

### `crates/djls-source`

Foundation crate — file representation, source-file registry, filesystem access, file discovery, text positions, spans, line indexing, diagnostic rendering. `SourceFiles` owns the path-to-`File` side table and assigns Salsa durability from file roots: first-party project roots are low durability, module/search-path roots are high durability, and file paths are stable identity. The `FileSystem` interface is the shared seam for reading files, checking path kind, and walking source roots; production uses `OsFileSystem`, tests can use `InMemoryFileSystem`, and the LSP server provides an overlay adapter for open buffers. Nearly every other crate depends on this one.

### `crates/djls-conf`

Settings and diagnostics configuration. Merges configuration from multiple sources into a single `Settings` type. See [Configurability](#configurability) for the full picture.

### `crates/djls-bench`

Benchmarks using [divan](https://github.com/nvzqz/divan). Its database supports both explicit projectless structural fixtures and realistic project-backed inputs. Project-backed warm-semantic workloads call the production priming seam; cold-Project and primed-Project/cold-Template workloads keep those costs distinct. The crate benchmarks parsing, sparse Template analysis, validation, extraction, diagnostics, and the documented validation/render kernels used by `djls check`. Check benchmark Divan inputs create the database, synchronize all Template sources, and call the same `djls-ide` one-shot preparation function as the CLI outside the timed region; timed per-file work calls the production `djls::check` kernel directly. They exclude CLI argument/config loading, Django Discovery, filesystem Template discovery, Rayon scheduling, batch sorting, and terminal I/O, so they are not full-pipeline benchmarks. The separate semantic cold-Project and primed-Project/cold-Template benchmarks keep setup and Project costs visible. `just dev profile <bench> [filter]` generates flamegraphs.

### `crates/djls-testing`

Shared test infrastructure. Owns the corpus sync tool, shared Salsa test database, fixture builders, and markdown test runner. The corpus syncs real-world Django project source — templates and templatetag modules from 40+ packages and 17 real projects — for testing extraction and validation against code that actually exists in the wild. See the [Testing](#testing) section for how corpus tests work.

## The Database Trait Stack

Salsa requires a single concrete database type, but each crate should see only the capabilities it needs. DJLS follows the same broad pattern as Ruff/ty, rust-analyzer, BAML, and Cairo: the concrete database owns state, database traits expose capabilities, tracked functions compute derived facts, and imperative Django Discovery code stays outside tracked queries.

```
salsa::Database
└── SourceDb   (djls-source)  — source-file registry, tracked files, filesystem reads/walks
     └── ProjectDb  (djls-project) — current Project input
          └── SemanticDb (djls-semantic) — semantic accessors used by validation and IDE features
```

Template parsing does not need its own database trait. `parse_template` depends directly on `SourceDb` because it only needs source text. Filesystem access also enters through `SourceDb`: Django Discovery walks source roots through the database filesystem, and `DjangoDatabase` observes LSP buffer state through the server's overlay filesystem adapter.

`DjangoDatabase` in `djls-db` implements the production stack. Test databases and `BenchDatabase` implement the same traits with fixture-backed source and project state. Project-backed tests install a `Project` and derive each template's effective environment from its discovered backend, Template Libraries, and builtins; Template Library inventory is not global database state.

### Salsa boundary rules

- Concrete database structs own storage and runtime infrastructure. They should not become semantic service objects.
- Database traits describe capabilities: file access, current project access, or semantic fixture access.
- Tracked queries compute values from Salsa inputs and tracked files. They should not run subprocesses, write caches, or mutate inputs.
- Free functions perform imperative synchronization from the outside world into Salsa inputs: discovering template directories and Template Libraries, indexing first-party project files, scanning installed packages, and updating `Project` fields with setters.
- Durability follows the same split: first-party file revisions and the project file set are low durability; project configuration is medium durability; search-path roots and stable file identity are high durability.

## How Project Facts Get In

### Static Django Discovery

The server needs to know what Django has installed: `INSTALLED_APPS`, template directories, Template Libraries, and the Tag Definitions and Filter Definitions they export. It derives those Project Facts from source instead of importing Django or running project Python.

Django Discovery starts from the configured Django settings module. `djls-project` parses that module and its source-level imports with the Ruff parser, then derives the Project Facts used by downstream semantic analysis:

1. The `Project` input stores the project root, interpreter path, Django settings module, search paths, and manual tag-spec configuration.
2. Template directories come from the static settings projection and are exposed as tracked project queries.
3. Template Libraries come from three source-derived places: configured `OPTIONS["libraries"]`, configured/default `OPTIONS["builtins"]`, and `templatetags` packages under installed apps.
4. The registration scanner produces indexed definition facts and independently backdatable Tag and Filter facts for each Template Library identity. Shared catalog assembly stores definition-name references and backend correlation, not copied semantic specs. `djls-project::models` separately extracts Django model graphs.
5. Search-path roots for installed packages remain high-durability source roots, so package files are reread when Django Discovery detects that external data changed.

A full startup reload reads configuration, runs Django Discovery, primes intrinsic Template Library products, publishes generation readiness, republishes diagnostics, and then warms optional IDE caches. `djls-project` owns the discovery phase registry and domain progress metadata; `djls-ide` owns priming and cache warm-up; `djls-server` owns coalescing, generation readiness, cancellation/retry, and LSP progress transport. There is no embedded inspector zipapp, no `django.setup()`, and no Template Library disk cache in the server path.

### Rust-Side Extraction (Static Validation Rules)

Static discovery reports *what* tags and filters exist. But to actually validate usage — "does this tag accept these arguments?" — the server needs to know *how* each tag and filter works. Django's template engine answers this question at runtime, by calling the tag's compilation function and seeing what happens. We don't have a runtime.

Instead, `djls-project` parses templatetag Python source files with the Ruff parser and extracts validation rules directly from the AST. It walks each module looking for `@register.tag`, `@register.simple_tag`, `@register.filter`, and similar decorators, then analyzes the decorated function's signature, decorators, and `if condition: raise ...` guard patterns to infer:

- **Tag rules** — argument count constraints (min/max positional args), required keywords, choice-constrained positions, `as var` support, block specs (which intermediate and end tags a block tag expects)
- **Filter arity** — whether a filter requires an argument, accepts one optionally, or takes none

This works well for `simple_tag` and `inclusion_tag` registrations where the function signature maps directly to template arguments. Hand-written compilation functions (like Django's built-in `do_if` or `do_for`) are harder — those have custom argument parsing that doesn't follow a signature — but extraction still tries, using abstract interpretation to track variables like `bits = token.split_contents()` through the function body and infer constraints from the raise guards. Hardcoded specs provide baseline structure (end tags, intermediates) for builtins, and extraction results merge on top to add argument validation. When extraction can't figure something out, it falls back gracefully.

Both workspace and search-path extraction now flow through Salsa:

- **Installed package modules** (site-packages/dist-packages) — resolved through typed `SearchPath`s, represented as high-durability tracked files under `SearchPath` roots, and extracted through the same Salsa tracked queries as workspace files. External-data discovery bumps search-path root revisions and currently discovered dependency file revisions so stale installed-package content is reread.
- **Workspace modules** (project code and extra pythonpath entries) — represented as low-durability tracked files under `Project` roots and extracted through the same Salsa tracked queries. Edits to known files recompute automatically; new files enter the graph on the next Django Discovery run.

## The Template Pipeline

When a template file opens or changes, it flows through a series of stages. Each stage feeds the next, and errors accumulate along the way without blocking later stages:

1. **Lexing** — tokenizes template text into tag, variable, comment, and text tokens.
2. **Parsing** — produces a flat `NodeList`. Parse errors become `Node::Error` entries and are also emitted via `TemplateErrorAccumulator`.
3. **Template analysis** — Project-backed analysis uses `TemplateAnalysisProjection` to run a sparse structural/load fixed point. It resolves source occurrences against the borrowed `TemplateEnvironment`, converges loader state, builds the `TemplateTree`, and records sparse Tag and Filter facts. Opening Branches capture their contracts, so later loads cannot rewrite existing structure. Structural errors accumulate once from the converged pass. Explicit projectless structure analysis is a direct one-pass query that builds only the sparse grammar and tree needed by its caller.
4. **Validation** — a single-pass validator consumes the correlated projection and checks load scoping, argument counts, Filter Arity, expression syntax, and `{% extends %}` rules. Opaque contents have already been excluded from active semantic occurrences. Errors accumulate as `ValidationError`s.
5. **Diagnostics** — `collect_template_diagnostics` in `djls-semantic` collects syntax and validation errors without output policy. `collect_diagnostics` in `djls-ide` converts that result to LSP diagnostics and applies severity overrides; `djls::check` owns terminal filtering and rendering.

The key insight is that no stage blocks on errors from a previous stage. A template full of syntax errors still gets structural analysis on its valid portions, and a template with structural problems still gets validation on the tags that parsed correctly.

### Load Scoping

Django templates have position-dependent Tag and Filter Availability. A tag or filter becomes valid only *after* the `{% load %}` that introduces its Template Library, and only tags/filters from loaded Template Libraries are available.

The Template analysis fixed point recognizes loaders by effective `TagRole`, records ordered full and selective loads, and resolves each source occurrence against the load prefix at its position. The resulting sparse facts distinguish available, unloaded, ambiguously unloaded, unknown, and inconclusive symbols without building a complete per-Template symbol index. Completion is the explicit exception: it enumerates shared catalog name indexes when the editor needs a complete inventory.

## Cross-Cutting Concerns

### Error Handling

The codebase follows a deliberate split: **analysis never fails, infrastructure can.**

Template parsing and semantic validation currently use Salsa accumulators to report errors. These functions return their primary result (a node list, a template tree) regardless of how many errors they found. The errors are side-channel output that callers retrieve separately. This is essential for IDE use — you need to provide completions and navigation even in files full of errors.

> [!NOTE]
> Accumulators work well at our current scale, but they have a known limitation: calling `accumulated()` adds an untracked dependency, which means the collecting query re-runs on every revision. Larger Salsa projects (ty, rust-analyzer, Cairo) avoid accumulators in favor of embedding diagnostics in return values. We'll likely need to make that migration at some point.

Infrastructure code — the CLI, file I/O, configuration loading — uses `anyhow::Result`. These are operations that can genuinely fail (disk full, malformed TOML), and the failure should propagate up to the user.

`collect_template_diagnostics` in `djls-semantic` is the boundary between accumulated analysis errors and output adapters. `collect_diagnostics` in `djls-ide` rejects files that are not LSP diagnostic targets and translates the collected errors into a flat `Vec<Diagnostic>`. The `djls::check` kernel separately combines collected errors with fallible source reads and terminal rendering. Analysis collection itself never fails; infrastructure failures remain typed until the CLI adds application context.

### Observability

The server uses `tracing` with a custom `LspLayer` subscriber that routes a single log call to two destinations: rotating daily log files on disk, and the editor's output panel via LSP `window/logMessage` notifications. This means `tracing::info!("something happened")` in any crate automatically shows up in both places without the callsite knowing about LSP.

### Configurability

`djls-conf` merges settings from multiple sources (user config, project TOML files, LSP client options) into a single `Settings` type. When settings change at runtime via `didChangeConfiguration`, the server compares each field before calling Salsa setters — this avoids unnecessary invalidation and keeps incremental recomputation tight.

## Testing

The project has a few different testing layers, each targeting a different boundary.

### Template Parser Tests

The parser uses [insta](https://insta.rs/) snapshot tests extensively — there are 400+ snapshot files across the codebase. A typical parser test parses a template string and snapshots the resulting AST:

```rust
#[test]
fn test_parse_django_variable() {
    let source = "{{ user.name|title }}";
    let nodelist = parse_test_template(source);
    insta::assert_yaml_snapshot!(convert_nodelist_for_testing(&nodelist));
}
```

In general, we prefer snapshot tests over hand-written assertions. Nobody wants to write `assert_eq!` against a deeply nested AST, and nobody wants to read one either. Snapshots show you the full picture, `cargo insta review` makes changes easy to audit, and adding a new test case is just "write the input, run it, eyeball the output, accept."

### Semantic Validation Tests

Validation tests use test databases that implement only the Salsa traits they need. `TestDatabase` provides an in-memory filesystem and fallback semantic specs for source-only unit tests — no Python process, Django runtime, or disk I/O. Project-scoped tests use `ProjectFixture` to install settings and Python source into that filesystem. Normal discovery then derives backend membership, builtins, and available Template Libraries for each template origin; the database does not carry a global Template Library inventory.

The typical source-only pattern is: build a database with the specs you care about, parse a template, validate it, and check the accumulated errors:

```rust
#[test]
fn unknown_tag_produces_diagnostic() {
    let db = standard_db();
    let errors = collect_errors(&db, "test.html", "{% foobar %}");
    assert!(errors.iter().any(|e| matches!(e, ValidationError::UnknownTag { .. })));
}
```

For more complex cases, there's a diagnostic renderer (the same one `djls check` uses for terminal output) that produces human-readable snapshots:

```
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}oops{% endif %}
  | ^^^^^^^^^^^^^^
  |
  = note: in tag: if
```

**Architecture Invariant:** tests never require a Django installation or run a Python interpreter. Source-only tests may supply tag specs and filter arities directly. Project-scoped tests supply settings and Python modules through `ProjectFixture`, then exercise the same static Template Library and environment discovery used by production. JSON fixtures remain appropriate for isolated extraction projections.

### Corpus Tests

This is the most interesting testing infrastructure. The corpus (`just corpus sync`) downloads real source from 40+ PyPI packages (Django itself, django-allauth, django-crispy-forms, etc.) and 17 real-world projects (Sentry, NetBox, Read the Docs).

Corpus tests serve two purposes:

1. **Extraction snapshot tests** — parse every `templatetags/*.py` file with the Ruff parser and snapshot the extracted rules. This catches regressions in Python AST analysis and documents what we can extract from real-world code.
2. **Validation integration tests** — validate real templates against extracted rules. This is our "zero false positives" check: if we report a diagnostic on a template from a real project, it's probably a bug in our analysis, not in the project.

The corpus is deliberately not checked into the repository (it's ~hundreds of MB of third-party source). `just corpus sync` downloads it from the lockfile (`crates/djls-testing/manifest.lock`), which pins exact versions and SHA-256 checksums.

### Incremental Computation Tests

`DjangoDatabase` and focused test databases capture Salsa events such as `WillExecute` and `DidValidateMemoizedValue`. Tests prime a concrete query graph, mutate source or Project inputs with normal setters, then assert exact query identities and execution counts alongside semantic output.

Current coverage follows the production contract across crates. It proves that shared warm-up evaluates each per-library source, Tag, and Filter product exactly once and performs no per-Template work; CLI-style parallel validation starts only after priming; and LSP requests wait for current-generation readiness. Template-pipeline tests cover same-revision memoization, whitespace backdating, meaningful load/Block Tag/Filter edits, unrelated Template and Python edits, sparse projection locality, and unchanged environment backdating. Server tests cover covered-source invalidation, stale and cancelled generations, failure wakeups, race-free readiness watches, and full-reload dominance over queued re-primes.

### CLI Integration Tests

Black-box tests in `crates/djls/tests/check.rs` that invoke the `djls check` binary as a subprocess against temp directories. They verify exit codes, diagnostic output format, and CLI flags like `--ignore` and stdin input (`check -`).
