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
error[S102]: '{% else %}' must be inside an open '{% if %}' or '{% ifchanged %}' block
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
error[S101]: Closing tag '{% endif %}' has no matching '{% if %}' opener
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
error[S103]: Closing block name 'sidebar' does not match opening block name 'content'
 --> test.html:2:1
  |
2 | {% endblock sidebar %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```
