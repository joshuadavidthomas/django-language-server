# cache

## Valid

### caches content after load

```htmldjango
{% load cache %}

{% cache 500 sidebar %}
  <nav>Expensive sidebar</nav>
{% endcache %}
```

```snapshot
✓ no diagnostics
```

### supports vary-on arguments

```htmldjango
{% load cache %}

{% cache 500 sidebar request.user.username %}
  <nav>User-specific sidebar</nav>
{% endcache %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires load before use

```htmldjango
{% cache 500 before_load %}
  <p>Before load.</p>
{% endcache %}
```

```snapshot
error[S109]: Tag 'cache' requires {% load cache %}
 --> test.html:1:1
  |
1 | {% cache 500 before_load %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### reports unclosed cache block

```htmldjango
{% load cache %}
{% cache 500 broken %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed tag: cache
 --> test.html:2:1
  |
2 | {% cache 500 broken %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```
