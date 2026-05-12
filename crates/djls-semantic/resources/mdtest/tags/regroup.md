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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: 'regroup' tag takes five arguments
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
error[S117]: second argument to 'regroup' tag must be 'by'
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
error[S117]: next-to-last argument to 'regroup' tag must be 'as'
 --> test.html:1:1
  |
1 | {% regroup items by category WRONG grouped %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
