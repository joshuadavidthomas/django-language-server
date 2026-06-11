# Memo: Template-library domain model

> Design memo — no implementation. Written against working-copy commit
> `710f4107` (branch `plan-008-derive-template-libraries-from-source`,
> PR #664), including the uncommitted edits to
> `crates/djls-semantic/src/project/{settings,symbols}.rs`. All line
> references are to that state.

## Problem statement

Static discovery (plan 008) derives Django template tag libraries from four
sources: app `templatetags/*.py` packages, `TEMPLATES[*]["OPTIONS"]["libraries"]`,
Django's default builtins, and `OPTIONS["builtins"]`. The model represents the
loadable/preloaded difference as a *kind* on the library value —
`LibraryStatus::{Active, Builtin}` — when it is actually a property of *where
the library is mounted in the project*. A module listed in `OPTIONS["builtins"]`
is an ordinary template tag library that happens to be available without
`{% load %}`. "Active vs Builtin" is also a false opposition: builtins are
active too. The question is what the real domain concepts are and at what level
the loadable/preloaded distinction belongs.

## Current model summary

`crates/djls-semantic/src/project/symbols.rs`:

- `LibraryStatus` (symbols.rs:67-75): `Active { module, origin: Option<LibraryOrigin> }`
  | `Builtin { module }`. Both arms carry `module`; only `Active` carries
  provenance.
- `TemplateLibrary` (symbols.rs:77-83): `{ name: LibraryName, status, symbols }`.
- `TemplateLibraries` (symbols.rs:174-179):
  `{ knowledge: StaticKnowledge, loadable: BTreeMap<LibraryName, Vec<TemplateLibrary>>, builtins: Vec<TemplateLibrary> }`.

`crates/djls-semantic/src/project/settings.rs`:

- `template_libraries` (settings.rs:119-180) assembles the collection from the
  four sources.
- `apply_derived` (settings.rs:182-192) routes each derived library into a
  bucket *by reading its status*: `Active → set_loadable`, `Builtin → push_builtin`.
- `SettingsLibraryDeclaration::derive` (settings.rs:239-272): the `Builtin` arm
  fabricates a `LibraryName` from the module basename with an `unwrap_or("builtin")`
  fallback (settings.rs:262-264) purely because `TemplateLibrary` demands a name.
- `configured_library` (settings.rs:356-372) is dead code — superseded by
  `SettingsLibraryDeclaration::Loadable`, no callers.

### Vestiges already visible in the current shape

These are inspector-era leftovers that survived the plan 008 rework, and they
are the strongest evidence that the model is misshapen:

1. **`is_active()` always returns true** (symbols.rs:123-129): it matches
   `Active { .. } | Builtin { .. }` — every variant. The third state it used to
   discriminate (discovered-but-not-installed, from the inspector era; see
   CONTEXT.md's flagged ambiguity "describe a Template Tag Library as
   discovered, active, or builtin") was deleted. The active/inactive axis
   collapsed; only the loadable/preloaded axis remains, wearing the old axis's
   name.
2. **`enabled_loadable_libraries()`** (symbols.rs:273-278) filters by
   `is_active` — a no-op filter.
3. **`is_enabled_library()`** (symbols.rs:354-359) is now just "key exists".
4. **`loadable: BTreeMap<_, Vec<TemplateLibrary>>` multiplicity is dead**:
   `set_loadable` (symbols.rs:259-261) always writes `vec![library]`, so every
   value is a one-element Vec, and `best_loadable_library`'s three-stage
   fallback (symbols.rs:325-334: active → has-origin → first) always selects
   element 0.
5. **`LibraryStatus` has zero behavior-bearing consumers.** Outside the
   defining files, nothing matches on it. Its only reader is `apply_derived`'s
   bucket routing — i.e., the value carries a tag whose sole purpose is to tell
   the collection which field to put it in. `module()` (symbols.rs:108-113)
   unwraps both arms identically. `origin()` (symbols.rs:115-121) is consumed
   only by the vestigial fallback in (4). This is exactly the
   no-behaviorless-type-distinctions smell.
6. **Builtin names are write-only.** The fabricated basename never feeds
   anything: `push_builtin` dedups by module (symbols.rs:263-271), candidate
   keying uses module (symbols.rs:288-305), hover and completions display the
   module. Nothing reads a builtin's `LibraryName`.
7. **Serde on these types is likely dead.** `Serialize/Deserialize` existed for
   the inspector disk cache, which plan 008 deleted. The golden-fixture test
   deserializes its own `GoldenTemplateLibraries` type (settings.rs:654-659),
   not `TemplateLibraries`. Worth a `rg`-verify during implementation.

## Django semantic constraints

- An app's `templatetags/foo.py` creates a loadable library whose load name is
  the file stem `foo`.
- `OPTIONS["libraries"]` maps an explicit load name to a module path; it
  overrides an app-scanned library with the same load name (tested:
  settings.rs:1094-1145).
- Django's default builtins (`defaulttags`, `defaultfilters`, `loader_tags`) and
  `OPTIONS["builtins"]` entries are available without `{% load %}`. They have
  no load name *as builtins* — `{% load %}` resolves load names, not module
  paths.
- The same module can be mounted both ways (e.g. `django.templatetags.static`
  added to `OPTIONS["builtins"]` while remaining loadable as `static`).
- Builtin order is symbol precedence: a later builtin's symbol shadows an
  earlier one's. Consumed today by `installed_symbol_candidates`'s last-wins
  BTreeMap insert (symbols.rs:288-305), by `registration_modules()` order
  (symbols.rs:199-216) feeding `templatetag_modules` (resolve.rs:227-267) and
  the last-wins filter-arity merge (filters.rs:81-94), and asserted by the
  golden fixture (settings.rs:1233-1237).
- Loadable libraries drive `{% load %}` resolution, unloaded-symbol diagnostics,
  and load-name completion. Their collection order is not semantic; their key is.

## The actual domain concepts (Q1)

| Concept | What it is | Where it shows up |
|---|---|---|
| **Template tag library** | A Python module with a `Library()` registration and its tag/filter symbols. Identity = `PyModuleName`. | The substance. Identical regardless of how it was mounted. |
| **Load name** | The `{% load X %}` token. A *binding* of a name to a library in the project's loadable namespace. | App scan derives it from the file stem; `OPTIONS["libraries"]` declares it explicitly. Builtins have none. |
| **Mount** (availability mode) | Whether the library is preloaded (no `{% load %}` needed) or loadable (needs it). | A property of the project configuration, not of the module. |
| **Provenance** | Which declaration produced the mount (app scan / OPTIONS libraries / default builtin / OPTIONS builtins). | Currently `LibraryOrigin` (symbols.rs:59-64) — constructed (settings.rs:343-347) but consumed by nothing outside the vestigial fallback. |
| **Precedence** | Order among *preloaded* libraries only. | Builtin Vec order; see constraints above. |
| **Knowledge** | How much of the inventory we trust (`StaticKnowledge`). | Collection-level; gates diagnostics and completions. |

Note that the codebase **already has the correct model at the projection
layer**: `InstalledSymbolOrigin::{Builtin { module }, Loadable { load_name }}`
(symbols.rs:162-166) is precisely "mount + the identity appropriate to that
mount" — module for preloaded, load name for loadable. Hover branches on it
(hover.rs:144-150), completions branch on it (completions.rs:376-392). The
recommendation below makes the storage layer match the projection layer that
consumers already proved out.

## Where each distinction belongs (Q2)

- **On `TemplateLibrary` (the value):** module identity and symbols. Nothing
  else has a consumer.
- **On the collection (`TemplateLibraries`):** the mount. Loadable-ness is
  being-in-the-`loadable`-map (keyed by load name); preloaded-ness is
  being-in-the-`builtins`-list (ordered). The distinction becomes *positional*
  instead of *tagged* — which is what `apply_derived` was already reducing it
  to, one hop late.
- **Nowhere (until a consumer exists):** provenance. `LibraryOrigin` is
  currently decorative. Plan 018's diagnostics ("installed but not in
  INSTALLED_APPS") will likely want app attribution — reintroduce provenance
  *then*, shaped by that consumer, rather than carrying a speculative struct
  now.

## Verdict on `LibraryStatus::{Active, Builtin}` (Q3)

Invalid, not merely misnamed. Three independent defects:

1. "Active" names a dead axis (active vs. discovered-but-inactive) from the
   inspector era; after static derivation everything in the collection is
   active, which is why `is_active()` is constant-true.
2. "Builtin" as the opposing variant conflates the live axis (loadable vs.
   preloaded) with the dead one, producing the false opposition the prompt
   identifies: builtins are active too.
3. The enum smuggles provenance (`origin`) into one arm of an availability
   distinction — two unrelated concerns in one type.

Renaming the variants (e.g. `Loadable`/`Preloaded`) would fix only defect 2.
The structural fix is deletion: with the mount positional, no per-value tag
remains to misname.

## Should a preloaded library have a `LibraryName`? (Q4)

No. The fabricated basename (settings.rs:262-264) exists only to satisfy the
struct, is read by nothing (vestige 6 above), and is *wrong* as a load name —
`{% load defaulttags %}` does not work; Django's builtins are not addressable
by basename. Its replacement is the `PyModuleName` the builtin already carries:
dedup by module, candidate origin by module, display by module — all of which
the code already does. If a builtin module is also loadable, that fact lives in
the `loadable` map as a separate entry, which is the truthful representation
(it genuinely is mounted twice).

## Candidate shapes (Q5/Q6 context)

**A. Keep the enum, rename variants** (`Loadable`/`Preloaded`). Minimal churn;
keeps the routing-by-tag in `apply_derived`, the fabricated names, the dead
multiplicity, and a value-level tag with no branching consumer. Rejected: fixes
the name, not the model.

**B. Mount as collection position; library value = module + symbols
(recommended).** Conceptually:

- `TemplateLibrary { module: PyModuleName, symbols: Vec<TemplateSymbol> }`
- `TemplateLibraries { knowledge, loadable: BTreeMap<LibraryName, TemplateLibrary>, builtins: Vec<TemplateLibrary> }`

The load name lives only as the map key (today it is duplicated between the key
and `library.name`). `LibraryStatus`, `LibraryOrigin`, the fabricated builtin
names, the per-name `Vec`, and the always-true predicates all disappear.
Derivation helpers return *libraries plus knowledge*; the declaration site —
which is the only place that knows whether a declaration is a load-name binding
or a builtin mount — inserts into the right index directly, instead of tagging
the value and letting `apply_derived` re-discover the routing.

**C. Single list + mount enum on each entry**
(`Vec<(Mount, TemplateLibrary)>` with `Mount::{Loadable(LibraryName), Builtin}`).
Honest, but every consumer pays a filter: scoping wants "all builtins then all
loadables" (scoping/symbols.rs:121-161), completions want the loadable keys
(symbols.rs:238-249), `{% load %}` resolution wants map lookup by name. The two
access patterns are exactly the two indexes of shape B; C just makes consumers
rebuild them. Rejected.

## Recommended model

Shape B. Naming recommendations:

- Keep **`TemplateLibrary`** for the value and **`TemplateLibraries`** for the
  collection — both still mean what they say.
- Keep **`builtins`** as the preloaded index's field name. It is Django's own
  word for this mount (`OPTIONS["builtins"]`, `Engine(builtins=...)`), and the
  objection was never to the word — it was to "Builtin" as a *kind of library*
  opposed to "Active". As a mount name on the collection it is
  dependency-native vocabulary at the Django seam. (If a non-Django-shadowing
  name is preferred, `preloaded` is the alternative; do not invent a third
  term.)
- Rename the degenerate predicates to what they now mean:
  `is_enabled_library` → membership check on `loadable`
  (its consumers at scoping/loads.rs:220-222 are asking "is this a known load
  name"); `enabled_loadable_libraries` → `loadable_libraries`;
  `best_loadable_library` → plain `loadable_library` returning
  `Option<&TemplateLibrary>` (the "best" selection died with the multiplicity).
- Delete `configured_library` (settings.rs:356-372) — already dead.
- `SettingsLibraryDeclaration` (settings.rs:228-237) remains a good seam: it is
  the declaration-kind type. Its `derive` should return the library value (+
  knowledge); the caller mounts it. `DerivedTemplateLibraries`
  (settings.rs:194-226) survives as `(knowledge, Vec<TemplateLibrary>)` for the
  app-scan path, but `apply_derived`'s status-routing disappears — app-scan
  results are all loadable by construction, so the call site inserts them as
  such.

How derivation sharing works without erasing the semantic difference (Q5): all
four sources share the *library construction* path (resolve module → parse →
registry scan → symbols; today `library_with_symbols` +
`TemplateLibraryAnalysis`, settings.rs:374-447). They differ only in the
*binding* they declare — `(LibraryName → module)` vs `(position → module)` —
and that difference is expressed by which index the declaration site writes,
not by a tag on the value. The semantic difference is preserved exactly where
it is meaningful and erased exactly where it was noise.

## Consumer impact (Q6)

- **Scoping availability** (`scoping/symbols.rs:90-205`): `builtin_libraries()`
  iteration unchanged; the loadable loop drops its no-op `is_active` filter.
  `AvailableSymbols`, `TagAvailability`, `SymbolIndex` untouched.
- **Unloaded diagnostics** (`validation/scoping.rs:102-139`): the
  `loadable.contains_key` check (line 123) unchanged; reads become simpler
  (no Vec).
- **Completions** (`completions.rs`): `completion_library_names` loses the
  filter; `loadable_library_module` / `best_loadable_library_str`
  (completions.rs:626, 654) become map lookups; detail strings and the
  `InstalledSymbolOrigin` branches (completions.rs:376-392) unchanged.
- **Hover** (`hover.rs:49`): map lookup; origin-based "Requires {% load %}"
  rendering (hover.rs:144-150) unchanged.
- **Registration module collection** (`registration_modules`,
  symbols.rs:199-216 → resolve.rs:227-267): same iteration order contract —
  loadables (key order) then builtins (precedence order) — minus the dead
  knowledge guard's interaction with `enabled_*`.
- **Filter arity precedence** (`filters.rs:81-94`): unchanged; depends only on
  `templatetag_modules` order.
- **Test/bench constructors** (`testing.rs:140-215`, `specs.rs:160-180`,
  fixture builders in completions.rs tests): mechanical updates; the
  inspector-era `*_json` helper naming in testing.rs is worth renaming in
  passing since the JSON shape it mimicked is gone.

Net: every consumer either shrinks or is untouched. None gains a branch.

## Invariants the model should enforce

1. One library per load name in `loadable`; later declarations override
   (OPTIONS over app scan) — enforced by map insert order at the assembly site,
   already tested (settings.rs:1094-1145).
2. `builtins` is ordered (defaults first, then per-backend `OPTIONS["builtins"]`
   in declaration order) and deduped by module — order is the symbol-precedence
   contract, golden-tested.
3. A module may appear in both indexes; neither entry implies the other.
4. Builtin entries are structurally nameless (no field to fabricate).
5. `knowledge` only weakens during assembly (`weakened_by` /
   `demoted_to_partial`).
6. Per-library symbols stay sorted and deduped (`merge_symbol`,
   symbols.rs:131-153).

## Phased implementation outline

Sequencing note: do this **before plan 015** moves `project/symbols.rs` and the
derivation into `djls-project` — move the clean shape, not the vestiges. The
types are hot right now from the plan 008 rework (PR #664), so the cheapest
moment is a follow-up commit on that branch or immediately after merge. Plan
015's move table is unaffected (same files); plan 018 should consume the new
shape and is where provenance gets reintroduced if its diagnostics need it.

1. **Dead-code pass (zero behavior change):** delete `configured_library`;
   collapse `loadable` to `BTreeMap<LibraryName, TemplateLibrary>`; delete
   `is_active` and the `best_*` fallback chain (lookup = `get`); rename the
   degenerate predicates; drop serde derives if the `rg` check confirms no
   deserializer. All tests pass unchanged except mechanical constructor edits.
2. **Delete `LibraryStatus`:** hoist `module` onto `TemplateLibrary`; drop
   `name` (key-only) and `origin`/`LibraryOrigin`; split derivation from
   mounting (declaration sites insert into the right index; `apply_derived`'s
   routing goes away). Builtin name fabrication disappears.
3. **Consumer sweep:** scoping, validation, completions, hover, db impls,
   testing/bench fixtures.
4. **Docs:** CONTEXT.md's flagged-ambiguity line "discovered, active, or
   builtin" needs updating — "active" is no longer a state of a library;
   availability vocabulary is `loadable` (requires load) vs `builtin`
   (preloaded), and "discovered" returns only if plan 018 reintroduces it
   with behavior.

## Validation strategy

- `cargo test -q -p djls-semantic -p djls-ide -p djls-db` after each phase.
- Zero insta snapshot changes expected (this is representation, not behavior);
  any snapshot diff is a defect.
- The two `#[ignore]`d golden-fixture tests (settings.rs:1147-1255) via the e2e
  venv (`just e2e` session) — builtin order and load-name→module mapping are
  the contract most at risk from the mounting rework.
- `rg -n "LibraryStatus|new_builtin|new_active|is_active" crates/` returns no
  matches when done.
