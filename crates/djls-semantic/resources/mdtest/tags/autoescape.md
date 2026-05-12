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
error[S117]: Tag 'autoescape' takes exactly 1 argument, but 0 were given
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
error[S117]: Tag 'autoescape' takes exactly 1 argument, but 3 were given
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
error[S117]: Tag 'autoescape' argument must be one of: 'on', 'off'
 --> test.html:1:1
  |
1 | {% autoescape unknown %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^
```
