# widthratio

## Valid

### computes a width ratio

```htmldjango
{% widthratio this_value max_value max_width %}
```

```snapshot
✓ no diagnostics
```

### assigns computed ratio to a variable

```htmldjango
{% widthratio this_value max_value max_width as ratio %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires as keyword in assignment form

```htmldjango
{% widthratio this_value max_value max_width WRONG ratio %}
```

```snapshot
error[S117]: Invalid syntax in widthratio tag. Expecting 'as' keyword
 --> test.html:1:1
  |
1 | {% widthratio this_value max_value max_width WRONG ratio %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects missing arguments

```htmldjango
{% widthratio %}
```

```snapshot
error[S117]: Tag 'widthratio' takes 3 or 5 arguments
 --> test.html:1:1
  |
1 | {% widthratio %}
  | ^^^^^^^^^^^^^^^^
```

### rejects two arguments

```htmldjango
{% widthratio this_value max_value %}
```

```snapshot
error[S117]: Tag 'widthratio' takes 3 or 5 arguments
 --> test.html:1:1
  |
1 | {% widthratio this_value max_value %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects assignment without a variable

```htmldjango
{% widthratio this_value max_value max_width as %}
```

```snapshot
error[S117]: Tag 'widthratio' takes 3 or 5 arguments
 --> test.html:1:1
  |
1 | {% widthratio this_value max_value max_width as %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects an extra assignment argument

```htmldjango
{% widthratio this_value max_value max_width as ratio extra %}
```

```snapshot
error[S117]: Tag 'widthratio' takes 3 or 5 arguments
 --> test.html:1:1
  |
1 | {% widthratio this_value max_value max_width as ratio extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
