# Extends diagnostics

## two-file scenario validates the primary template

`child.html`:

```htmldjango
{% extends "base.html" %}
{% block content %}Hello{% endblock %}
```

`base.html`:

```htmldjango
{% block content %}{% endblock %}
```

```snapshot
✓ no diagnostics
```

## extends is not first tag

```htmldjango
{% load i18n %}
{% extends "base.html" %}
```

```snapshot
error[S122]: The 'extends' tag must be the first tag in the template
 --> test.html:2:1
  |
2 | {% extends "base.html" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

## multiple extends tags

```htmldjango
{% extends "base.html" %}
{% extends "other.html" %}
```

```snapshot
error[S123]: The 'extends' tag can only appear once in a template
 --> test.html:2:1
  |
2 | {% extends "other.html" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
```
