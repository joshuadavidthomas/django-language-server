# Memo: Settings source graph — extraction vs. dependency tracking

> Design memo — no implementation. Written against working-copy commit
> `710f4107` (branch `plan-008-derive-template-libraries-from-source`,
> PR #664), including the uncommitted edits to
> `crates/djls-semantic/src/project/settings.rs`. All line references are to
> that state.

## Problem statement

`crates/djls-semantic/src/project/settings.rs` holds two functions over what
looks like the same settings source graph: `django_settings`
(settings.rs:51-61), a tracked query that extracts `DjangoSettings`, and
`settings_source_files` (settings.rs:63-73), an untracked function that walks
the settings star-import closure to hand `sync.rs` a list of `File`s to bump.
Is this principled separation or a smell? Should they unify, stay separate, or
be reframed as two projections of one model?

## Current model summary

- `settings_module_file` (settings.rs:45-49, tracked): resolves
  `DJANGO_SETTINGS_MODULE` to a `File` via the search paths. The root anchor
  for everything below.
- `django_settings` (settings.rs:51-61, tracked, `returns(ref)`): reads the
  settings file's source and runs `djls_project::extract_settings` with
  `SalsaSettingsResolver` answering star-import requests.
- `SalsaSettingsResolver::resolve_star_import` (settings.rs:559-572): resolves
  the imported module to a `File` (`module_file`, settings.rs:496-509 — which
  touches every search-path root revision, then probes the filesystem
  untracked) and returns `file.source(db)` — a **tracked** read.
- `settings_source_files` (settings.rs:63-73, plain `pub(super)` fn): calls
  `collect_settings_source_files` (settings.rs:455-494), which reads each file
  via `db.read_file` — an **untracked** direct filesystem read
  (djls-source/src/db.rs:17-19, overlay-aware) — re-parses it with
  `ruff_python_parser` directly, scans **top-level statements only** for
  star imports, and recurses with a `seen` set.
- `sync.rs::refresh_python_modules` (sync.rs:28-66): on explicit refresh, bumps
  every search-root revision, then bumps the revision of each file returned by
  `settings_source_files`, plus the model/templatetag module files.

## The domain concepts (Q1)

| Concept | What it is | Where it lives |
|---|---|---|
| **Settings module file** | The root of the graph, from `DJANGO_SETTINGS_MODULE`. | `settings_module_file` (tracked). |
| **Settings source closure** | Settings module + transitively star-imported modules. Crucially this is **evaluation-dependent**: the extractor follows star imports anywhere in the statement walk — inside `try`/`if`/`with`/loop bodies (extractor.rs:186-246, 322-354) — and *skips* statically-false branches (extractor.rs:356-365). There is no static "import graph" separate from evaluation. | Discovered by the extractor's walk. |
| **Extracted settings** | `DjangoSettings` — the pure projection of the closure's content. | `djls-project` (extractor.rs:35-44). |
| **Invalidation footprint** | The set of `File` inputs whose `source` the extraction read, plus the root revisions covering its untracked existence probes. | Recorded by Salsa internally as `django_settings`'s dependency edges; not enumerable by callers. |
| **Refresh boundary** | The imperative reconciliation of disk truth into Salsa inputs (the LSP has no watched-file stream for dependency roots — sync.rs:29-31). Needs an *enumerable* file list, computed against **current disk**, not against the Salsa view. | `sync.rs` + `settings_source_files`. |
| **Source resolver** | The anti-corruption seam between the pure extractor and the Salsa world: `SettingsSourceResolver` (djls-project/src/extraction/settings.rs:226-233). | Trait in djls-project; impl in djls-semantic. |

## Why both functions exist — and whether the duplication is real (Q2)

The two functions are **two projections of the same graph at different
freshness**, and the freshness difference is load-bearing:

- `django_settings` projects the **Salsa view**: it reads `file.source(db)`,
  which is memoized keyed on `File.revision`. Its job is derivation, and its
  dependency edges are how within-session invalidation works.
- `settings_source_files` projects the **disk view**: `db.read_file` bypasses
  the memo and reads the filesystem (through the overlay) directly. Its job is
  to tell the refresh boundary which `File` revisions to bump so that the
  Salsa view catches up to disk.

The disk-freshness cannot be replaced by asking Salsa: a refresh that bumps
only the *old* closure under-bumps when the settings file changed on disk to
import a module Salsa already has a stale `File` for. Suppose `settings.py`
gains `from .extra import *` on disk, and `extra.py` was previously read for
some other reason. Bumping the old closure ({settings.py, base.py}) re-runs
`django_settings`, which reads fresh `settings.py`, resolves `extra.py` — and
gets the **stale memoized source**, because nothing bumped `extra.py`. The
disk-walk sees the new edge before Salsa does and includes `extra.py` in the
bump set. So the *role* separation is principled.

The *logic* duplication is real, though — two walkers over the same graph with
different fidelity — and the fidelity gap is a latent invalidation bug today:

> **The bug.** `collect_settings_source_files` scans only top-level
> `Stmt::ImportFrom` (settings.rs:474-479: `for stmt in
> parsed.into_syntax().body`). The extractor follows star imports nested in
> any compound statement it walks — including `try` bodies
> (extractor.rs:218-226). The single most common Django settings idiom is
> exactly that nesting:
>
> ```python
> try:
>     from .local_settings import *
> except ImportError:
>     pass
> ```
>
> Extraction follows `local_settings.py` (Salsa dependency recorded via
> `file.source`), but the sync walker never lists it. On the next disk edit to
> `local_settings.py` + refresh: the root-revision bumps invalidate
> `django_settings`, it re-runs, and `file.source(local_settings)` returns the
> stale memo — **stale settings survive a refresh**. The layering tests pass
> today only because they use top-level `from .base import *`
> (settings.rs:752-776).

There is also a second-order divergence in the other direction: the extractor
skips statically-false branches, so a hypothetical graph-walker that followed
*all* imports would over-approximate. For invalidation, over-approximation is
safe (a spurious bump costs a re-run); under-approximation is the bug class.
Today's walker under-approximates.

## Candidate shapes (Q3, Q7)

**A. Status quo + fidelity fix.** Teach `collect_settings_source_files` to
recurse into compound statements. Fixes the known bug; keeps two hand-matched
walkers that have already drifted once and will drift again (every extractor
walk change needs a mirrored sync change, enforced by nothing). Acceptable as
a hotfix on PR #664's branch; wrong steady state.

**B. Extraction returns the footprint.** Make `django_settings` (or
`extract_settings`) return `(DjangoSettings, Vec<File>)`. Rejected twice over:
it pollutes the pure extractor's output with caller bookkeeping, and it has the
wrong freshness — a tracked query's footprint reflects the *stale* sources, so
it under-bumps in the new-edge case described above. The footprint must be
computed from disk at refresh time.

**C. One walker, two drivers (recommended).** The star-import closure walk
*is* the extractor's walk — so reuse it. The observation point already exists:
the resolver sees every star-import edge the extraction follows. Sync-side:

- implement a second, disk-backed resolver in `project/settings.rs` that
  resolves modules the same way (`module_file` path probing) but serves
  source via `db.read_file` (untracked, overlay-aware) instead of
  `file.source(db)`, and **records every path it serves**;
- `settings_source_files` becomes: run `extract_settings` over the settings
  file's *disk* content with the recording disk resolver, discard the
  `DjangoSettings`, return the root file plus the recorded files.

Fidelity is then identical to extraction **by construction** — same walk, same
truthiness handling, same cycle/cache semantics (the extractor may ask the
resolver repeatedly for the same module; recording into a set dedups, as the
current `seen` set does). `collect_settings_source_files`, its hand-rolled
parse loop, and the second walk implementation are deleted. The cost — running
the full extraction once per explicit refresh — is the same order as today's
walker, which already parses every file in the closure; refresh is rare
(initialize / explicit refresh, sync.rs:13-26).

`djls-project`'s public API is untouched: the `SettingsSourceResolver` trait is
already the seam, and the recording adapter is ~15 lines in djls-semantic. (If
an explicit seam is ever wanted, a recording resolver wrapper could live beside
the extractor — but the narrow-API constraint says don't until a second
consumer exists.)

**D. A Salsa "settings source graph" query that extraction depends on.**
Rejected as the fake layer the prompt warns about. The graph is
evaluation-dependent (truthiness gates which imports are followed), so a
standalone graph query either re-implements the evaluator or over-approximates;
extraction cannot consume it without redoing its own walk anyway; and Salsa
already records the true read-set as dependency edges — reifying a second,
parallel "graph" creates two sources of truth for one fact. The only thing the
graph query would add is enumerability, which option C gets from the existing
resolver seam without a new query.

## Recommended model

Option C, stated as roles:

- **Extraction** (`django_settings`) is a pure tracked projection of the Salsa
  view. It never writes, never bumps, and its invalidation footprint is
  whatever Salsa recorded — `file.source` per closure member plus root
  revisions. Unchanged.
- **The refresh boundary** (`sync.rs`) is the only revision writer. To learn
  *what* to bump, it asks the same evaluator to walk the **disk** view through
  a recording resolver. `settings_source_files` keeps its name, signature, and
  untracked/`pub(super)` placement; only its implementation changes.
- **The resolver trait** is the single seam where the two worlds meet; there
  is exactly one walk implementation (the extractor's), driven by two
  resolvers (Salsa-sourced and disk-sourced).

## Salsa invalidation implications (Q4)

- **Which files must be read by which tracked functions:** `django_settings`
  reads (via `file.source`) exactly the closure it evaluates — already true,
  no change. `settings_module_file` reads no sources, only path probes covered
  by root touches. Dependency files must **not** be touched inside extraction
  beyond what it actually reads (no speculative reads to "register"
  dependencies); the disk-walk is sync's job.
- **Where dependency files are bumped:** inside sync, as today
  (sync.rs:42-44). Not inside extraction (tracked code can't write), and not
  through a graph query (option D).
- **The untracked-probe invariant**, worth writing down where the resolver
  lives: *every untracked filesystem probe inside a tracked query must be
  covered by a root-revision dependency on each root probed.* That is what
  `touch_search_path_roots` (input.rs:104-115) is for, and why `module_file`
  touches roots before probing (settings.rs:497). The probes themselves
  (`module_file_in_search_path`, resolve.rs:208-225; `path_is_dir`/`walk_entries`)
  are invisible to Salsa; the root revision is the proxy that makes refresh
  re-run them.
- **The footprint invariant** the fix establishes: *the refresh bump set must
  be a superset of the extraction read-set as evaluated against current disk.*
  Option C makes this hold by construction; option A makes it hold by
  discipline.

## Crate-boundary implications (Q5)

| Piece | Now | Later (plan 015) |
|---|---|---|
| Settings AST walking (`extract_settings`, truthiness, branch joins) | `djls-project` — already there, stays the only walker | unchanged |
| `DjangoSettings` and the resolver trait | `djls-project` | unchanged |
| Salsa query wrapping (`settings_module_file`, `django_settings`), both resolvers, `settings_source_files` | `djls-semantic/src/project/settings.rs` | moves to `djls-project` with the project model, per plan 015's move table (`settings.rs` row) |
| The refresh boundary (`sync.rs`) | `djls-semantic` | moves with plan 015 (`sync.rs` row) |

Plan 015 does not solve this issue — it relocates whichever implementation
exists. Doing option C first means deleting `collect_settings_source_files`
*before* the move, so 015 moves one walker instead of two. No new
`djls-project` public API in either ordering.

## Interaction with the existing seams (Q6)

- `settings_module_file` stays the single root anchor; both projections start
  from it (the disk-walk should resolve the root the same way, reading its
  content via `read_file`).
- `Project::touch_search_path_roots`: the disk resolver does not need it (it
  is not in tracked context), but reusing `module_file` as-is is harmless —
  reading a root revision outside a query is a no-op. Minor cleanup
  opportunity, not a requirement: `module_file` re-touches all roots on every
  call inside tracked loops (settings.rs:497); hoisting the touch to query
  entry (as `template_dirs`/`template_libraries` already do at
  settings.rs:78/122) would make the per-call touch redundant. Salsa dedups
  dependency edges, so this is noise, not a bug.
- Star-import resolution: `resolve_star_import_module`'s relative-import logic
  (settings.rs:527-556, `ModuleFileParts`) is shared by both resolvers
  already — it stays the single implementation both drivers call.

## Invariants

1. Refresh bump set ⊇ extraction read-set under current disk content
   (by construction: same walker).
2. Extraction is a pure projection: tracked, no revision writes, no reads
   beyond what it evaluates.
3. `sync.rs` is the only revision writer.
4. One star-import walk implementation; resolvers differ only in where source
   text comes from (memoized `file.source` vs. direct `read_file`).
5. Untracked probes inside tracked queries are covered by root-revision
   dependencies on every probed root.

## Phased implementation outline

1. **Replace the walker** (small, self-contained; suitable as a PR #664
   follow-up commit or its own change): add the disk-backed recording resolver
   in `project/settings.rs`; reimplement `settings_source_files` as
   "extract-and-discard over disk content"; delete
   `collect_settings_source_files`. No signature changes; sync.rs untouched.
2. **Regression tests** (the reason this memo exists): in the djls-db
   invalidation suite (the source-mutation pattern plan 008 Step 4
   established), cover (a) the `try:/except ImportError` star import — edit
   the imported file's content behind the filesystem, call
   `refresh_external_data`, assert `django_settings` reflects the new
   content; (b) a star import nested under an ambiguous `if`; (c) the
   new-edge case — settings file gains a star import of an already-known
   stale `File`, refresh, assert fresh content is seen.
3. **Optional tidy:** hoist the root touch out of `module_file` to the
   tracked-query entries; document the two invariants (footprint superset,
   untracked-probe coverage) as comments at the resolver and in `sync.rs`'s
   module doc, which is where the next person will look.
4. **Plan 015** then moves `settings.rs` + `sync.rs` as written — one walker,
   two resolvers, same trait seam.

## Validation strategy

- `cargo test -q -p djls-semantic -p djls-db` — existing layering tests
  (settings.rs:752-794) must pass unchanged; the three new invalidation tests
  from phase 2 are the acceptance bar (test (a) should fail against today's
  implementation — write it first to prove the bug, then fix).
- `just e2e` — the fixture project flows through `refresh_external_data` on
  initialize; parity must hold.
- Done check: `rg -n "collect_settings_source_files" crates/` returns no
  matches; `rg -n "ruff_python_parser" crates/djls-semantic/src/project/` returns
  no matches (the sync-side re-parse is gone; parsing happens only in the
  extractor and the tracked `parse_python_module`).
