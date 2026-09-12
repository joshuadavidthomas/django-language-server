# Feasible backend diagnostics

## an open backend keeps a concrete library inconclusive

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [
    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': [UNKNOWN, '/templates/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},
    UNKNOWN,
]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name='shared_tag')
def alpha(value):
    pass
```

`shared/page.html`:

```htmldjango
{% load shared %}{% shared_tag %}
```

```snapshot
✓ no diagnostics
```

## a wholly unknown templates branch keeps validation inconclusive

`settings.py`:

```py
INSTALLED_APPS = []
if FLAG:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]
else:
    TEMPLATES = UNKNOWN
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def shared_tag(value):
    pass
```

```htmldjango
{% load shared %}{% shared_tag %}
```

```snapshot
✓ no diagnostics
```

## each template uses its resolving backend's library

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [
    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/a'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},
    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/b'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},
]
```

### first backend template

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def alpha():
    pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def beta():
    pass
```

`a/alpha.html`:

```htmldjango
{% load shared %}{% alpha %}{% beta %}
```

```snapshot
error[S108]: Unknown tag 'beta'
 --> a/alpha.html:1:29
  |
1 | {% load shared %}{% alpha %}{% beta %}
  |                             ^^^^^^^^^^
```

### second backend template

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def alpha():
    pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def beta():
    pass
```

`b/beta.html`:

```htmldjango
{% load shared %}{% alpha %}{% beta %}
```

```snapshot
error[S108]: Unknown tag 'alpha'
 --> b/beta.html:1:18
  |
1 | {% load shared %}{% alpha %}{% beta %}
  |                  ^^^^^^^^^^^
```

## conflicting backend contracts remain inconclusive

`settings.py`:

```py
INSTALLED_APPS = []
if FLAG:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]
else:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name='shared_tag')
def alpha(value):
    pass
@register.filter(name='shared_filter')
def alpha_filter(value, arg):
    return value
@register.tag(name='panel')
def alpha_panel(parser, token):
    body = parser.parse(('endalpha',))
    return Node(body)
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name='shared_tag')
def beta():
    pass
@register.filter(name='shared_filter')
def beta_filter(value):
    return value
@register.tag(name='panel')
def beta_panel(parser, token):
    body = parser.parse(('endbeta',))
    return Node(body)
```

`shared/page.html`:

```htmldjango
{% load shared %}{% shared_tag %}{{ value|shared_filter }}{% panel %}
```

```snapshot
✓ no diagnostics
```

## disagreeing feasible backends suppress symbol diagnostics

`settings.py`:

```py
INSTALLED_APPS = []
if FLAG:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}}]
else:
    TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/shared'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}}]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def alpha():
    pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def beta():
    pass
```

`shared/page.html`:

```htmldjango
{% load shared %}{% alpha %}{% beta %}
```

```snapshot
✓ no diagnostics
```

## a shadowed template keeps its origin backend

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [
    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates/first'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'alpha_tags'}}},
    {'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'libraries': {'shared': 'beta_tags'}}},
]
```

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def alpha():
    pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def beta():
    pass
```

`first/test.html`:

```htmldjango
{% load shared %}{% alpha %}
```

```htmldjango
{% load shared %}{% beta %}
```

```snapshot
✓ no diagnostics
```
