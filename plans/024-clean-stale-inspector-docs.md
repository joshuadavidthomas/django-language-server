# Plan 024: Clean stale inspector documentation

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report; do not improvise. When done, update the status row for this plan in
> `plans/README.md`.
>
> **Drift check (run first)**: This plan is written against fetched `main` at
> commit `bbff2406` (`server: defer project loading from initialize (#679)`,
> 2026-06-21). Run
> `jj diff --stat --from bbff2406 --to @ -- docs tests README.md CONTRIBUTING.md ARCHITECTURE.md CONTEXT.md`
> and then run the inventory command in Step 1. The runtime inspector was
> deleted by plan 009; this plan is documentation cleanup only.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/009, plans/012
- **Category**: docs (static-discovery cleanup)
- **Planned at**: commit `bbff2406`, 2026-06-21
- **Execution status**: DONE — implemented on
  `plan-024-clean-stale-inspector-docs` at source head `60d3ac50`

## Execution record — source head `60d3ac50`

Implemented the docs-only cleanup for stale runtime inspector language.

Changed:

- `docs/configuration/index.md`: rewrote environment-variable, `pythonpath`,
  `env_file`, and diagnostic-code wording in static Project Facts terms.
- `docs/clients/sublime-text.md`: replaced project introspection wording with
  Project Facts / project-specific template-library wording.
- `CONTRIBUTING.md`: updated the development overview to say DJLS derives
  Django Project Facts through static analysis.
- `tests/project/djls_app/templates/djls_app/tags/scoping.html`: replaced the
  Python runtime inspector inventory comment with "static project facts".
- `tests/project/djls_app/templates/djls_app/tags/structural.html`: clarified
  that structural diagnostics are syntax-only and always available.

Final inventory:

- `CONTEXT.md:35` is glossary guidance saying not to call static extraction
  "introspection".
- `ARCHITECTURE.md:164` accurately says there is no embedded inspector zipapp
  and no `django.setup()` in the server path.
- `docs/configuration/index.md:11` accurately says DJLS does not execute
  settings the way `django.setup()` would.

Validation:

- `just fmt --check`
- `just lint`

## Why this matters

The runtime inspector is gone, but some user-facing docs and fixture comments
still describe template diagnostics as depending on an inspector subprocess or
Python runtime inventory. That language is now actively misleading: users may
try to fix stale inspector failures that cannot occur in the static-analysis
server, and future contributors may misread the current architecture.

This is intentionally separate from plan 022. Startup behavior changes should
not be mixed with prose cleanup, and stale docs are safe to fix independently.

## Current state

The current inventory includes stale references such as:

- `ARCHITECTURE.md:164`: accurate current-state wording says startup has no
  embedded inspector zipapp and no `django.setup()`; keep this unless the
  architecture section is otherwise edited for clarity.
- `docs/configuration/index.md:11`: missing variables described as causing the
  inspector process to fail initializing Django.
- `docs/configuration/index.md:17-31`: environment variables described as
  reaching the inspector subprocess.
- `docs/configuration/index.md:83`: `pythonpath` described as paths added
  when the inspector process runs.
- `docs/configuration/index.md:96-98`: `env_file` described as variables
  injected into the inspector subprocess.
- `docs/configuration/index.md:174-198`: tag/filter/library diagnostics
  described as requiring inspector availability; S120 mentions "inspector
  inventory".
- `tests/project/djls_app/templates/djls_app/tags/scoping.html:1`: fixture
  comment says scoping diagnostics require Python runtime inspector inventory.
- `tests/project/djls_app/templates/djls_app/tags/structural.html:1`: current
  wording says structural diagnostics need no Python runtime. That statement
  is accurate, but while touching adjacent fixture comments, consider whether
  the phrase should become "project facts" / "template-library knowledge" for
  consistency.

There are also historical references in
`docs/agents/static-discovery-groundwork/research.md`; those are source
material for the plan stack and should usually remain historical.

## Scope

**In scope**:

- `docs/configuration/index.md`
- `docs/template-validation.md` if current anchors still discuss inspector
  availability
