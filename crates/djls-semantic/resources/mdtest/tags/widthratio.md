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
error[S117]: 'widthratio' expected 'as' at position 4
 --> test.html:1:1
  |
1 | {% widthratio this_value max_value max_width WRONG ratio %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: widthratio
```
