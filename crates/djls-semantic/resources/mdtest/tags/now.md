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
error[S117]: Tag 'now' takes exactly 1 argument, but 0 were given
 --> test.html:1:1
  |
1 | {% now %}
  | ^^^^^^^^^
```

### rejects too many arguments

```htmldjango
{% now "Y" "m" %}
```

```snapshot
error[S117]: Tag 'now' takes exactly 1 argument, but 2 were given
 --> test.html:1:1
  |
1 | {% now "Y" "m" %}
  | ^^^^^^^^^^^^^^^^^
```
