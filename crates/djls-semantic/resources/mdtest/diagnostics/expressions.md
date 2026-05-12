# Expression diagnostics

## if tag

### starts with infix operator

```htmldjango
{% if and x %}{% endif %}
```

```snapshot
error[S114]: Unexpected 'and' in if expression
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
error[S114]: If expression is incomplete
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
error[S114]: Unexpected 'y' at end of if expression
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
error[S114]: If expression is empty
 --> test.html:1:1
  |
1 | {% if %}{% endif %}
  | ^^^^^^^^
```

## Known gaps

### expression validation is only applied to if and elif

```htmldjango
{% firstof and x %}
```

```snapshot
✓ no diagnostics
```
