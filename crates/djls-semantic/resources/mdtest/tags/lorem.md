# lorem

## Valid

### renders requested paragraph count

```htmldjango
{% lorem 3 p random %}
```

```snapshot
✓ no diagnostics
```

### uses default arguments

```htmldjango
{% lorem %}
```

```snapshot
✓ no diagnostics
```

### accepts count only

```htmldjango
{% lorem 3 %}
```

```snapshot
✓ no diagnostics
```

### accepts output method without randomness

```htmldjango
{% lorem 3 p %}
```

```snapshot
✓ no diagnostics
```

## Invalid

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
