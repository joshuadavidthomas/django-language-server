# autoescape

## Valid

### enables autoescaping

```htmldjango
{% autoescape on %}
  {{ content }}
{% endautoescape %}
```

```snapshot
✓ no diagnostics
```

### disables autoescaping

```htmldjango
{% autoescape off %}
  {{ content }}
{% endautoescape %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects missing mode

```htmldjango
{% autoescape %}{% endautoescape %}
```

```snapshot
error[S117]: 'autoescape' tag requires exactly one argument.
 --> test.html:1:1
  |
1 | {% autoescape %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^
```

### rejects too many arguments

```htmldjango
{% autoescape on off extra %}{% endautoescape %}
```

```snapshot
error[S117]: 'autoescape' tag requires exactly one argument.
 --> test.html:1:1
  |
1 | {% autoescape on off extra %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects unknown mode

```htmldjango
{% autoescape unknown %}{% endautoescape %}
```

```snapshot
error[S117]: 'autoescape' argument should be 'on' or 'off'
 --> test.html:1:1
  |
1 | {% autoescape unknown %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^
```
