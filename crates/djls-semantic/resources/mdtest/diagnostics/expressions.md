# Expression diagnostics

## if tag

### starts with infix operator

```htmldjango
{% if and x %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}{% endif %}
  | ^^^^^^^^^^^^^^
```

### ends after infix operator

```htmldjango
{% if x or %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if x or %}{% endif %}
  | ^^^^^^^^^^^^^
```

### contains unused token

```htmldjango
{% if x y %}{% endif %}
```

```snapshot
error[S114]: Unused 'y' at end of if expression.
 --> test.html:1:1
  |
1 | {% if x y %}{% endif %}
  | ^^^^^^^^^^^^
```

### has no condition

```htmldjango
{% if %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if %}{% endif %}
  | ^^^^^^^^
```

## Known gaps

Django compiles each of these tag arguments with `FilterExpression` and rejects the template. DJLS does not check them yet.

### if operand is not a valid filter expression

```htmldjango
{% if user| %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### elif operand starts with an underscore

```htmldjango
{% if x %}{% elif _secret %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### firstof value is not a valid filter expression

```htmldjango
{% firstof a| b %}
```

```snapshot
✓ no diagnostics
```

### cycle value starts with an underscore

```htmldjango
{% cycle 'a' _b %}
```

```snapshot
✓ no diagnostics
```

### for sequence is not a valid filter expression

```htmldjango
{% for x in items| %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

### ifchanged value is not a valid filter expression

```htmldjango
{% ifchanged date| %}{% endifchanged %}
```

```snapshot
✓ no diagnostics
```

### regroup grouper starts with an underscore

```htmldjango
{% regroup people by _gender as groups %}
```

```snapshot
✓ no diagnostics
```

### url keyword argument has no value

```htmldjango
{% url 'home' page= %}
```

```snapshot
✓ no diagnostics
```

### widthratio value is not a valid filter expression

```htmldjango
{% widthratio this max 100| %}
```

```snapshot
✓ no diagnostics
```

### with value starts with an underscore

```htmldjango
{% with total=_count %}{% endwith %}
```

```snapshot
✓ no diagnostics
```

### filter tag has a dangling argument separator

```htmldjango
{% filter lower: %}{% endfilter %}
```

```snapshot
✓ no diagnostics
```

### filter tag uses escape

```htmldjango
{% filter escape %}{% endfilter %}
```

```snapshot
✓ no diagnostics
```

### lorem count starts with an underscore

```htmldjango
{% lorem _n w %}
```

```snapshot
✓ no diagnostics
```

### extends parent starts with an underscore

```htmldjango
{% extends _parent %}
```

```snapshot
✓ no diagnostics
```

### include assignment is not a valid filter expression

```htmldjango
{% include "card.html" with item=card| %}
```

```snapshot
✓ no diagnostics
```

### translate message starts with an underscore

```htmldjango
{% load i18n %}
{% translate _msg %}
```

```snapshot
✓ no diagnostics
```

### blocktranslate count starts with an underscore

```htmldjango
{% load i18n %}
{% blocktranslate count counter=_n %}one{% plural %}many{% endblocktranslate %}
```

```snapshot
✓ no diagnostics
```

### language code starts with an underscore

```htmldjango
{% load i18n %}
{% language _code %}{% endlanguage %}
```

```snapshot
✓ no diagnostics
```

### get_language_info code starts with an underscore

```htmldjango
{% load i18n %}
{% get_language_info for _code as lang %}
```

```snapshot
✓ no diagnostics
```

### static path starts with an underscore

```htmldjango
{% load static %}
{% static _path %}
```

```snapshot
✓ no diagnostics
```

### timezone value is not a valid filter expression

```htmldjango
{% load tz %}
{% timezone tz| %}{% endtimezone %}
```

```snapshot
✓ no diagnostics
```

### cache timeout is not a valid filter expression

```htmldjango
{% load cache %}
{% cache 500| sidebar %}{% endcache %}
```

```snapshot
✓ no diagnostics
```

### querystring keyword value starts with an underscore

```htmldjango
{% querystring page=_page %}
```

```snapshot
✓ no diagnostics
```

### simple tag argument starts with an underscore

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {}}}]
```

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def greet(name): return f"Hello, {name}"
```

```htmldjango
{% greet _name %}
```

```snapshot
✓ no diagnostics
```

### unknown filter in a tag argument

```htmldjango
{% for x in items|nope %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

### filter arity in a tag argument

```htmldjango
{% firstof value|default %}
```

```snapshot
✓ no diagnostics
```
