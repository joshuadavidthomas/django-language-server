# Plan 022: Make initialize protocol-only and defer project loading

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report; do not improvise. When done, update the status row for this plan in
> `plans/README.md`.
>
> **Drift check (run first)**: This plan is written against fetched `main` at
> commit `23008060` (`Report startup progress and cover startup contract
> (#677)`, 2026-06-13). Run
> `jj diff --stat --from 23008060 --to @ -- crates/djls-server/src crates/djls-db/src crates/djls-project/src crates/djls-conf/src tests/e2e`
> and content-match the "Current state" anchors below. Plans 010, 011, and
> 012 must all be DONE in `plans/README.md`.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/010, plans/011, plans/012
- **Category**: perf / dx (startup-loading follow-up from PR #626)
- **Planned at**: commit `23008060`, 2026-06-13
- **Execution status**: DONE — PR #679 merged into `main` on 2026-06-21 as
  `bbff2406 server: defer project loading from initialize (#679)` (source
  head `1fe76206`)

## Execution record — source head / PR #679 (2026-06-21)

Implemented as the plan-022 startup-loading PR and merged as `bbff2406`.

Implementation notes:

- `Session::new` is protocol-only: it derives the project root and client
  options, builds the workspace and database from already-available client
  settings, records client capabilities, and returns. It does not call
  `Settings::new`, `Project::bootstrap`, env-file loading, Django-settings
  auto-detection, or search-path probing.
- `DjangoDatabase::new` now installs a stable initial `Project` through
  `Project::initial` when a root exists. The initial project has root-only
  search paths, in-memory client overrides, empty env-file variables, and the
  same stable handle later updated by `Project::reload_from_settings`.
- Full settings/project loading moved into the queued refresh path:
  `load_project_settings` captures root and config overrides under the
  session lock, runs `Settings::new` on `spawn_blocking`, then
  `apply_project_settings` stores settings and reloads the stable `Project`
  under a short epoch-checked lock. Project facts are then computed from a
  cloned database, applied briefly, warmed from a snapshot, and republished.
- `did_change_configuration` no longer performs a synchronous settings load;
  it queues the same refresh path with `ProjectRefreshReason::ConfigurationChanged`.
- Final review removed the temporary request-waiting design entirely. There
  is no `wait_for_current_project_refresh`, no request-purpose enum, no
  refresh completion latch, and no timeout. Feature handlers take snapshots
  immediately and answer best-effort while startup refresh is in flight.
- Startup progress remains: the background refresh reports
  `Loading Django project` and its phases, then finishes as `complete`,
  `skipped`, `superseded`, or `failed`. Tests that require fully loaded
  project facts wait for progress completion explicitly.

Validation recorded after the final no-wait correction:

- `uv run pytest tests/e2e/test_startup.py tests/e2e/test_completions.py tests/e2e/test_hover.py tests/e2e/test_navigation.py -q -x`
- `cargo test -q -p djls-server`
- `cargo test -q`
- `just fmt --check`
- `just clippy --allow-dirty`

Planning note: the original full-gate list below remains the desired executor
gate. The final local record did not include a post-correction `just lint`
run; PR merge/CI accepted the change.

## Why this matters

Plans 010-012 made startup responsive once `initialized` queues the project
refresh, but `initialize` still performs project loading work before the
server exists. `Session::new` reads config files and then calls a database
constructor that bootstraps `Project` from disk-backed facts. That means the
LSP handshake is not yet protocol-only: a slow or strange workspace can still
delay the initialize response before progress reporting, snapshot reads, and
the refresh epoch guard have any chance to help.

PR #626's useful idea was not its controller stack; it was the smaller
contract: create a stable, cheap project identity during initialize, then load
project facts in the background path. This plan applies that idea to the
post-static-analysis codebase without resurrecting the PR's startup state
machine.

## Current state

- `crates/djls-server/src/session.rs:51-74`: `Session::new` finds the
  workspace root, builds client settings, calls
  `djls_conf::Settings::new(path, Some(client_settings.clone()))`, then calls
  `DjangoDatabase::new(workspace.overlay(), &settings, project_path.as_deref())`.
- `crates/djls-conf/src/lib.rs:91-123`: `Settings::new` reads user config,
  `pyproject.toml`, `.djls.toml`, and `djls.toml` before applying client
  overrides.
- `crates/djls-db/src/db.rs:97-122`: `DjangoDatabase::new` stores settings
  and calls `set_project`; `set_project` calls `Project::bootstrap`.
- `crates/djls-project/src/project.rs:103-129`: `Project::bootstrap`
  discovers the interpreter, resolves the Django settings module, loads
  `.env`/`env_file`, computes search paths, registers roots, and creates the
  `Project` input.
- `crates/djls-project/src/resolve.rs:87-117` and
  `crates/djls-project/src/python.rs:42-117`: search-path computation probes
  Python path entries and venv/site-packages layout through the filesystem.
- `crates/djls-db/src/settings.rs:36-112`: `set_settings` updates the stable
  `Project` fields via setters, but `update_project_from_settings` still
  parses `env_file` while the caller holds the session lock, and it does not
  run the same Django-settings auto-detection as `Project::bootstrap`.
- `crates/djls-server/src/refresh.rs:93-162`: the refresh task already has
  the right outer shape: report progress, compute off-lock, apply under a
  short lock, warm queries from a snapshot, then republish diagnostics.
- `crates/djls-server/src/server.rs:528-567`: `did_change_configuration`
  still calls `Settings::new` under `with_session_mut`; keep this in view so
  the startup fix does not leave a duplicated settings-load path.

## Scope

**In scope**:

- `crates/djls-server/src/session.rs`
- `crates/djls-server/src/refresh.rs`
- `crates/djls-server/src/server.rs` only for startup/config refresh wiring
- `crates/djls-db/src/db.rs`
- `crates/djls-db/src/settings.rs`
- `crates/djls-project/src/project.rs`
- `crates/djls-project/src/resolve.rs` only if a root-only `SearchPaths`
  constructor belongs there
- focused Rust and e2e tests that pin the new startup contract

**Out of scope**:

- PR #626's `startup.rs`, `discovery_run.rs`, source-file-set model, startup
  controller, or any new queue abstraction.
- File watching, `workspace/diagnostic/refresh`, pull-diagnostics invalidation,
  or multi-environment project loading.
- Changing `djls check` semantics as a side effect. If a shared constructor
  changes, keep the CLI on the eager/full bootstrap path or add an explicit
  CLI bootstrap call.
- Documentation cleanup; plan 024 handles stale inspector wording.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Build server | `cargo build -q -p djls-server` | exit 0 |
| Build CLI | `cargo build -q -p djls` | exit 0 |
| Server tests | `cargo test -q -p djls-server` | exit 0 |
| DB/project tests | `cargo test -q -p djls-db -p djls-project` | exit 0 |
| Startup e2e | `uv run pytest tests/e2e/test_startup.py -q -x` | exit 0 |
| Full Rust tests | `cargo test -q` | exit 0 |
| Matrix | `just test` and `just e2e` | exit 0 |
| Final gates | `just clippy`, `just fmt`, `just lint` | exit 0 |

## Steps

### Step 1: Add a minimal project bootstrap path

Add a small, explicit way to create a stable `Project` input without project
config reads, env-file reads, auto-detection, venv probing, or site-packages
walking. The initial project should:

- keep `Project.root` stable from initialize onward;
- register the first-party workspace root only;
- carry client-provided overrides that require no disk reads, such as explicit
  `django_settings_module`, `venv_path`, `pythonpath`, and tagspec overrides;
- start with empty env-file variables and root-only `search_paths`;
- preserve the existing `Project` handle when the full project load later
  applies settings and derived facts.

Do not make all callers deferred accidentally. `DjangoDatabase::new` is used by
the CLI (`crates/djls/src/commands/check.rs` and `commands/common.rs`) and many
tests. Either keep that constructor eager and add a server-specific constructor,
or make the mode explicit enough that CLI callers still get full project facts.

**Verify**: `cargo build -q -p djls-server` and `cargo build -q -p djls` pass.

### Step 2: Remove config and project discovery from `Session::new`

Change `Session::new` so it only derives protocol inputs from
`InitializeParams`:

1. determine the project root;
2. clone the client settings/options;
3. create `Workspace`;
4. create `DjangoDatabase` with client settings and the minimal project input;
5. create `ClientInfo`;
6. return.

There should be no call to `Settings::new`, `Project::bootstrap`,
`load_env_file`, `resolve_django_settings`, or
`SearchPaths::from_project_settings` on this path.

Add a focused test for the shape. It should fail if initialize calls the full
bootstrap path. A useful pattern is a temporary workspace containing config or
env files whose effects are visible only after refresh: before refresh the
project exists but has root-only search paths and no env-file variables; after
the background project load, the settings-derived values appear.

**Verify**: `cargo test -q -p djls-server` passes.

### Step 3: Move settings/project-input loading into the refresh task

Extend the existing refresh task instead of adding a new startup subsystem.
Before `compute_refresh`, gather full project settings and settings-derived
project inputs off the session lock:

- read `Settings::new(project_root, Some(client_overrides))`;
- resolve Django settings with the same behavior as the old bootstrap,
  including auto-detection;
- parse `.env`/configured `env_file`;
- compute cheap value objects needed to update the existing `Project` input.

Then take the session lock briefly, check the epoch, store settings, and apply
only changed project-input setters to the existing `Project`. Disk work must
already be complete before this lock is held.

After that apply, run the existing `compute_refresh` path against a fresh
database clone so search paths, settings-source files, model modules, and
template tag modules reflect the newly applied project inputs.

Keep the refresh outcome model small. A helper such as `ProjectLoadData` or
`LoadedProjectSettings` is fine; a startup state enum/controller is not.

**Verify**: `cargo test -q -p djls-db -p djls-project` passes.

### Step 4: Route configuration reload through the same load/apply path

Remove the duplicate synchronous config read from
`did_change_configuration`. It should bump/submit refresh work and let the
refresh path load settings off-lock. Diagnostics-only changes can still be
handled by the same refresh: the task applies settings, warms/republishes, and
the epoch guard prevents stale publishes.

If you find a compelling reason to keep a direct diagnostics-only fast path,
it still must not call `Settings::new` or parse env files while holding the
session lock.

**Verify**: config-related tests still pass under `cargo test -q -p djls-db -p djls-server`.

### Step 5: Pin the startup contract

Update `tests/e2e/test_startup.py` so the first test name and assertions match
the new fact: initialize is protocol-only, not merely responsive after the
refresh task starts.

Add at least one Rust test around the new minimal/bootstrap split. The test
must assert both sides of the contract:

- immediately after server/database creation, the stable `Project` exists and
  no disk-derived env/search-path/settings facts have been loaded;
- after the refresh load/apply path, the same `Project` handle has updated
  fields rather than being replaced.

**Verify**: `uv run pytest tests/e2e/test_startup.py -q -x` passes.

### Step 6: Full validation

Run the full gate:

- `cargo test -q`
- `just test`
- `just e2e`
- `just clippy`
- `just fmt`
- `just lint`

## Done criteria

- [ ] `Session::new` contains no `Settings::new`, `Project::bootstrap`,
  env-file parsing, settings auto-detection, or search-path/site-packages
  probing.
- [ ] Initialize creates a stable `Project` when a project root exists; early
  snapshot reads do not have to tolerate a temporary `None -> Some` project
  transition.
- [ ] Full settings/project-input loading happens in the refresh task off the
  session lock, then applies changed Salsa inputs under a short epoch-checked
  lock.
- [ ] `did_change_configuration` no longer performs `Settings::new` under
  `with_session_mut`.
- [ ] CLI/check behavior remains eager enough to preserve current tests and
  user-facing behavior.
- [ ] Focused tests cover the minimal-before-refresh and full-after-refresh
  states.
- [ ] `cargo test -q`, `just test`, `just e2e`, `just clippy`, `just fmt`, and
  `just lint` all pass.

## STOP conditions

- The implementation needs to replace the `Project` input after initialize
  instead of updating the existing handle.
- The implementation needs `Session::new` to read project/user config,
  parse env files, auto-detect settings modules from disk, or probe venvs.
- Settings loading, env-file parsing, or site-packages probing remains under
  the session lock.
- The change starts recreating PR #626's controller/state-machine apparatus.
- CLI behavior changes in a way that requires broad command redesign.

## Maintenance notes

- Keep the invariant simple enough to audit by grep: initialize creates
  protocol state and a minimal stable project; refresh loads project facts.
- If future work adds file watching, it should reuse the same load/apply
  boundary rather than adding a second settings path.
