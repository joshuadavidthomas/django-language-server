# tz

## Valid

### enables local time after load

```htmldjango
{% load tz %}

{% localtime on %}
  {{ value }}
{% endlocaltime %}
```

```snapshot
✓ no diagnostics
```

### disables local time after load

```htmldjango
{% load tz %}

{% localtime off %}
  {{ value }}
{% endlocaltime %}
```

```snapshot
✓ no diagnostics
```

### activates literal timezone

```htmldjango
{% load tz %}

{% timezone "Europe/Paris" %}
  {{ value }}
{% endtimezone %}
```

```snapshot
✓ no diagnostics
```

### activates variable timezone

```htmldjango
{% load tz %}

{% timezone user_tz %}
  {{ value }}
{% endtimezone %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### reports unclosed timezone block

```htmldjango
{% load tz %}
{% timezone "UTC" %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed 'timezone' tag
 --> test.html:2:1
  |
2 | {% timezone "UTC" %}
  | ^^^^^^^^^^^^^^^^^^^^
```
