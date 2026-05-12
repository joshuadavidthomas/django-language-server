# cycle

## Valid

### alternates between literal values

```htmldjango
{% cycle "row1" "row2" %}
```

```snapshot
✓ no diagnostics
```

### accepts more than two values

```htmldjango
{% cycle "a" "b" "c" %}
```

```snapshot
✓ no diagnostics
```

### alternates between variables

```htmldjango
{% cycle var1 var2 var3 %}
```

```snapshot
✓ no diagnostics
```

### assigns the cycle to a variable

```htmldjango
{% cycle "row1" "row2" as rowcolors %}
```

```snapshot
✓ no diagnostics
```

### supports silent assignment

```htmldjango
{% cycle "row1" "row2" "row3" as rowcolors silent %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires at least one value

```htmldjango
{% cycle %}
```

```snapshot
error[S117]: 'cycle' requires at least 1 argument
 --> test.html:1:1
  |
1 | {% cycle %}
  | ^^^^^^^^^^^
  |
  = note: in tag: cycle
```
