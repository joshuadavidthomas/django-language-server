# Unreadable Template Library loads

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'open': 'open_tags'}}}]
```

## loaded library has a registration DJLS could not read

`open_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def known_tag(): pass
register.simple_tag(takes_context=True)(globals()['other_tag'])
```

```htmldjango
{% load open %}
{% known_tag %}
```

```snapshot
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 5 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:9
  |
1 | {% load open %}
  |         ^^^^
```

## unrecognized tag from an unreadable library is not reported

`open_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def known_tag(): pass
register.simple_tag(takes_context=True)(globals()['other_tag'])
```

```htmldjango
{% load open %}
{% other_tag %}
```

```snapshot
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 5 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:9
  |
1 | {% load open %}
  |         ^^^^
```
