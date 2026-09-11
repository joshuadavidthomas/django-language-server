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

## accepts one loop variable

```htmldjango
{% for x in items %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

## accepts unpacked loop variables

```htmldjango
{% for x, y in items %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

## accepts reversed after the iterable

```htmldjango
{% for x in items reversed %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

## accepts unpacking with reversed

```htmldjango
{% for x, y in items reversed %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

## rejects from in place of in

```htmldjango
{% for x from items %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should use the format 'for x in y': for x from items
 --> test.html:1:1
  |
1 | {% for x from items %}{% endfor %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```

## rejects from before reversed

```htmldjango
{% for x from items reversed %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should use the format 'for x in y': for x from items reversed
 --> test.html:1:1
  |
1 | {% for x from items reversed %}{% endfor %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## rejects reversed as the iterable

```htmldjango
{% for x in reversed %}{% endfor %}
```

```snapshot
error[S117]: Tag 'for' does not accept 'reversed' at position 3
 --> test.html:1:1
  |
1 | {% for x in reversed %}{% endfor %}
  | ^^^^^^^^^^^^^^^^^^^^^^^
```

## rejects a missing in keyword

```htmldjango
{% for x y in reversed %}{% endfor %}
```

```snapshot
error[S117]: Tag 'for' does not accept 'reversed' at position 4
 --> test.html:1:1
  |
1 | {% for x y in reversed %}{% endfor %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

## rejects an omitted iterable

```htmldjango
{% for x in %}{% endfor %}
```

```snapshot
error[S117]: 'for' statements should have at least four words: for x in
 --> test.html:1:1
  |
1 | {% for x in %}{% endfor %}
  | ^^^^^^^^^^^^^^
```
