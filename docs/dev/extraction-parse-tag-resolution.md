# Extraction: Follow `parse_tag` and similar helper functions

## Problem

The extraction crate currently only detects `token.split_contents()` as the
source of tag arguments. But real-world code wraps this in helper functions:

```python
# allauth/templatetags/allauth.py
def parse_tag(token, parser):
    bits = token.split_contents()
    tag_name = bits.pop(0)
    args = []
    kwargs = {}
    for bit in bits:
        match = kwarg_re.match(bit)
        kwarg_format = match and match.group(1)
        if kwarg_format:
            key, value = match.groups()
            kwargs[key] = FilterExpression(value, parser)
        else:
            args.append(FilterExpression(bit, parser))
    return (tag_name, args, kwargs)

@register.tag(name="element")
def do_element(parser, token):
    tag_name, args, kwargs = parse_tag(token, parser)
    if len(args) > 1:
        raise template.TemplateSyntaxError(...)
```

Our extraction sees `len(args) > 1` but doesn't know `args` came from
`split_contents()` via `parse_tag`. Without the `split_contents` detection,
the split variable is `None`, and the fallback heuristic matches `args` by
name — but `args` here is a *filtered* list (only positional args, no kwargs,
tag name already popped), not the raw `split_contents()` result.

This causes false positives: `{% element "form" argument=value %}` gets
flagged as "accepts at most 0 arguments" because `len(args) > 1` with the
raw split_contents count semantics means something different than with the
parsed args semantics.

## Corpus evidence

- **allauth** `element` tag: `parse_tag(token, parser)` → `tag_name, args, kwargs`
  then `if len(args) > 1: raise`. Produces massive false positives across
  all allauth templates.
- **allauth** `user_display` tag: `@register.simple_tag(name="user_display")`
  with `takes_context=True` — separate issue (see below).
- **sentry** `injected_script_assets`: similar pattern.

## Solution approach

### Phase 1: Resolve helper function calls

When the compile function body contains:
```python
tag_name, args, kwargs = parse_tag(token, parser)
```

1. Detect that `parse_tag` is called with `token` as an argument
2. Find the `parse_tag` function definition in the same module
3. Analyze `parse_tag`'s body to find `split_contents()`
4. Track that the return value maps back through the helper

This is essentially **intra-module call resolution**: follow one level of
function calls within the same file to find where `split_contents()` lives.

### Phase 2: Track semantic differences

`parse_tag` doesn't return the raw `split_contents()` list. It:
- Pops the tag name (`bits.pop(0)`)
- Separates positional args from kwargs
- Returns `(tag_name, args, kwargs)` as a tuple

So `len(args) > 1` means "more than 1 positional arg" — the tag name is
already excluded, and kwargs don't count. This is semantically different
from `len(bits) > N` where `bits` includes the tag name.

The extraction needs to understand this transformation to correctly
interpret the constraint. Options:
- Track that `args` is a "processed" variable and apply different offset logic
- Recognize the `parse_tag` return-tuple pattern and adjust constraints
- At minimum, when we can't resolve the semantics, don't emit a constraint
  (no false positive is better than a wrong one)

### Phase 3: Handle common wrapper patterns

Survey the corpus for other wrapper patterns beyond `parse_tag`:
```bash
grep -rn "split_contents" crates/djls-corpus/.corpus/ --include="*.py" | \
  grep -v "token.split_contents\|parser.token.split_contents" | head -20
```

## Interim fix

Until this is implemented, the fallback heuristic that matches `args` by
name should be more conservative. Currently `is_split_var_name` matches
`"bits" | "args" | "parts" | "tokens"` when no split variable is detected.
This is too aggressive — `args` is a very common variable name that often
has nothing to do with `split_contents()`.

Options:
- Remove `"args"` from the fallback list (it's the most ambiguous)
- Only use fallback names when `split_contents()` appears *somewhere* in
  the function body (even if we can't resolve the binding)
- Require the variable to appear in a `len()` call pattern that also
  guards a `TemplateSyntaxError` raise
