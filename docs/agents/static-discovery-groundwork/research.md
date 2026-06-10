# Investigation: PRs #606, #626, and the codebase shape that produced them

Date: 2026-06-10
Subject: deep-dive review of PR #606 ("Add source-based Django discovery") and PR #626 ("Rethink startup loading"), the structural problems on `main` that inflated both, and the house-cleaning sequence that should precede any retry. References: ty/ruff (`reference/ruff`) and rust-analyzer.

Investigated at `main` tip `922cc4d7`. Both PRs branch from `51f5c425` (27 commits behind main). Diffs analyzed: #606 +12,205/−1,006 (29 files), #626 +24,089/−6,262 (101 files, 68 commits).

## TL;DR

Both PRs reached the goal, and both contain a genuinely sound core. They ballooned for the same reason: **the current codebase forces every new project fact through a multiplication table** — widen the god `Project` input, extend the imperative `sync.rs` push choreography, invent another partial-knowledge representation, thread it through a fixture-style db trait implemented three times, and rebuild test scaffolding because tests bypass the discovery pipeline. The features themselves are modest; the multiplication is the +12k/+24k. Neither PR is worth rebasing (main has moved 27 commits across the same ground, and #626's e2e test value already landed independently in #635–643). The right move: close both, do a short sequence of house-cleaning PRs against main, then re-land the salvageable cores as small pull-shaped changes.

## What each PR actually achieved

**#606** built a real static Django discovery pipeline: ruff-parse `settings.py`, follow star imports with cycle detection, evaluate `BASE_DIR / "templates"`-style path expressions, resolve `INSTALLED_APPS` entries with Django-faithful AppConfig semantics (`apps.py` `default` selection, module-then-class ordering), discover app `templatetags/` with `pkgutil.walk_packages` semantics, and reuse the existing registration extractor for symbols. It has 164 tests including a static-vs-runtime comparison harness. The domain logic is largely correct.

**#626** got the architecture *direction* right: the fat semantic `Project` fact bag is genuinely deleted, `initialize` is protocol-only, a stable `djls_project::Project` Salsa input is mutated via setters (the ty pattern, correctly cited in its own `architecture-decision-project-root.md`), orchestration/generations/progress live outside Salsa, and `tests/lsp/test_startup.py` tests the real startup contract. The Ruff-AST → `PythonSourceModel` anti-corruption layer and the environment-candidates + late `environment_for_file` selection are good designs.

## Where #606 went off the rails

1. **Salsa was bypassed entirely.** None of the extraction is a tracked query — it reads via `fs::read_to_string` in the imperative sync layer, then reinvents incrementality with a hand-rolled mtime-snapshot JSON cache (`sync.rs:808–1019` on the branch). That cache is a parallel reimplementation of exactly what Salsa file inputs + tracked functions exist for.
2. **`sync.rs` became a 4,088-line god module** (from 787): cache policy, SHA-256 keys, two cache formats, dual-mode dispatch, assembly orchestration, fallbacks. `refresh_template_state` alone is 345 lines with `#[expect(clippy::too_many_lines)]` acknowledging the debt.
3. **The dual source/runtime path tripled the pipeline.** The runtime arm's fallback is a near byte-for-byte copy of the source arm; one 60-line snapshot-classification match block appears five times.
4. **The partial evaluator dug past the point of value**: Python list-method algebra over facts (`insert`/`pop`/`remove`/`sort`/`reverse`, ~250 lines of 4-arm matches), importing zero-argument path-returning functions from sibling modules, four copies of the same recursive statement-walker differing only in leaf predicate.
5. **`Fact<T>` confidence is mostly behaviorless ceremony.** `Ambiguous` is constructed in exactly one production site and meaningfully consumed in ~2; roughly 25 of 28 consuming matches treat `Unknown` and `Ambiguous` identically. The 4-state `Fact` collapses to 3-state `Knowledge` collapses to 2-state `TemplateDirs`, each via bespoke matches — because the data must land in inspector-era wire types on the god input. The one place confidence genuinely drives behavior (`Knowledge::Partial` gating which diagnostics fire) is small and good.
6. **Two parallel module resolvers** coexist (`resolve.rs` and `module_resolver.rs`), patched over with import aliases; plus dead enum variants, test-only fact fields, `#[cfg(test)]` functions inside production sections, docs claiming multi-environment behavior the branch deleted as dead code, and Django 5.2 defaults hard-pinned regardless of the project's actual Django version.

## Where #626 went off the rails

1. **Three architectures were built and torn down inside one PR.** Per-commit churn sums to ~38.9k insertions / ~21k deletions — roughly 15k lines written and deleted within the branch. The docs record the full arc: readiness-bag input → loading plan/effects/driver framework → discovery run, with two mid-flight correction plans and a 414-line issue-payload inventory. The final code keeps scar tissue from all three.
2. **Its own design's central anti-goal got built.** `design.md` explicitly rejects "one new `reload_project` pipeline… it risks becoming `refresh_external_data` with a new name." `run_django_discovery` is a strictly sequential 7-stage for-loop (`discovery_run.rs:393–414`) — `refresh_external_data` with a new name, wearing a milestone table.
3. **Half the readiness machine is unreachable.** `SourceFilePartitionReadiness::{Loading, Deferred, Skipped, Stale}` are never constructed in production; `SourceFilesApplyResult::Deferred` never produced; `DiscoveryStageStatus` has 7 variants whose distinctions change only a Debug string. ~14 state/status enums + ~8 issue enums + ~12 carrier structs model "loading," relayed three layers deep into a binary "unready" at the consumer.
4. **Two god modules**: `source_files.rs` (2,075 lines, ~20 types, including a u16 precedence scheme for exactly three partitions) and `djls-server/src/startup.rs` (1,936 lines, 19 types, with `is_current()` guards repeated 18 times and a six-layer progress relay).
5. **Salsa hygiene bug**: a fresh `SourceFileSet` input is created on every apply — Salsa inputs are never collected, so each discovery run leaks an input entity, and change-detection comparing the fresh handle means identical contents still invalidate everything downstream. The stable-handle pattern was adopted at the top but not the in-place-update discipline beneath it.
6. **Design violation**: the crate designed to exclude "runtime subprocess lifecycle" ended up spawning the Python inspector (`enrichment.rs` links `libc`, `wait-timeout`, process groups), and test concerns leaked into production types (`ProjectRootDiscovery::FixtureDoesNotModelDiscovery`, a 3-variant enum naming which *test fixture* is unavailable, three stacked wrapper functions whose parameters exist only for test gates).
7. **Scope: roughly 10–12 PRs in one** — protocol-only init + e2e harness, file-set primitives, the crate skeleton, the startup controller, progress reporting, python source models, environment candidates, settings composition, template inventory, enrichment relocation, CLI migration, conf refactors — plus 7,600 lines of planning docs in the diff.

## The root cause: five structural problems on main

These are why both attempts — by different routes — produced the same explosion. (Current main, tip `922cc4d7`.)

1. **The god `Project` input** (`djls-semantic/src/project/input.rs:160`) mixes four planes in 10 fields: identity (`root`), Python environment (`interpreter`, `pythonpath`, `env_vars`, `search_paths`), discovered Django facts (`django_settings_module`, `template_dirs`, `template_libraries`), user config (`tagspecs`), and a derived index stored as input state (`template_files`). Every new fact = new field + refresh function + setter-compare + 10-positional-arg constructor updates in every test. And it lives behind an **untracked** `Arc<Mutex<Option<Project>>>` on `DjangoDatabase` (`djls-db/src/db.rs:42`) read from inside tracked queries — the `None→Some` transition invalidates nothing.

2. **The facts plane is push-shaped; static extraction is pull-shaped.** Derived data is imperatively walked and written *into* inputs by `sync.rs` choreography (`refresh_template_files` walks dirs and stores the result as input state; `refresh_python_modules` calls tracked queries and then bumps the revisions of the files those queries found). Static extraction naturally wants `settings.py` file → tracked parse → tracked facts. #606 joined the push choreography (more sync.rs + a hand-rolled cache); #626 rebuilt the world to escape it. Worse, **extraction targets currently come from the inspector** (`templatetag_modules` reads `template_libraries.registration_modules()`), so settings extraction — which *produces* the libraries list — inverts the dependency arrow. There's no seam to plug into; you must restructure.

3. **Three-and-a-half competing "not known yet" representations**: `TemplateDirs::Unknown/Known`, two `Knowledge` flags on `TemplateLibraries` gated manually at each consumer, `Option`-returning accessors, and the dead `Fact<T>` lattice in the stranded modules. Static extraction is fundamentally about partial knowledge; with no single representation, #606 invented a fourth and then had to map every consumer onto it.

4. **Stranded scar tissue and dead machinery**: 1,444 lines of `#![allow(dead_code)]` "Milestone A1–A3" modules (`static_model.rs`, `static_resolver.rs`, `static_django_environments.rs`) duplicating live types under parallel names (`ImportRoot` vs `SearchPath`, `ResolvedModule` vs `PythonModule`), plus the 713-line `TemplateLibraries` dual-source merge machinery whose `Discovered` arm no production code ever constructs. Any new attempt first has to reconcile two vocabularies.

5. **The test and session architecture force big-bang changes.** Unit tests inject `TagSpecs`/`TemplateLibraries` as constants, bypassing discovery entirely; the only end-to-end exercise of discovery needs a real venv and the real subprocess (and `TestDatabase::project_introspector()` constructs a real introspector, leaking a reaper thread per call). So discovery changes can't be validated piecewise. Meanwhile every request and the "background" refresh share one `tokio::Mutex<Session>` — `SessionSnapshot` is `#[cfg(test)]`-only — so any startup-improvement PR is forced to also build snapshot/cancellation infrastructure. That's exactly where #626's scope exploded.

What #647–652 already fixed: the **derivation side** (search paths → modules → extraction → tag specs) is now respectably pull-shaped and Salsa-correct, template resolution is real tracked queries, domain types match CONTEXT.md, `djls-workspace` is dissolved. What they didn't touch: the god input, the untracked project mutex, the push-shaped sync/cache layer, the inspector owning discovery's *inputs*, the single-environment assumption, and the session-lock startup model.

## What ty and rust-analyzer do instead (the three rules)

The reference study (`reference/ruff`) reduces to three rules that kept ty's equivalents small:

1. **Discovery produces plain values; only orchestration writes them into Salsa inputs, in one revision.** `ProjectMetadata::discover` is db-free TOML walking; rust-analyzer's `project-model` is a deliberately plain, slow, fallible crate; reload applies the result as one atomic change. Environment discovery is *input production, not a query*.
2. **Every "facts from disk" computation is either a db-free function or a tracked query over `File`/root-revisions — never a push pipeline.** `ty_site_packages` (3,646 lines, zero subprocesses) finds the entire Python environment statically: `$VIRTUAL_ENV` → `.venv` → binary-path arithmetic, a hand-rolled `pyvenv.cfg` parser, directory-layout probing that even infers the Python version from layout. Directory scans become incremental by depending on a `FileRoot` revision counter the watcher bumps (`ty_module_resolver/src/resolve.rs:841–848`) — no mtime caches. Lazy expensive indexes use the `IndexedFiles` cell pattern (`ty_project/src/files.rs:14–98`), not eager pushes.
3. **Python understanding is capped at syntax-pattern recognition with an honest Unknown.** ty's one Python-source recognizer carries the thesis comment: *"This is all syntax-only analysis so it could be fooled but it's really unlikely… better than nothing"* (`ty_module_resolver/src/resolve.rs:1515`). The alternative — actual abstract interpretation — costs ty ~150k lines in crates whose entire reason to exist is inference. #606's list-method algebra and path-returning-function imports, and the existing `python/analysis/guards.rs` machinery, are the first steps down that slope.

Two structural patterns worth copying directly: ty splits configuration into a slim HIGH-durability `Program` singleton input (python version + search paths — what the semantic layer needs) versus the broader MEDIUM-durability `Project` input; and the concrete db holds a plain `Option<Project>` whose handle stays stable for the db's lifetime, mutated only via setters (`ty_project/src/db.rs:37–46`) — the documented invariant that makes the untracked `db.project()` read safe.

## The house-cleaning sequence (make the change easy)

Ordered so each lands small and independently, cheapest first:

1. **Delete the dead weight.** The three `static_*` milestone modules (1,444 lines) and the never-constructed `Discovered`/merge machinery in `symbols.rs`. Pure deletion; removes the parallel vocabulary every future attempt must reconcile.
2. **Converge on one partial-knowledge representation.** Pick one (the `Knowledge::Partial` + positive/absence diagnostic gating from #606 is the proven, behavior-bearing kernel) and retire `TemplateDirs::Unknown` / ad-hoc `Option` signaling. Do this *before* extraction, so consumers are rewritten once.
3. **Stabilize the project handle.** Replace `Arc<Mutex<Option<Project>>>` with ty's pattern: handle created at db construction, never swapped, mutated via setters. Small diff, removes a whole class of invalidation bugs, and is a precondition for splitting the input.
4. **Split the god input along its planes.** A slim environment input (root, interpreter, search paths, settings module — MEDIUM/HIGH durability) separate from user config (`tagspecs`); evict the derived fields. `template_files` becomes a lazy index or tracked query; `template_dirs`/`template_libraries` stay as inputs only until extraction replaces them. This is the single highest-leverage change — it's what both PRs were really fighting.
5. **Extract db-free environment discovery.** Move interpreter/venv/site-packages probing (`project/python.rs`, parts of `resolve.rs`) out of djls-semantic into a small db-free module or crate — the `ty_site_packages` analog — pure functions over the `FileSystem` trait with origin-carrying error enums. This is also where pyvenv.cfg parsing lands when wanted.
6. **Give discovery a test seam.** A fake-able introspector/discovery trait so the subprocess can be swapped piecewise and discovery changes validated incrementally — the missing piece that forces big-bang PRs today.
7. **Promote `SessionSnapshot` out of `#[cfg(test)]`** and move background work onto snapshots. This is the precondition for any honest startup work; without it, "background refresh" and request handling share one lock.

With those landed, the two features become genuinely easy changes:

- **Static extraction** = tracked queries over `File` inputs: `settings_facts(db, file)`, `app_registry(db, ...)`, `templatetag_libraries(db, ...)` — reusing #606's domain logic (module resolver, tier-1 settings recognizer minus the evaluator tail, AppConfig semantics, templatetags discovery, and its test corpus) with no hand-rolled cache, no dual pipeline (delete the runtime path rather than keep both), and confidence only where behavior branches. The inspector shrinks to deleted.
- **Startup rethink** = #626's salvageable core re-landed small: protocol-only initialize (already true), discovery as a plain function producing values applied in one revision, generation guard, work-done progress with log fallback — without the 7-stage observer framework, the readiness partitions, or the 22 enums.

## Disposition of the PRs

Close both; don't rebase. Both branch from 27-commits-stale main, #626 touches the since-dissolved `djls-workspace`, and its e2e-test contribution already landed via #635–643. Before closing, harvest:

- #606: extraction modules (`module_resolver.rs`, `settings_facts.rs` tier-1 core, `app_registry.rs`, `template_libraries.rs`/`template_symbols.rs`) and the fixture corpus + static-vs-runtime comparison harness
- #626: `python/source.rs` (Ruff-AST anti-corruption layer), `architecture-decision-project-root.md`, `reference-evidence-rust-analyzer-ruff-ty.md`, `tests/lsp/test_startup.py`, the generation-guard semantics, the bounded installed-app file-loading policy (`apps.rs`)
- Standalone cherry-pick candidate: #606's `is_register_object` precision fix in `python/registry.rs` — an independent correctness improvement.

## Reference crate mapping (for future design work)

| ty / rust-analyzer piece | Role | DLS analog |
|---|---|---|
| `ruff_db` | `File`/`Files`/`FileRoot` inputs, source/parse queries, `System` | `djls-source` (root revisions exist; keep growing this) |
| `ty_site_packages` | db-free static env discovery (pyvenv.cfg, layout probing) | new db-free env-discovery module/crate extracted from `djls-semantic/src/project/{python,resolve}.rs` |
| `ty_module_resolver` | search-path queries, root-revision invalidation | module/templatetag/template-name resolution queries |
| `is_legacy_namespace_package` recognizer | syntax-only fact extraction with honest Unknown | settings + templatetag recognizers (bounded; the anti-`ty_python_semantic`) |
| `ty_python_core::Program` | slim singleton input of fundamental settings, HIGH durability | slim Django environment input (root, interpreter, search paths, settings module) |
| `ty_project` (`Project` input, `ProjectDatabase`, `IndexedFiles`, `apply_changes`) | project umbrella + concrete db + change routing | `djls-db` grown into the real project crate; `djls-conf` keeps raw-Options vs resolved-Settings split |
| r-a `project-model` + `reload.rs` | plain non-incremental loader; one atomic apply seam | db-free discovery functions called from orchestration; single `apply_changes` entrypoint |
| `ty_server::Session` / r-a `GlobalState` | mutable state, snapshots, deferred init, retry-on-cancel, progress | `djls-server` (session owns db; background work = snapshot-only; `LazyWorkDoneProgress` pattern) |
