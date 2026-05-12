# firstof

## Valid

### chooses the first truthy variable

```htmldjango
{% firstof var1 var2 var3 %}
```

```snapshot
✓ no diagnostics
```

### supports a literal fallback

```htmldjango
{% firstof var1 var2 "fallback" %}
```

```snapshot
✓ no diagnostics
```

### assigns the result to a variable

```htmldjango
{% firstof var1 var2 as result %}
```

```snapshot
✓ no diagnostics
```
