# Extends diagnostics

## two-file scenario validates the primary template

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

## whitespace may precede extends

```htmldjango


  {% extends "base.html" %}
```

```snapshot
✓ no diagnostics
```

## a template comment may precede extends

```htmldjango
{# this is a comment #}{% extends "base.html" %}
```

```snapshot
✓ no diagnostics
```

## a template without extends has no extends diagnostics

```htmldjango
{% if user %}hello{% endif %}
```

```snapshot
✓ no diagnostics
```

## a variable before extends is not first

```htmldjango
{{ variable }}{% extends "base.html" %}
```

```snapshot
error[S122]: The 'extends' tag must be the first tag in the template
 --> test.html:1:15
  |
1 | {{ variable }}{% extends "base.html" %}
  |               ^^^^^^^^^^^^^^^^^^^^^^^^^
```

## content before two extends tags reports both errors

```htmldjango
{% load i18n %}{% extends "a.html" %}{% extends "b.html" %}
```

```snapshot
error[S122]: The 'extends' tag must be the first tag in the template
 --> test.html:1:16
  |
1 | {% load i18n %}{% extends "a.html" %}{% extends "b.html" %}
  |                ^^^^^^^^^^^^^^^^^^^^^^
error[S123]: The 'extends' tag can only appear once in a template
 --> test.html:1:38
  |
1 | {% load i18n %}{% extends "a.html" %}{% extends "b.html" %}
  |                                      ^^^^^^^^^^^^^^^^^^^^^^
```

## extends inside verbatim does not affect ordering

```htmldjango
<p>body</p>{% verbatim %}{% extends "base.html" %}{% endverbatim %}
```

```snapshot
✓ no diagnostics
```

## extends inside comment does not count

```htmldjango
{% comment %}{% extends "a.html" %}{% extends "b.html" %}{% endcomment %}
```

```snapshot
✓ no diagnostics
```

## an opaque extends does not follow an active one

```htmldjango
{% extends "base.html" %}{% verbatim %}{% extends "ignored.html" %}{% endverbatim %}
```

```snapshot
✓ no diagnostics
```
