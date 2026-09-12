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

## an imported register keeps symbol misses inconclusive

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'open': 'open_tags'}}}]
```

`open_tags.py`:

```py
from shared import register
@register.simple_tag
def known_tag(): pass
@register.filter
def known_filter(value): return value
```

```htmldjango
{% load missing_library %}{% load open %}{% known_tag %}{% absent_tag %}{{ value|known_filter }}{{ value|absent_filter }}
```

```snapshot
error[S120]: Unknown template tag library 'missing_library'
 --> test.html:1:9
  |
1 | {% load missing_library %}{% load open %}{% known_tag %}{% absent_tag %}{{ value|known_filter }}{{ value|absent_filter }}
  |         ^^^^^^^^^^^^^^^
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 1 (the template register is rebound, deleted, or augmented), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:35
  |
1 | {% load missing_library %}{% load open %}{% known_tag %}{% absent_tag %}{{ value|known_filter }}{{ value|absent_filter }}
  |                                   ^^^^
```

## a known tag reports its unloaded library

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'known': 'known_tags', 'open': 'open_tags'}}}]
```

`known_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def known_tag(): pass
```

`open_tags.py`:

```py
from django import template
register = template.Library()
def other_tag(context): pass
register.simple_tag(takes_context=True)(globals()['other_tag'])
```

```htmldjango
{% known_tag %}
```

```snapshot
error[S109]: Tag 'known_tag' requires the 'known' tag library
 --> test.html:1:1
  |
1 | {% known_tag %}
  | ^^^^^^^^^^^^^^^
```

## loading an unreadable library makes another tag inconclusive

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'known': 'known_tags', 'open': 'open_tags'}}}]
```

`known_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def known_tag(): pass
```

`open_tags.py`:

```py
from django import template
register = template.Library()
def other_tag(context): pass
register.simple_tag(takes_context=True)(globals()['other_tag'])
```

```htmldjango
{% load open %}{% known_tag %}
```

```snapshot
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 4 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:9
  |
1 | {% load open %}{% known_tag %}
  |         ^^^^
```

## loading the known library makes its tag valid

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'known': 'known_tags', 'open': 'open_tags'}}}]
```

`known_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def known_tag(): pass
```

`open_tags.py`:

```py
from django import template
register = template.Library()
def other_tag(context): pass
register.simple_tag(takes_context=True)(globals()['other_tag'])
```

```htmldjango
{% load known %}{% known_tag %}
```

```snapshot
✓ no diagnostics
```

## a closed registration source keeps symbol misses definite

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'closed': 'closed_tags'}}}]
```

`closed_tags.py`:

```py
from django import template
register = template.Library()
```

```htmldjango
{% load closed %}{% absent_tag %}{{ value|absent_filter }}
```

```snapshot
error[S108]: Unknown tag 'absent_tag'
 --> test.html:1:18
  |
1 | {% load closed %}{% absent_tag %}{{ value|absent_filter }}
  |                  ^^^^^^^^^^^^^^^^
error[S111]: Unknown filter 'absent_filter'
 --> test.html:1:43
  |
1 | {% load closed %}{% absent_tag %}{{ value|absent_filter }}
  |                                           ^^^^^^^^^^^^^
```
