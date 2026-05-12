# if

## Valid

### accepts simple truthy expression

```htmldjango
{% if user.is_authenticated %}
  <p>Welcome!</p>
{% endif %}
```

```snapshot
✓ no diagnostics
```

### supports elif and else branches

```htmldjango
{% if user.is_superuser %}
  <p>Admin</p>
{% elif user.is_staff %}
  <p>Staff</p>
{% else %}
  <p>User</p>
{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts boolean and operator

```htmldjango
{% if x and y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts boolean or operator

```htmldjango
{% if x or y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts unary not operator

```htmldjango
{% if not x %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts equality comparison

```htmldjango
{% if x == y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts inequality comparison

```htmldjango
{% if x != y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts ordering comparisons

```htmldjango
{% if x > y %}{% endif %}
{% if x >= y %}{% endif %}
{% if x < y %}{% endif %}
{% if x <= y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts membership operator

```htmldjango
{% if x in items %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts negated membership operator

```htmldjango
{% if x not in items %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts identity operator

```htmldjango
{% if x is None %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts negated identity operator

```htmldjango
{% if x is not None %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts filters in expressions

```htmldjango
{% if items|length > 0 %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### honors operator precedence

```htmldjango
{% if x or y and z %}{% endif %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects expression starting with and

```htmldjango
{% if and %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and %}{% endif %}
  | ^^^^^^^^^^^^
  |
  = note: in tag: if
```

### rejects expression starting with or

```htmldjango
{% if or x %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'or' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if or x %}{% endif %}
  | ^^^^^^^^^^^^^
  |
  = note: in tag: if
```

### rejects expression ending with or

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

### rejects expression ending with and

```htmldjango
{% if x and %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if x and %}{% endif %}
  | ^^^^^^^^^^^^^^
  |
  = note: in tag: if
```

### rejects bare not expression

```htmldjango
{% if not %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if not %}{% endif %}
  | ^^^^^^^^^^^^
  |
  = note: in tag: if
```

### rejects adjacent operands

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

### rejects empty expression

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

### reports else outside if

```htmldjango
{% else %}
```

```snapshot
error[S102]: Orphaned tag 'else' - 'if' or 'ifchanged' block
 --> test.html:1:1
  |
1 | {% else %}
  | ^^^^^^^^^^
```

### reports elif outside if

```htmldjango
{% elif orphaned %}
```

```snapshot
error[S102]: Orphaned tag 'elif' - 'if' block
 --> test.html:1:1
  |
1 | {% elif orphaned %}
  | ^^^^^^^^^^^^^^^^^^^
```

### reports unclosed if

```htmldjango
{% if unclosed %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed tag: if
 --> test.html:1:1
  |
1 | {% if unclosed %}
  | ^^^^^^^^^^^^^^^^^
```
