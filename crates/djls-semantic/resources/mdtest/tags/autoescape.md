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
error[S117]: 'autoescape' takes exactly 1 argument, 0 given
 --> test.html:1:1
  |
1 | {% autoescape %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^
  |
  = note: in tag: autoescape
```

### rejects too many arguments

```htmldjango
{% autoescape on off extra %}{% endautoescape %}
```

```snapshot
error[S117]: 'autoescape' takes exactly 1 argument, 3 given
 --> test.html:1:1
  |
1 | {% autoescape on off extra %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: autoescape
```

### rejects unknown mode

```htmldjango
{% autoescape unknown %}{% endautoescape %}
```

```snapshot
error[S117]: 'autoescape' argument must be one of 'on', 'off'
 --> test.html:1:1
  |
1 | {% autoescape unknown %}{% endautoescape %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: autoescape
```
