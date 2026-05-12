# LSP Feature Roadmap

Prioritized by user-visible value, reuse of existing data, and avoiding large architecture work before cheap wins.

1. ✅ **Preserve parse error structure/spans** — GH #443, part of #525
   - Fixed diagnostics that land at `(0,0)` for parser errors with source positions.
   - Parser diagnostics now preserve structured `ParseError` variants and use `T101`–`T110` codes.
   - Improves editor squiggles, CLI rendering, future code actions, and diagnostic docs.

2. **Diagnostics performance first slice** — GH #525
   - Add better edit-sequence benches.
   - Target eager discovered-symbol candidate maps and redundant push-diagnostic flow.
   - Keeps LSP responsiveness healthy before adding more features.

3. **Document symbols** — GH #421
   - High value for low complexity.
   - The semantic forest/block tree already has most outline data.

4. **Document links + precise goto `LocationLink`** — GH #421
   - Bundle around `extends`/`include` string ranges and template resolution.
   - Makes existing navigation feel polished instead of basic.

5. **Hover** — GH #421
   - Table-stakes feature.
   - Start with tag/filter docs, library/source info, and resolved template paths.
   - Avoid variable/type hover until context inference exists.

6. **Template-name completion for `extends` / `include`** — GH #419
   - High daily value.
   - Template discovery already exists; work is completion-context plumbing and prefix filtering.

7. **Code actions for existing diagnostics** — GH #420
   - Start narrow: insert `{% load ... %}`, fix unmatched block names, move first `extends`.
   - Best after parse spans and diagnostic payloads are cleaner.

8. **Finish LSP formatting integration** — GH #422 / formatter roadmap
   - A lot of formatter work is already done.
   - Next useful slice is `textDocument/formatting`, then corpus/idempotency validation.
   - Skip range formatting at first.

9. **Block-name completion and block navigation** — GH #419 / #421
   - Core Django workflow.
   - Needs an extends-chain helper and cycle safety.
   - Sets up better references, rename, code lens, and inheritance UX.

10. **Static settings extraction, Tier 1** — GH #401, supports #485
    - Big strategic reliability win.
    - Start with literal `INSTALLED_APPS` and `TEMPLATES`, not full dynamic settings.
    - If solving complex-project initialization pain is the top goal, move this to #2.

11. **Settings-aware mdtests / diagnostic scenario harness** — GH #399 / #400 / #398
    - Do alongside static settings and expanded diagnostics.
    - Prevents fake fixture behavior from driving wrong validation decisions.

12. **Tag completions without typing `{%`** — GH #506
    - Worth doing after template-name completion.
    - Needs careful trigger-character/manual-completion behavior to avoid noisy completions everywhere.

13. **Python import resolution / ty decision / model graph qualification** — GH #454 / #451
    - Important for ORM and context inference.
    - Not next unless the project pivots into Python/Django semantic analysis.

14. **Template type system and context inference** — GH #424
    - Huge payoff, huge scope.
    - Needs settings extraction, import resolution, and model graph decisions first.

15. **Later polish**
    - Semantic tokens.
    - Inlay hints.
    - Workspace symbols.
    - Document highlights.
    - Selection range.
    - Rename.
    - Code lens.
    - These either depend on stronger symbol/inheritance/context models or have lower immediate payoff.

## Opportunistic small task

**Diagnostic code docs** — GH #343. Do this after GH #443 so the T-series story is not immediately stale.
