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
error[S102]: 'empty' must be inside an open 'for' block
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
error[S100]: Unclosed 'for' tag
 --> test.html:1:1
  |
1 | {% for item in items %}
  | ^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects missing loop variables

```htmldjango
{% for %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should have at least four words: for
 --> test.html:1:1
  |
1 | {% for %}{% endfor %}
  | ^^^^^^^^^
```

### rejects missing in keyword and iterable

```htmldjango
{% for item %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should have at least four words: for item
 --> test.html:1:1
  |
1 | {% for item %}{% endfor %}
  | ^^^^^^^^^^^^^^
```

### rejects missing iterable

```htmldjango
{% for item in %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should have at least four words: for item in
 --> test.html:1:1
  |
1 | {% for item in %}{% endfor %}
  | ^^^^^^^^^^^^^^^^^
```
