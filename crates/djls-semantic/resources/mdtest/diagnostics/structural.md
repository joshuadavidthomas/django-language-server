# Structural diagnostics

## if block is not closed

```htmldjango
{% if user %}
  Hello
```

```snapshot
error[S100]: Unclosed 'if' tag
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
error[S102]: 'else' must be inside an open 'if' or 'ifchanged' block
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
error[S101]: 'endif' has no matching 'if' block
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
error[S103]: Closing block 'sidebar' does not match opening block 'content'
 --> test.html:2:1
  |
2 | {% endblock sidebar %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```
