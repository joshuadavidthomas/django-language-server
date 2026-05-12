# Structural diagnostics

## if block is not closed

```htmldjango
{% if user %}
  Hello
```

```snapshot
error[S100]: Unclosed tag: if
 --> test.html:1:1
  |
1 | {% if user %}
  | ^^^^^^^^^^^^^
```

## else outside if

```htmldjango
{% else %}
```

```snapshot
error[S102]: Orphaned tag 'else' - 'if' or 'ifchanged' block
 --> test.html:1:1
  |
1 | {% else %}
  | ^^^^^^^^^^
```

## endif outside if

```htmldjango
{% endif %}
```

```snapshot
error[S101]: Unbalanced structure: 'if' missing closing ''
 --> test.html:1:1
  |
1 | {% endif %}
  | ^^^^^^^^^^^
```

## closing block name mismatch

```htmldjango
{% block content %}
{% endblock sidebar %}
```

```snapshot
error[S103]: 'sidebar' does not match 'content'
 --> test.html:2:1
  |
2 | {% endblock sidebar %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```
