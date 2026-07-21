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

### requires load when cache is not configured as a builtin

```htmldjango
{% cache 500 before_load %}
  <p>Before load.</p>
{% endcache %}
```

```snapshot
error[S109]: Tag 'cache' requires the 'cache' tag library
 --> test.html:1:1
  |
1 | {% cache 500 before_load %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endcache'
 --> test.html:3:1
  |
3 | {% endcache %}
  | ^^^^^^^^^^^^^^
```

### reports unclosed cache block

```htmldjango
{% load cache %}
{% cache 500 broken %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed 'cache' tag
 --> test.html:2:1
  |
2 | {% cache 500 broken %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```
