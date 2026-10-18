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
```

### elif starts with infix operator

```htmldjango
{% if x %}a{% elif and %}b{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:12
  |
1 | {% if x %}a{% elif and %}b{% endif %}
  |            ^^^^^^^^^^^^^^
```

### elif has no condition

```htmldjango
{% if x %}a{% elif y %}b{% elif %}c{% else %}d{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:25
  |
1 | {% if x %}a{% elif y %}b{% elif %}c{% else %}d{% endif %}
  |                         ^^^^^^^^^^
```

### elif inside a nested block is checked against its own if

```htmldjango
{% if x %}{% for item in items %}{% if item %}{% elif or %}{% endif %}{% endfor %}{% elif y %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'or' in this position in if tag.
 --> test.html:1:47
  |
1 | {% if x %}{% for item in items %}{% if item %}{% elif or %}{% endif %}{% endfor %}{% elif y %}{% endif %}
  |                                               ^^^^^^^^^^^^^
```

## Known gaps

### expression validation is only applied to if and elif

```htmldjango
{% firstof and x %}
```

```snapshot
✓ no diagnostics
```
