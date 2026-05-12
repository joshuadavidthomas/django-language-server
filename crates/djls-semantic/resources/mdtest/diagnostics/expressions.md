# Expression diagnostics

## if tag

### starts with infix operator

```htmldjango
{% if and x %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}{% endif %}
  | ^^^^^^^^^^^^^^
  |
  = note: in tag: if
```

### ends after infix operator

```htmldjango
{% if x or %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if x or %}{% endif %}
  | ^^^^^^^^^^^^^
  |
  = note: in tag: if
```

### contains unused token

```htmldjango
{% if x y %}{% endif %}
```

```snapshot
error[S114]: Unused 'y' at end of if expression.
 --> test.html:1:1
  |
1 | {% if x y %}{% endif %}
  | ^^^^^^^^^^^^
  |
  = note: in tag: if
```

### has no condition

```htmldjango
{% if %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if %}{% endif %}
  | ^^^^^^^^
  |
  = note: in tag: if
```
