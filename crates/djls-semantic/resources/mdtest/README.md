# Markdown diagnostic snapshots

These files are executable examples for Django template diagnostics. They are meant to be easy to read, write, and review.

Run them with:

```bash
cargo test -p djls-semantic markdown_diagnostic_snapshots -- --nocapture
```

Update generated snapshots with:

```bash
DJLS_UPDATE_MDTEST_SNAPSHOTS=1 cargo test -p djls-semantic markdown_diagnostic_snapshots -- --nocapture
```

## Authoring format

Any Markdown heading can define a scenario when its section contains one Django template code block:

````markdown
# if

## Invalid

### rejects empty expression

```htmldjango
{% if %}{% endif %}
```
````

Headings without template code blocks are just grouping. Start flat if that is easier, then group later:

````markdown
# if

## rejects empty expression

```htmldjango
{% if %}{% endif %}
```
````

The runner accepts `htmldjango`, `django`, and `html` fences as template source. It writes snapshots in `snapshot` fences. A scenario with no diagnostics renders as:

```snapshot
✓ no diagnostics
```

## Scenario rules

- Use one template code block per scenario.
- Treat the template code block as terminal for that heading section.
- Put the generated `snapshot` block directly after the template block.
- Do not put child headings below a heading after it has a template block.
- Use `## Valid`, `## Invalid`, and `## Known gaps` when grouping helps readability.

For non-default paths, put a backtick label immediately before the template block:

````markdown
`templates/example.html`:

```htmldjango
{% else %}
```
````

## Current scope

The mdtest runner uses `pulldown-cmark` for Markdown parsing, but the mdtest format is intentionally small: heading groups, fenced template code blocks, optional file labels, and generated snapshot fences.

Snapshots run against the curated validation fixture in `src/testing.rs`, not a live inspected Django project. That keeps tests deterministic. Long term, we may add fixtures that exercise more real project discovery behavior.
