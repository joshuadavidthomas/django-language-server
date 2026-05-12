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
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 0 were given
 --> test.html:1:1
  |
1 | {% regroup %}
  | ^^^^^^^^^^^^^
```

### rejects missing by keyword and target

```htmldjango
{% regroup items %}
```

```snapshot
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 1 was given
 --> test.html:1:1
  |
1 | {% regroup items %}
  | ^^^^^^^^^^^^^^^^^^^
```

### rejects missing regroup attribute and target

```htmldjango
{% regroup items by %}
```

```snapshot
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 2 were given
 --> test.html:1:1
  |
1 | {% regroup items by %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```

### rejects missing as keyword and target variable

```htmldjango
{% regroup items by category %}
```

```snapshot
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 3 were given
 --> test.html:1:1
  |
1 | {% regroup items by category %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects missing target variable

```htmldjango
{% regroup items by category as %}
```

```snapshot
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 4 were given
 --> test.html:1:1
  |
1 | {% regroup items by category as %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects too many arguments

```htmldjango
{% regroup items by category as grouped extra %}
```

```snapshot
error[S117]: Tag 'regroup' takes exactly 5 arguments, but 6 were given
 --> test.html:1:1
  |
1 | {% regroup items by category as grouped extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### requires by keyword

```htmldjango
{% regroup items WRONG category as grouped %}
```

```snapshot
error[S117]: Tag 'regroup' expects 'by' at position 2
 --> test.html:1:1
  |
1 | {% regroup items WRONG category as grouped %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### requires as keyword

```htmldjango
{% regroup items by category WRONG grouped %}
```

```snapshot
error[S117]: Tag 'regroup' expects 'as' at position 4
 --> test.html:1:1
  |
1 | {% regroup items by category WRONG grouped %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
