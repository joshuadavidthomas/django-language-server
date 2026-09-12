# Unknown load contracts

## Partly known library map

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['contract_tags'], 'libraries': {**UNKNOWN, 'exact': 'exact_tags'}}}]
```

### an unknown full load suppresses an argument contract

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load unknown_library %}
{% contract_tag %}
```

```snapshot
✓ no diagnostics
```

### a later exact full load restores exact contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load unknown_library exact %}
{% exact_symbol %}
{% contract_tag %}
{{ value|exact_filter }}
```

```snapshot
error[S117]: 'exact_symbol' did not receive value(s) for the argument(s): 'value'
 --> test.html:2:1
  |
2 | {% exact_symbol %}
  | ^^^^^^^^^^^^^^^^^^
error[S115]: Filter 'exact_filter' requires an argument
 --> test.html:4:10
  |
4 | {{ value|exact_filter }}
  |          ^^^^^^^^^^^^
```

### a later exact load restores selectively uncertain contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load exact_symbol exact_filter from unknown_library %}
{% load exact %}
{% exact_symbol %}
{{ value|exact_filter }}
```

```snapshot
error[S117]: 'exact_symbol' did not receive value(s) for the argument(s): 'value'
 --> test.html:3:1
  |
3 | {% exact_symbol %}
  | ^^^^^^^^^^^^^^^^^^
error[S115]: Filter 'exact_filter' requires an argument
 --> test.html:4:10
  |
4 | {{ value|exact_filter }}
  |          ^^^^^^^^^^^^
```

### a later selective unknown load suppresses exact contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load exact %}
{% load exact_symbol exact_filter from unknown_library %}
{% exact_symbol %}
{{ value|exact_filter }}
```

```snapshot
✓ no diagnostics
```

### a later unknown full load suppresses exact contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load exact unknown_library %}
{% exact_symbol %}
{{ value|exact_filter }}
```

```snapshot
✓ no diagnostics
```

### an unknown full load suppresses later orphan contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load unknown_library %}
{% else %}
{% endif %}
```

```snapshot
✓ no diagnostics
```

### an unknown full load suppresses a later opener contract

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load unknown_library %}
{% if condition %}
```

```snapshot
✓ no diagnostics
```

### a selective unknown import affects only named symbols

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load contract_tag if from unknown_library %}
{% contract_tag %}
{% other_contract %}
{% if condition %}
{% for item in items %}
```

```snapshot
error[S117]: 'other_contract' did not receive value(s) for the argument(s): 'value'
 --> test.html:3:1
  |
3 | {% other_contract %}
  | ^^^^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'for' tag
 --> test.html:5:1
  |
5 | {% for item in items %}
  | ^^^^^^^^^^^^^^^^^^^^^^^
```

### an exact load retains unrelated contracts

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load exact %}
{% contract_tag %}
{% if condition %}
```

```snapshot
error[S117]: 'contract_tag' did not receive value(s) for the argument(s): 'value'
 --> test.html:2:1
  |
2 | {% contract_tag %}
  | ^^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'if' tag
 --> test.html:3:1
  |
3 | {% if condition %}
  | ^^^^^^^^^^^^^^^^^^
```

### a closed missing library retains known contracts

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['one_arg_tags'], 'libraries': {}}}]
```

`one_arg_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def one_arg_tag(value): pass
```

```htmldjango
{% load missing_library %}
{% one_arg_tag %}
{% if condition %}
```

```snapshot
error[S120]: Unknown template tag library 'missing_library'
 --> test.html:1:9
  |
1 | {% load missing_library %}
  |         ^^^^^^^^^^^^^^^
error[S117]: 'one_arg_tag' did not receive value(s) for the argument(s): 'value'
 --> test.html:2:1
  |
2 | {% one_arg_tag %}
  | ^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'if' tag
 --> test.html:3:1
  |
3 | {% if condition %}
  | ^^^^^^^^^^^^^^^^^^
```

### a captured intermediate and closer survive an unknown load

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% if first %}
{% load unknown_library %}
{% elif second %}second
{% endif %}
```

```snapshot
✓ no diagnostics
```

### a captured closer keeps its argument contract

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% block expected %}
{% load unknown_library %}
{% endblock actual %}
```

```snapshot
error[S103]: Closing block 'actual' does not match opening block 'expected'
 --> test.html:3:1
  |
3 | {% endblock actual %}
  | ^^^^^^^^^^^^^^^^^^^^^
```

### selective unknown imports suppress definition roles

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load if extends from unknown_library %}
{% csrf_token %}
{% extends 'base.html' %}
{% if and %}
```

```snapshot
✓ no diagnostics
```

### a shadowed loader does not create a later load event

`contract_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def contract_tag(value): pass
@register.simple_tag
def other_contract(value): pass
```

`exact_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def exact_symbol(value): pass
@register.filter
def exact_filter(value, argument): pass
```

```htmldjango
{% load load from unknown_library %}
{% load exact %}
{% exact_symbol %}
```

```snapshot
error[S109]: Tag 'exact_symbol' requires the 'exact' tag library
 --> test.html:3:1
  |
3 | {% exact_symbol %}
  | ^^^^^^^^^^^^^^^^^^
```
