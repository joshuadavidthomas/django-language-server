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

### a repeated name inside a comment is not compiled

```htmldjango
{% block content %}{% endblock %}
{% comment %}{% block content %}{% endblock %}{% endcomment %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects a repeated block name

Django's `do_block` records each name on the parser and rejects one it has already seen, even in another branch or nested inside the first block.

```htmldjango
{% extends "base.html" %}
{% block content %}{% block content %}{% endblock %}{% endblock %}
{% if user %}{% block sidebar %}{% endblock %}{% else %}{% block sidebar %}{% endblock %}{% endif %}
```

```snapshot
error[S125]: 'block' tag with name 'content' appears more than once
 --> test.html:2:29
  |
2 | {% block content %}{% block content %}{% endblock %}{% endblock %}
  |                             ^^^^^^^
error[S125]: 'block' tag with name 'sidebar' appears more than once
 --> test.html:3:66
  |
3 | {% if user %}{% block sidebar %}{% endblock %}{% else %}{% block sidebar %}{% endblock %}{% endif %}
  |                                                                  ^^^^^^^
```

### rejects mismatched closing name

```htmldjango
{% block header %}
  <h1>Title</h1>
{% endblock footer %}
```

```snapshot
error[S103]: Closing block 'footer' does not match opening block 'header'
 --> test.html:3:1
  |
3 | {% endblock footer %}
  | ^^^^^^^^^^^^^^^^^^^^^
```

### rejects extra arguments after the closing name

Django's `do_block` accepts only `endblock` or `endblock <name>` as the whole closing contents.

```htmldjango
{% block header %}{% endblock header extra %}
{% block footer %}{% endblock sidebar extra %}
```

```snapshot
error[S103]: Closing block 'header extra' does not match opening block 'header'
 --> test.html:1:19
  |
1 | {% block header %}{% endblock header extra %}
  |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^
error[S103]: Closing block 'sidebar extra' does not match opening block 'footer'
 --> test.html:2:19
  |
2 | {% block footer %}{% endblock sidebar extra %}
  |                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### reports unclosed block

```htmldjango
{% block dangling %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed 'block' tag
 --> test.html:1:1
  |
1 | {% block dangling %}
  | ^^^^^^^^^^^^^^^^^^^^
```
