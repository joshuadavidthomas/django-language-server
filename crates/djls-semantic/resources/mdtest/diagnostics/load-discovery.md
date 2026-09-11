# Load discovery diagnostics

## rebuilding structure reveals a later load

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['opaque_tags'], 'libraries': {'first': 'first_tags', 'second': 'second_tags'}}}]
```

`opaque_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="shadow")
def shadow(parser, token):
    parser.skip_past("endshadow")
    return Node()
```

`first_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="shadow")
def shadow(): pass
```

`second_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def revealed(): pass
```

```htmldjango
{% load first %}{% shadow %}{% load second %}{% endshadow %}{% revealed %}
```

```snapshot
error[S108]: Unknown tag 'endshadow'
 --> test.html:1:46
  |
1 | {% load first %}{% shadow %}{% load second %}{% endshadow %}{% revealed %}
  |                                              ^^^^^^^^^^^^^^^
```

## newly opaque grammar discards hidden loads and diagnostics

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {'gates': 'gate_tags', 'hidden': 'hidden_tags'}}}]
```

`gate_tags.py`:

```py
from django import template
register = template.Library()
@register.tag
def gate(parser, token):
    parser.skip_past("endgate")
    return Node()
```

`hidden_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="if")
def custom_if(value): pass
```

```htmldjango
{% load gates %}{% gate %}{% load hidden %}{% endif %}{% endgate %}{% if value %}
```

```snapshot
error[S100]: Unclosed 'if' tag
 --> test.html:1:68
  |
1 | {% load gates %}{% gate %}{% load hidden %}{% endif %}{% endgate %}{% if value %}
  |                                                                    ^^^^^^^^^^^^^^
```

## structural diagnostics appear once after convergence

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
def customblock(parser, token):
    parser.parse(("endcustomblock",))
    return Node()
```

```htmldjango
{% load custom %}{% customblock %}
```

```snapshot
error[S100]: Unclosed 'customblock' tag
 --> test.html:1:18
  |
1 | {% load custom %}{% customblock %}
  |                  ^^^^^^^^^^^^^^^^^
```

## load discovery crosses ten grammar changes

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['opaque_tags'], 'libraries': {'chain_0': 'chain_0_tags', 'chain_1': 'chain_1_tags', 'chain_2': 'chain_2_tags', 'chain_3': 'chain_3_tags', 'chain_4': 'chain_4_tags', 'chain_5': 'chain_5_tags', 'chain_6': 'chain_6_tags', 'chain_7': 'chain_7_tags', 'chain_8': 'chain_8_tags', 'chain_9': 'chain_9_tags', 'chain_10': 'chain_10_tags'}}}]
```

`opaque_tags.py`:

```py
from django import template
register = template.Library()
@register.tag(name="gate_0")
def gate_0(parser, token):
    parser.skip_past("endgate_0")
    return Node()
@register.tag(name="gate_1")
def gate_1(parser, token):
    parser.skip_past("endgate_1")
    return Node()
@register.tag(name="gate_2")
def gate_2(parser, token):
    parser.skip_past("endgate_2")
    return Node()
@register.tag(name="gate_3")
def gate_3(parser, token):
    parser.skip_past("endgate_3")
    return Node()
@register.tag(name="gate_4")
def gate_4(parser, token):
    parser.skip_past("endgate_4")
    return Node()
@register.tag(name="gate_5")
def gate_5(parser, token):
    parser.skip_past("endgate_5")
    return Node()
@register.tag(name="gate_6")
def gate_6(parser, token):
    parser.skip_past("endgate_6")
    return Node()
@register.tag(name="gate_7")
def gate_7(parser, token):
    parser.skip_past("endgate_7")
    return Node()
@register.tag(name="gate_8")
def gate_8(parser, token):
    parser.skip_past("endgate_8")
    return Node()
@register.tag(name="gate_9")
def gate_9(parser, token):
    parser.skip_past("endgate_9")
    return Node()
```

`chain_0_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_0")
def gate_0(): pass
```

`chain_1_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_1")
def gate_1(): pass
```

`chain_2_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_2")
def gate_2(): pass
```

`chain_3_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_3")
def gate_3(): pass
```

