# Extraction Test Strategy: Corpus-Grounded Tests

## Problem

The extraction crate (`djls-python`) tests Python source analysis — parsing
Django templatetag/filter registration files and extracting validation rules,
block specs, filter arities, etc.

Currently, most tests use **fabricated Python snippets** that may not reflect
real-world patterns. This creates risks:

1. Testing patterns that don't exist in the wild
2. Missing patterns that DO exist in real code
3. Building logic around imaginary edge cases while ignoring real ones
4. Golden test snapshots that encode behavior for fake code (e.g., the
   `golden_keyword_position_check` test used a fabricated `cycle` tag that
   doesn't match Django's actual `cycle` implementation)

## Principle

**The corpus is the single source of truth for extraction tests.**

The `djls-corpus` crate syncs real Python source from:
- Django (multiple versions: 4.2, 5.1, 5.2, 6.0)
- Third-party packages (debug-toolbar, allauth, crispy-forms, wagtail, compressor)
- Real project repos (sentry, netbox)

All extraction tests that take Python source as input should be grounded in
this corpus.

## Boundary

| Test type | Source | Fabricated OK? |
|-----------|--------|----------------|
| Extraction (Python → rules/registrations/specs) | Corpus files | **No** — use real code |
| Template parser (Django template syntax) | Template strings | Yes — that's what users type |
| Pure Rust logic (constraint comparison, key hashing) | Rust values | Yes — no Python involved |
| User-reported bugs (custom templatetags) | Bug report repros | Yes — once we have them |

## Work Items

### Phase 1: Audit existing fabricated tests

- [ ] Inventory all tests in `djls-python` that use inline Python source
  - `src/lib.rs` — golden snapshot tests (~40 tests)
  - `src/rules.rs` — unit tests for rule extraction (~30 tests)
  - `src/registry.rs` — unit tests for registration discovery (~20 tests)
  - `src/context.rs` — unit tests for split_var detection (~10 tests)
  - `src/blocks.rs` — unit tests for block spec extraction
  - `src/filters.rs` — unit tests for filter arity extraction
- [ ] For each test, determine if a corpus example covers the same pattern
- [ ] Identify patterns tested by fabricated code that have NO corpus equivalent
  (these are candidates for removal or for finding new corpus sources)

### Phase 2: Find corpus coverage for each extraction pattern

For each extraction feature, find the real-world corpus file(s) that exercise it:

**Registration patterns:**
- `@register.tag` (bare decorator) → Django `defaulttags.py`
- `@register.tag("name")` (positional name) → Django `defaulttags.py`
- `@register.simple_tag` → various third-party
- `@register.inclusion_tag` → various third-party
- `@register.filter` → Django `defaultfilters.py`
- `register.tag("name", func)` (call-style) → Django `defaulttags.py`
- `register.filter("name", func)` (call-style) → Django `defaultfilters.py`

**Rule extraction patterns:**
- `len(bits) < N` / `> N` / `!= N` guards → Django `defaulttags.py`
- `bits[N] != "keyword"` checks → Django `defaulttags.py`, `i18n.py`
- `not (N <= len(bits) <= M)` range checks → find in corpus
- `len(bits) > N and bits[M] != "kw"` guard pattern → Django `static.py`
- While-loop option parsing → Django `defaulttags.py` (`include` tag)
- Tuple unpacking of split var → find in corpus
- Indexed access patterns → find in corpus

**Block spec patterns:**
- `parser.parse(("endfor",))` → Django `defaulttags.py`
- `parser.parse(("else", "endif"))` intermediates → Django `defaulttags.py`
- `parser.skip_past("endverbatim")` opaque → Django `defaulttags.py`

**Filter arity patterns:**
- No-arg filter `def lower(value)` → Django `defaultfilters.py`
- Required-arg filter `def default(value, arg)` → Django `defaultfilters.py`
- Optional-arg filter `def truncatewords(value, arg=None)` → find in corpus

### Phase 3: Replace fabricated tests with corpus-sourced tests

- [ ] Create test helpers that load source from corpus files by path
- [ ] For each fabricated test, either:
  - Replace with a corpus-sourced equivalent that tests the same pattern
  - Remove if the pattern doesn't exist in real code (it was imaginary)
  - Keep temporarily with a `// TODO: find corpus source` comment if we
    can't find a corpus example but the pattern seems plausible
- [ ] Update all insta snapshots to reflect real code output

### Phase 4: Add corpus regression tests

- [ ] For each corpus file, run full extraction and snapshot the result
- [ ] These become regression tests — if extraction behavior changes,
  snapshots show exactly what changed against real code
- [ ] Consider testing across Django versions (4.2 vs 5.2 vs 6.0) to
  catch version-specific patterns

## Notes

- The corpus tests already have infrastructure: `find_corpus_dir()` checks
  `DJLS_CORPUS_PATH` env var + relative paths. Tests skip gracefully when
  corpus isn't present.
- Golden tests using `find_django_source()` already exist for some patterns.
- The `module_path_from_file()` helper derives Python module paths from
  corpus file paths.
- Tests should be gated on corpus availability (skip when not synced) so
  CI can control when corpus tests run.
