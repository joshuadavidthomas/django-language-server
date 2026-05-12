# verbatim

## Valid

### treats contents as opaque

```htmldjango
{% verbatim %}
  {{ this_is_not_rendered }}
  {% if this_is_ignored %}{% endif %}
{% endverbatim %}
```

```snapshot
✓ no diagnostics
```

### supports named verbatim blocks

```htmldjango
{% verbatim myblock %}
  {{ still_literal }}
{% endverbatim myblock %}
```

```snapshot
✓ no diagnostics
```