`chain_4_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_4")
def gate_4(): pass
```

`chain_5_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_5")
def gate_5(): pass
```

`chain_6_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_6")
def gate_6(): pass
```

`chain_7_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_7")
def gate_7(): pass
```

`chain_8_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_8")
def gate_8(): pass
```

`chain_9_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag(name="gate_9")
def gate_9(): pass
```

`chain_10_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def revealed(): pass
```

```htmldjango
{% load chain_0 %}{% gate_0 %}{% load chain_1 %}{% endgate_0 %}{% gate_1 %}{% load chain_2 %}{% endgate_1 %}{% gate_2 %}{% load chain_3 %}{% endgate_2 %}{% gate_3 %}{% load chain_4 %}{% endgate_3 %}{% gate_4 %}{% load chain_5 %}{% endgate_4 %}{% gate_5 %}{% load chain_6 %}{% endgate_5 %}{% gate_6 %}{% load chain_7 %}{% endgate_6 %}{% gate_7 %}{% load chain_8 %}{% endgate_7 %}{% gate_8 %}{% load chain_9 %}{% endgate_8 %}{% gate_9 %}{% load chain_10 %}{% endgate_9 %}{% revealed %}
```

```snapshot
error[S108]: Unknown tag 'endgate_0'
 --> test.html:1:49
  |
1 | {% load chain_0 %}{% gate_0 %}{% load chain_1 %}{% endgate_0 %}{% gate_1 %}{% load chain_2 %}{% endgate_1 %}{% gate_2 %}{% load chain...
  |                                                 ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_1'
 --> test.html:1:94
  |
1 | {% load chain_0 %}{% gate_0 %}{% load chain_1 %}{% endgate_0 %}{% gate_1 %}{% load chain_2 %}{% endgate_1 %}{% gate_2 %}{% load chain...
  |                                                                                              ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_2'
 --> test.html:1:139
  |
1 | ...d chain_2 %}{% endgate_1 %}{% gate_2 %}{% load chain_3 %}{% endgate_2 %}{% gate_3 %}{% load chain_4 %}{% endgate_3 %}{% gate_4 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_3'
 --> test.html:1:184
  |
1 | ...d chain_3 %}{% endgate_2 %}{% gate_3 %}{% load chain_4 %}{% endgate_3 %}{% gate_4 %}{% load chain_5 %}{% endgate_4 %}{% gate_5 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_4'
 --> test.html:1:229
  |
1 | ...d chain_4 %}{% endgate_3 %}{% gate_4 %}{% load chain_5 %}{% endgate_4 %}{% gate_5 %}{% load chain_6 %}{% endgate_5 %}{% gate_6 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_5'
 --> test.html:1:274
  |
1 | ...d chain_5 %}{% endgate_4 %}{% gate_5 %}{% load chain_6 %}{% endgate_5 %}{% gate_6 %}{% load chain_7 %}{% endgate_6 %}{% gate_7 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_6'
 --> test.html:1:319
  |
1 | ...d chain_6 %}{% endgate_5 %}{% gate_6 %}{% load chain_7 %}{% endgate_6 %}{% gate_7 %}{% load chain_8 %}{% endgate_7 %}{% gate_8 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_7'
 --> test.html:1:364
  |
1 | ...d chain_7 %}{% endgate_6 %}{% gate_7 %}{% load chain_8 %}{% endgate_7 %}{% gate_8 %}{% load chain_9 %}{% endgate_8 %}{% gate_9 %}{...
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_8'
 --> test.html:1:409
  |
1 | ...d chain_8 %}{% endgate_7 %}{% gate_8 %}{% load chain_9 %}{% endgate_8 %}{% gate_9 %}{% load chain_10 %}{% endgate_9 %}{% revealed %}
  |                                                             ^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endgate_9'
 --> test.html:1:455
  |
1 | ... chain_9 %}{% endgate_8 %}{% gate_9 %}{% load chain_10 %}{% endgate_9 %}{% revealed %}
  |                                                             ^^^^^^^^^^^^^^^
```

## a custom if does not run builtin expression validation

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
def custom_if(*args):
    pass
```

```htmldjango
{% load custom %}{% if and value %}
```

```snapshot
✓ no diagnostics
```