- `docs/clients/*.md` only where wording implies the removed server inspector
  still exists
- `tests/project/djls_app/templates/djls_app/tags/*.html` fixture comments
- small architecture/context wording only if it is current-state prose and
  not historical record

**Out of scope**:

- Changelog/history entries and `docs/agents/static-discovery-groundwork/`
  research notes, unless they claim to describe current behavior.
- Source behavior changes.
- Reworking all configuration docs. Keep the edit focused on stale inspector
  claims.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Inventory | `rg -n "inspector|introspector|django\\.setup|runtime inspector|inspector inventory|Python runtime" docs tests README.md CONTRIBUTING.md ARCHITECTURE.md CONTEXT.md` | only historical or accurate references remain |
| Docs/lint | `just lint` | exit 0 |
| Format check | `just fmt --check` | exit 0 |

## Steps

### Step 1: Inventory current references

Run:

`rg -n "inspector|introspector|django\\.setup|runtime inspector|inspector inventory|Python runtime" docs tests README.md CONTRIBUTING.md ARCHITECTURE.md CONTEXT.md`

Classify each hit as either:

- historical source material or changelog/history that should stay;
- current docs that must be rewritten;
- source/test comments that must be updated.

Do not delete historical context just to make the grep empty.

### Step 2: Rewrite configuration docs in static-discovery terms

In `docs/configuration/index.md`, replace subprocess/inspector wording with
current architecture vocabulary:

- configuration and environment values feed project settings and static
  discovery;
- `pythonpath` contributes import search roots for static module/template-tag
  discovery;
- `env_file` provides environment variables used while deriving project facts,
  not variables forwarded to a subprocess;
- diagnostics depend on available static project facts, not inspector
  availability.

Before making a specific claim about how `env_file` is consumed, verify the
current source path (`Project` inputs, settings extraction, tag specs, and
refresh load/apply). If the current behavior is narrower than the old docs
implied, document the narrower truth rather than preserving the old promise.

### Step 3: Update template-validation anchors and diagnostic wording

If `docs/template-validation.md` still exposes an
`#inspector-availability` anchor, replace it with a static-discovery/current
facts anchor and update inbound links from `docs/configuration/index.md`.

Update diagnostic descriptions like "not found in inspector inventory" to
describe the actual source of knowledge, for example "not found among known
template tag libraries" if that matches current behavior.

### Step 4: Update fixture comments

Update stale comments under `tests/project/` that describe inspector/Python
runtime prerequisites. Keep comments short and factual, e.g. "requires
project facts" or "requires static template-library knowledge" if that is the
current distinction.

### Step 5: Validate and leave a useful grep trail

Run the inventory command again. Remaining hits should be historical or
accurate current architecture statements. If any current docs still use
"inspector" or "introspector", either rewrite them or leave a short note in
the plan execution record explaining why the term is still accurate.

Run `just lint` and `just fmt --check`.

## Done criteria

- [ ] User-facing docs no longer describe a runtime inspector subprocess as a
  current part of the server.
- [ ] `docs/configuration/index.md` links to current static-discovery/project
  facts terminology, not inspector availability.
- [ ] Fixture comments under `tests/project/` no longer say diagnostics
  require Python runtime inspector inventory.
- [ ] Historical research/changelog references are preserved where useful.
- [ ] The final grep inventory has only historical or accurate current hits.
- [ ] `just lint` and `just fmt --check` pass.

## STOP conditions

- You cannot verify how a configuration option is currently consumed by static
  discovery. Stop and ask for a source-behavior decision instead of guessing.
- Fixing the docs reveals a real behavior mismatch in settings/env handling.
  Record the docs finding, but move source behavior into a separate plan.
- The edit starts expanding into a broad docs restructure. Keep this pass
  limited to stale inspector language.

## Maintenance notes

- Use the terminology from `CONTEXT.md`: Project Facts, static extraction,
  template-library knowledge, and project meaning.
- Do not use "inspector" as a generic synonym for static discovery. In this
  repo it now names removed runtime machinery or historical plans.
