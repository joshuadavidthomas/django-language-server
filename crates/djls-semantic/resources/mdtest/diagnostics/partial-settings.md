# Partial settings diagnostics

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'alpha': 'alpha_tags', 'beta': 'beta_tags', 'custom': 'custom_tags'}}, UNKNOWN: 'maybe'}]
```

## configured rules stay inconclusive

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def configured(value):
    pass
```

```htmldjango
{% load custom %}{% configured %}
```

```snapshot
✓ no diagnostics
```

## unknown tags are suppressed

```htmldjango
{% definitely_unknown %}
```

```snapshot
✓ no diagnostics
```

## an unknown load can provide known symbols

`alpha_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="shared")
def shared_tag(parser, token): pass
@register.filter(name="shared")
def shared_filter(value): pass
```

`beta_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="shared")
def shared_tag(parser, token): pass
@register.filter(name="shared")
def shared_filter(value): pass
```

```htmldjango
{% load unknown_library %}
{% shared %}
{{ value|shared }}
```

```snapshot
✓ no diagnostics
```

## unknown libraries are suppressed

```htmldjango
{% load missing_library %}
```

```snapshot
✓ no diagnostics
```

## unknown filters are suppressed

```htmldjango
{{ value|definitely_unknown }}
```

```snapshot
✓ no diagnostics
```

## an unknown load can shadow filter arity

```htmldjango
{% load project_filters %}
{{ value|truncatewords }}
```

```snapshot
✓ no diagnostics
```
