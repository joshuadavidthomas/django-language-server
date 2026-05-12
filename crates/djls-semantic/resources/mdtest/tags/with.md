# with

## Valid

### binds one keyword value

```htmldjango
{% with name="World" %}
  <p>Hello, {{ name }}!</p>
{% endwith %}
```

```snapshot
✓ no diagnostics
```

### binds multiple keyword values

```htmldjango
{% with first="Hello" last="World" %}
  <p>{{ first }} {{ last }}</p>
{% endwith %}
```

```snapshot
✓ no diagnostics
```

### supports as assignment syntax

```htmldjango
{% with items.count as total %}
  <p>{{ total }} items</p>
{% endwith %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### reports unclosed with block

```htmldjango
{% with x=1 %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed 'with' tag
 --> test.html:1:1
  |
1 | {% with x=1 %}
  | ^^^^^^^^^^^^^^
```
