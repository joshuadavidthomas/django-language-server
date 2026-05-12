# now

## Valid

### renders a date format

```htmldjango
{% now "Y-m-d" %}
```

```snapshot
✓ no diagnostics
```

### renders a date and time format

```htmldjango
{% now "jS F Y H:i" %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects missing format

```htmldjango
{% now %}
```

```snapshot
error[S117]: 'now' takes exactly 1 argument, 0 given
 --> test.html:1:1
  |
1 | {% now %}
  | ^^^^^^^^^
  |
  = note: in tag: now
```

### rejects too many arguments

```htmldjango
{% now "Y" "m" %}
```

```snapshot
error[S117]: 'now' takes exactly 1 argument, 2 given
 --> test.html:1:1
  |
1 | {% now "Y" "m" %}
  | ^^^^^^^^^^^^^^^^^
  |
  = note: in tag: now
```
