# verbatim

## Valid

### treats contents as opaque

```htmldjango
{% verbatim %}
  {{ this_is_not_rendered }}
  {% if this_is_ignored %}{% endif %}
{% endverbatim %}
```

```snapshot
✓ no diagnostics
```

### supports named verbatim blocks

```htmldjango
{% verbatim myblock %}
  {{ still_literal }}
{% endverbatim myblock %}
```

```snapshot
✓ no diagnostics
```

## extracted verbatim stays opaque while custom bodies stay active

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'custom': 'custom_tags'}}}]
```

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.tag
def panel(parser, token):
    if token.contents:
        body = parser.parse(("endpanel",))
    else:
        parser.skip_past("endpanel")
    return Node(body)
```

```htmldjango
{% verbatim %}{% if and hidden %}{% endverbatim %}{% load custom %}{% panel %}{% if and active %}{% endpanel %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:79
  |
1 | {% verbatim %}{% if and hidden %}{% endverbatim %}{% load custom %}{% panel %}{% if and active %}{% endpanel %}
  |                                                                               ^^^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'if' tag
 --> test.html:1:79
  |
1 | {% verbatim %}{% if and hidden %}{% endverbatim %}{% load custom %}{% panel %}{% if and active %}{% endpanel %}
  |                                                                               ^^^^^^^^^^^^^^^^^^^
```

## validates before and after an opaque region

```htmldjango
{{ value|truncatewords }}
{% verbatim %}{% if and x %}{% endverbatim %}
{{ value|title:"bad" }}
```

```snapshot
error[S115]: Filter 'truncatewords' requires an argument
 --> test.html:1:10
  |
1 | {{ value|truncatewords }}
  |          ^^^^^^^^^^^^^
error[S116]: Filter 'title' does not accept an argument
 --> test.html:3:10
  |
3 | {{ value|title:"bad" }}
  |          ^^^^^^^^^^^
```

## a hidden load does not affect later tag availability

```htmldjango
{% verbatim %}{% load i18n %}{% endverbatim %}
{% trans "hello" %}
```

```snapshot
error[S109]: Tag 'trans' requires the 'i18n' tag library
 --> test.html:2:1
  |
2 | {% trans "hello" %}
  | ^^^^^^^^^^^^^^^^^^^
```
