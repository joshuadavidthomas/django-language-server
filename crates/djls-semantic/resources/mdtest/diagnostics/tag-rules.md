# Tag argument diagnostics

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {}}}]
```

## tag requires an argument

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def one_arg_tag(value): pass
```

```htmldjango
{% one_arg_tag %}
```

```snapshot
error[S117]: 'one_arg_tag' did not receive value(s) for the argument(s): 'value'
 --> test.html:1:1
  |
1 | {% one_arg_tag %}
  | ^^^^^^^^^^^^^^^^^
```

## tag accepts exactly one argument

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def one_arg_tag(value): pass
```

```htmldjango
{% one_arg_tag first second %}
```

```snapshot
error[S117]: 'one_arg_tag' received too many positional arguments
 --> test.html:1:1
  |
1 | {% one_arg_tag first second %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## selects the matching routed diagnostic

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.tag("routed")
def compile_routed(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, branch, value, ending = bits
        if branch == "first":
            if ending != "done":
                raise template.TemplateSyntaxError("first branch: %s" % token.contents)
        elif branch == "second":
            if ending != "done":
                raise template.TemplateSyntaxError("second branch: %s" % token.contents)
        else:
            raise template.TemplateSyntaxError("bad branch")
    else:
        raise template.TemplateSyntaxError("bad count")
    return Node()
```

```htmldjango
{% routed first value wrong %}
```

```snapshot
error[S117]: first branch: routed first value wrong
 --> test.html:1:1
  |
1 | {% routed first value wrong %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## selects another routed diagnostic

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.tag("routed")
def compile_routed(parser, token):
    bits = token.split_contents()
    if len(bits) == 4:
        tag, branch, value, ending = bits
        if branch == "first":
            if ending != "done":
                raise template.TemplateSyntaxError("first branch: %s" % token.contents)
        elif branch == "second":
            if ending != "done":
                raise template.TemplateSyntaxError("second branch: %s" % token.contents)
        else:
            raise template.TemplateSyntaxError("bad branch")
    else:
        raise template.TemplateSyntaxError("bad count")
    return Node()
```

```htmldjango
{% routed second value wrong %}
```

```snapshot
error[S117]: second branch: routed second value wrong
 --> test.html:1:1
  |
1 | {% routed second value wrong %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## normalizes bits in an aliased tag diagnostic

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['loop_tags'], 'libraries': {}}}]
```

`loop_tags.py`:

```py
from django import template
register = template.Library()
@register.tag("targetTag")
def compile_loop(parser, token):
    bits = token.split_contents()
    if bits[2] != "in":
        raise template.TemplateSyntaxError("Use 100%% syntax: %r" % token.contents)
    return Node()
```

```htmldjango
{% targetTag   "quoted value"   from %}
```

```snapshot
error[S117]: Use 100% syntax: 'targetTag "quoted value" from'
 --> test.html:1:1
  |
1 | {% targetTag   "quoted value"   from %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
