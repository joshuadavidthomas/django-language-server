# comment

## Valid

### treats contents as opaque

```htmldjango
{% comment %}
  This is ignored by the template engine.
  {% if broken %}{% endif broken does not matter here %}
{% endcomment %}
```

```snapshot
✓ no diagnostics
```

### allows an optional note

```htmldjango
{% comment "Optional note" %}
  Also ignored.
{% endcomment %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### reports unclosed comment

```htmldjango
{% comment %}
  Never closed.
```

```snapshot
error[S100]: Unclosed 'comment' tag
 --> test.html:1:1
  |
1 | {% comment %}
  | ^^^^^^^^^^^^^
```

## a hidden load does not affect later tag availability

```htmldjango
{% comment %}{% load i18n %}{% endcomment %}
{% trans "hello" %}
```

```snapshot
error[S109]: Tag 'trans' requires the 'i18n' tag library
 --> test.html:2:1
  |
2 | {% trans "hello" %}
  | ^^^^^^^^^^^^^^^^^^^
```
