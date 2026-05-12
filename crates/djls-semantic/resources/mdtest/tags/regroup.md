# regroup

## Valid

### groups items by attribute

```htmldjango
{% regroup items by category as grouped %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects missing arguments

```htmldjango
{% regroup %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 0 given
 --> test.html:1:1
  |
1 | {% regroup %}
  | ^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### rejects missing by keyword and target

```htmldjango
{% regroup items %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 1 given
 --> test.html:1:1
  |
1 | {% regroup items %}
  | ^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### rejects missing regroup attribute and target

```htmldjango
{% regroup items by %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 2 given
 --> test.html:1:1
  |
1 | {% regroup items by %}
  | ^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### rejects missing as keyword and target variable

```htmldjango
{% regroup items by category %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 3 given
 --> test.html:1:1
  |
1 | {% regroup items by category %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### rejects missing target variable

```htmldjango
{% regroup items by category as %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 4 given
 --> test.html:1:1
  |
1 | {% regroup items by category as %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### rejects too many arguments

```htmldjango
{% regroup items by category as grouped extra %}
```

```snapshot
error[S117]: 'regroup' takes exactly 5 arguments, 6 given
 --> test.html:1:1
  |
1 | {% regroup items by category as grouped extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### requires by keyword

```htmldjango
{% regroup items WRONG category as grouped %}
```

```snapshot
error[S117]: 'regroup' expected 'by' at position 2
 --> test.html:1:1
  |
1 | {% regroup items WRONG category as grouped %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```

### requires as keyword

```htmldjango
{% regroup items by category WRONG grouped %}
```

```snapshot
error[S117]: 'regroup' expected 'as' at position 4
 --> test.html:1:1
  |
1 | {% regroup items by category WRONG grouped %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: regroup
```
