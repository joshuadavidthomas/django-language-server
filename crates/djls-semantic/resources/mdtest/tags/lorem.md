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
error[S117]: 'lorem' takes exactly 3 arguments, 0 given
 --> test.html:1:1
  |
1 | {% lorem %}
  | ^^^^^^^^^^^
  |
  = note: in tag: lorem
```

### rejects missing output method and randomness

```htmldjango
{% lorem 3 %}
```

```snapshot
error[S117]: 'lorem' takes exactly 3 arguments, 1 given
 --> test.html:1:1
  |
1 | {% lorem 3 %}
  | ^^^^^^^^^^^^^
  |
  = note: in tag: lorem
```

### rejects missing randomness

```htmldjango
{% lorem 3 p %}
```

```snapshot
error[S117]: 'lorem' takes exactly 3 arguments, 2 given
 --> test.html:1:1
  |
1 | {% lorem 3 p %}
  | ^^^^^^^^^^^^^^^
  |
  = note: in tag: lorem
```

### rejects too many arguments

```htmldjango
{% lorem 3 p random extra %}
```

```snapshot
error[S117]: 'lorem' takes exactly 3 arguments, 4 given
 --> test.html:1:1
  |
1 | {% lorem 3 p random extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: lorem
```
