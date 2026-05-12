# ifchanged

## Valid

### tracks changes in rendered body

```htmldjango
{% for item in items %}
  {% ifchanged %}
    {{ item.category }}
  {% endifchanged %}
{% endfor %}
```

```snapshot
✓ no diagnostics
```

### tracks changes in explicit values

```htmldjango
{% for item in items %}
  {% ifchanged item.category %}
    <h2>{{ item.category }}</h2>
  {% endifchanged %}
{% endfor %}
```

```snapshot
✓ no diagnostics
```

### supports else branch

```htmldjango
{% for item in items %}
  {% ifchanged item.category %}
    <h2>{{ item.category }}</h2>
  {% else %}
    <hr>
  {% endifchanged %}
{% endfor %}
```

```snapshot
✓ no diagnostics
```
