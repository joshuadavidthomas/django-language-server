# Scoping diagnostics

## unknown tag

```htmldjango
{% completelymadetuptag %}
```

```snapshot
error[S108]: Unknown tag 'completelymadetuptag'
 --> test.html:1:1
  |
1 | {% completelymadetuptag %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## tag requires load when not configured as a builtin

```htmldjango
{% static "before.css" %}
```

```snapshot
error[S109]: Tag 'static' requires the 'static' tag library
 --> test.html:1:1
  |
1 | {% static "before.css" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

## tag is available from multiple unloaded libraries

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="ambiguous_tag")
def ambiguous_tag(parser, token): pass
@register.filter(name="ambiguous_filter")
def ambiguous_filter(value, arg=None): pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="ambiguous_tag")
def ambiguous_tag(parser, token): pass
@register.filter(name="ambiguous_filter")
def ambiguous_filter(value, arg=None): pass
```

```htmldjango
{% ambiguous_tag %}
```

```snapshot
error[S110]: Tag 'ambiguous_tag' is available from multiple libraries: 'alpha', 'beta'
 --> test.html:1:1
  |
1 | {% ambiguous_tag %}
  | ^^^^^^^^^^^^^^^^^^^
```

## unknown filter

```htmldjango
{{ value|completelymadetupfilter }}
```

```snapshot
error[S111]: Unknown filter 'completelymadetupfilter'
 --> test.html:1:10
  |
1 | {{ value|completelymadetupfilter }}
  |          ^^^^^^^^^^^^^^^^^^^^^^^
```

## filter requires load when not configured as a builtin

`settings.py`:

```py
INSTALLED_APPS = ['django.contrib.humanize']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

```htmldjango
{{ value|intcomma }}
```

```snapshot
error[S112]: Filter 'intcomma' requires the 'humanize' tag library
 --> test.html:1:10
  |
1 | {{ value|intcomma }}
  |          ^^^^^^^^
```

## filter is available from multiple unloaded libraries

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="ambiguous_tag")
def ambiguous_tag(parser, token): pass
@register.filter(name="ambiguous_filter")
def ambiguous_filter(value, arg=None): pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="ambiguous_tag")
def ambiguous_tag(parser, token): pass
@register.filter(name="ambiguous_filter")
def ambiguous_filter(value, arg=None): pass
```

```htmldjango
{{ value|ambiguous_filter }}
```

```snapshot
error[S113]: Filter 'ambiguous_filter' is available from multiple libraries: 'alpha', 'beta'
 --> test.html:1:10
  |
1 | {{ value|ambiguous_filter }}
  |          ^^^^^^^^^^^^^^^^
```

## tag library is unknown

```htmldjango
{% load nonexistent_library %}
```

```snapshot
error[S120]: Unknown template tag library 'nonexistent_library'
 --> test.html:1:9
  |
1 | {% load nonexistent_library %}
  |         ^^^^^^^^^^^^^^^^^^^
```

## a source-less alias suppresses unavailable-app guidance

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'shared': 'missing.shared'}}}]
```

`available_in_app/templatetags/shared.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def shared_tag(): pass
@register.filter
def shared_filter(value): return value
```

```htmldjango
{% load shared %}{% shared_tag %}{{ value|shared_filter }}
```

```snapshot
✓ no diagnostics
```

## an unloaded custom if does not replace the builtin

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'collisions': 'collision_tags'}}}]
```

`collision_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="if")
def custom_if(parser, token):
    body = parser.parse(("endcustom",))
    return Node(body)
```

```htmldjango
{% if condition %}yes{% endif %}
```

```snapshot
✓ no diagnostics
```

## an unloaded custom if leaves the builtin block contract

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'collisions': 'collision_tags'}}}]
```

`collision_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="if")
def custom_if(parser, token):
    body = parser.parse(("endcustom",))
    return Node(body)
```

```htmldjango
{% if condition %}
```

```snapshot
error[S100]: Unclosed 'if' tag
 --> test.html:1:1
  |
1 | {% if condition %}
  | ^^^^^^^^^^^^^^^^^^
```

## an unloaded custom closer remains unknown

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'panels': 'panel_tags'}}}]
```

`panel_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="panel")
def panel(parser, token):
    body = parser.parse(("endpanel",))
    return Node(body)
```

```htmldjango
{% endpanel %}
```

```snapshot
error[S108]: Unknown tag 'endpanel'
 --> test.html:1:1
  |
1 | {% endpanel %}
  | ^^^^^^^^^^^^^^
```

## a loaded library contract wins over an unloaded collision

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags'}}}]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="shared_tag")
def alpha(value):
    pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="shared_tag")
def beta():
    pass
```

```htmldjango
{% load alpha %}{% shared_tag %}
```

```snapshot
error[S117]: 'shared_tag' did not receive value(s) for the argument(s): 'value'
 --> test.html:1:17
  |
1 | {% load alpha %}{% shared_tag %}
  |                 ^^^^^^^^^^^^^^^^
```

## a later load does not change an open block contract

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'custom': 'custom_tags'}}}]
```

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="if")
def custom_if(value):
    pass
```

```htmldjango
{% if value %}{% load custom %}{% endif %}
```

```snapshot
✓ no diagnostics
```

## an open block keeps its intermediate contract after a later load

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'custom': 'custom_tags'}}}]
```

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="if")
def custom_if(value): pass
```

```htmldjango
{% if first %}{% load custom %}{% elif second %}second{% endif %}
```

```snapshot
✓ no diagnostics
```

## a nested load affects later occurrences

```htmldjango
{% if value %}{% load i18n %}{% trans 'hello' %}{% endif %}
```

```snapshot
✓ no diagnostics
```
