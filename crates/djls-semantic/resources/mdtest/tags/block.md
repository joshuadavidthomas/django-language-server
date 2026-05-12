# block

## Valid

### defines a named block

```htmldjango
{% block content %}
  <p>Hello</p>
{% endblock %}
```

```snapshot
✓ no diagnostics
```

### allows repeated closing block name

```htmldjango
{% block sidebar %}
  <nav>Links</nav>
{% endblock sidebar %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects mismatched closing name

```htmldjango
{% block header %}
  <h1>Title</h1>
{% endblock footer %}
```

```snapshot
error[S103]: 'footer' does not match 'header'
 --> test.html:3:1
  |
3 | {% endblock footer %}
  | ^^^^^^^^^^^^^^^^^^^^^
```

### reports unclosed block

```htmldjango
{% block dangling %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed tag: block
 --> test.html:1:1
  |
1 | {% block dangling %}
  | ^^^^^^^^^^^^^^^^^^^^
```
