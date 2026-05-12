# for

## Valid

### iterates over a sequence

```htmldjango
{% for item in items %}
  {{ item }}
{% endfor %}
```

```snapshot
✓ no diagnostics
```

### supports empty fallback

```htmldjango
{% for item in items %}
  {{ item }}
{% empty %}
  <p>No items.</p>
{% endfor %}
```

```snapshot
✓ no diagnostics
```

### supports tuple unpacking

```htmldjango
{% for key, value in items.items %}
  {{ key }}: {{ value }}
{% endfor %}
```

```snapshot
✓ no diagnostics
```

### supports reversed iteration

```htmldjango
{% for item in items reversed %}
  {{ item }}
{% endfor %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects empty outside for

```htmldjango
{% empty %}
```

```snapshot
error[S102]: Orphaned tag 'empty' - 'for' block
 --> test.html:1:1
  |
1 | {% empty %}
  | ^^^^^^^^^^^
```

### reports unclosed loop

```htmldjango
{% for item in items %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed tag: for
 --> test.html:1:1
  |
1 | {% for item in items %}
  | ^^^^^^^^^^^^^^^^^^^^^^^
```

## Known gaps

### currently accepts missing loop variables

```htmldjango
{% for %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

### currently accepts missing in keyword and iterable

```htmldjango
{% for item %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

### currently accepts missing iterable

```htmldjango
{% for item in %}{% endfor %}
```

```snapshot
✓ no diagnostics
```
