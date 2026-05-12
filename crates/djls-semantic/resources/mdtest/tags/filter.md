# filter

## Valid

### applies one filter to block contents

```htmldjango
{% filter lower %}
  THIS WILL BE LOWERCASED
{% endfilter %}
```

```snapshot
✓ no diagnostics
```

### applies a filter chain to block contents

```htmldjango
{% filter lower|truncatewords:5 %}
  This is some text that will be lowered and truncated.
{% endfilter %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### reports unclosed filter block

```htmldjango
{% filter upper %}
  Never closed.
```

```snapshot
error[S100]: Unclosed tag: filter
 --> test.html:1:1
  |
1 | {% filter upper %}
  | ^^^^^^^^^^^^^^^^^^
```
