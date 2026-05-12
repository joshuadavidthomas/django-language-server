# lorem

## Valid

### renders requested paragraph count

```htmldjango
{% lorem 3 p random %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects missing arguments

```htmldjango
{% lorem %}
```

```snapshot
error[S117]: Incorrect format for 'lorem' tag
 --> test.html:1:1
  |
1 | {% lorem %}
  | ^^^^^^^^^^^
```

### rejects missing output method and randomness

```htmldjango
{% lorem 3 %}
```

```snapshot
error[S117]: Incorrect format for 'lorem' tag
 --> test.html:1:1
  |
1 | {% lorem 3 %}
  | ^^^^^^^^^^^^^
```

### rejects missing randomness

```htmldjango
{% lorem 3 p %}
```

```snapshot
error[S117]: Incorrect format for 'lorem' tag
 --> test.html:1:1
  |
1 | {% lorem 3 p %}
  | ^^^^^^^^^^^^^^^
```

### rejects too many arguments

```htmldjango
{% lorem 3 p random extra %}
```

```snapshot
error[S117]: Incorrect format for 'lorem' tag
 --> test.html:1:1
  |
1 | {% lorem 3 p random extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
