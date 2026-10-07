# django-bird

[django-bird](https://github.com/joshuadavidthomas/django-bird) registers its tags from sibling modules: `register.tag(slot.TAG, slot.do_slot)`. Each compile function names its closer through a module constant, `parser.parse((END_TAG,))`. The Python below copies the compile functions from django-bird 0.19.0 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_bird']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['django_bird.templatetags.django_bird'], 'libraries': {}}}]
```

## a component template uses slots, props, vars, and assets

`django_bird/__init__.py`:

```py
```

`django_bird/templatetags/__init__.py`:

```py
```

`django_bird/templatetags/django_bird.py`:

```py
from __future__ import annotations

from django import template

from .tags import asset
from .tags import bird
from .tags import load
from .tags import prop
from .tags import slot
from .tags import var

register = template.Library()


register.tag(asset.AssetTag.CSS.value, asset.do_asset)
register.tag(asset.AssetTag.JS.value, asset.do_asset)
register.tag(bird.TAG, bird.do_bird)
register.tag(load.TAG, load.do_load)
register.tag(prop.TAG, prop.do_prop)
register.tag(slot.TAG, slot.do_slot)
register.tag(var.TAG, var.do_var)
register.tag(var.END_TAG, var.do_end_var)
```

`django_bird/templatetags/tags/__init__.py`:

```py
```

`django_bird/templatetags/tags/asset.py`:

```py
from enum import Enum

from django import template


class AssetTag(Enum):
    CSS = "bird:css"
    JS = "bird:js"


def do_asset(_parser, token):
    bits = token.split_contents()
    if len(bits) < 1:
        msg = "bird:assets tag requires at least one argument"
        raise template.TemplateSyntaxError(msg)
    tag_name = bits[0]
    asset_tag = AssetTag(tag_name)
    return AssetNode(asset_tag)
```

`django_bird/templatetags/tags/bird.py`:

```py
from django import template

TAG = "bird"
END_TAG = "endbird"


def do_bird(parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"{TAG} tag requires at least one argument"
        raise template.TemplateSyntaxError(msg)

    name = bits.pop(0)
    attrs = {}
    explicit_context_mode = None

    for bit in bits:
        match bit:
            case "only" | "inherit":
                if explicit_context_mode and explicit_context_mode != bit:
                    msg = f"{TAG} tag cannot use both 'only' and 'inherit'"
                    raise template.TemplateSyntaxError(msg)
                explicit_context_mode = bit
            case "/":
                continue
            case _:
                if "=" in bit:
                    key, value = bit.split("=", 1)
                else:
                    key = bit
                    value = "True"
                attrs[key] = parser.compile_filter(value)

    nodelist = parse_nodelist(bits, parser)
    return BirdNode(name, attrs, nodelist)


def parse_nodelist(bits, parser):
    # self-closing tag
    # {% bird name / %}
    if len(bits) > 0 and bits[-1] == "/":
        nodelist = None
    else:
        nodelist = parser.parse((END_TAG,))
        parser.delete_first_token()
    return nodelist
```

`django_bird/templatetags/tags/load.py`:

```py
from django import template

TAG = "bird:load"


def do_load(_parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"{TAG} tag requires at least one component name"
        raise template.TemplateSyntaxError(msg)

    component_names = [bit.strip("\"'") for bit in bits]
    return LoadNode(component_names=component_names)
```

`django_bird/templatetags/tags/prop.py`:

```py
from django import template

TAG = "bird:prop"


def do_prop(parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"{TAG} tag requires at least one argument"
        raise template.TemplateSyntaxError(msg)

    prop = bits.pop(0)

    try:
        name, default = prop.split("=", 1)
    except ValueError:
        name = prop
        default = "None"

    return PropNode(name, parser.compile_filter(default), bits)
```

`django_bird/templatetags/tags/slot.py`:

```py
from django import template

TAG = "bird:slot"
END_TAG = "endbird:slot"

DEFAULT_SLOT = "default"


def do_slot(parser, token):
    _tag, *bits = token.split_contents()
    if len(bits) > 1:
        msg = f"{TAG} tag requires either one or no arguments"
        raise template.TemplateSyntaxError(msg)

    if len(bits) == 0:
        name = DEFAULT_SLOT
    else:
        name = bits[0]
        if name.startswith("name="):
            _, name = name.split("=")
        name = name.strip("'\"")

    nodelist = parser.parse((END_TAG,))
    parser.delete_first_token()

    return SlotNode(name, nodelist)
```

`django_bird/templatetags/tags/var.py`:

```py
import re

from django import template

register = template.Library()

TAG = "bird:var"
END_TAG = "endbird:var"

OPERATOR_PATTERN = re.compile(r"(\w+)\s*(\+=|=)\s*(.+)")


def do_var(parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"'{TAG}' tag requires an assignment"
        raise template.TemplateSyntaxError(msg)

    var_assignment = bits.pop(0)
    match = re.match(OPERATOR_PATTERN, var_assignment)
    if not match:
        msg = (
            f"Invalid assignment in '{TAG}' tag: {var_assignment}. "
            f"Expected format: {TAG} variable='value' or {TAG} variable+='value'."
        )
        raise template.TemplateSyntaxError(msg)

    var_name, operator, var_value = match.groups()
    var_value = var_value.strip()
    value = parser.compile_filter(var_value)

    return VarNode(var_name, operator, value)


def do_end_var(_parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"{token.contents.split()[0]} tag requires a variable name"
        raise template.TemplateSyntaxError(msg)

    var_name = bits.pop(0)

    return EndVarNode(var_name)
```

`bird/button.html`:

```htmldjango
{% bird:prop variant="primary" %}
{% bird:prop size %}
{% bird:var classes='btn' %}
{% bird:var classes+=' btn-lg' %}
<button {{ attrs }} class="{{ vars.classes }} {{ props.variant }}">
  {% bird:slot leading-icon %}{% endbird:slot %}
  {% bird:slot %}{{ slot }}{% endbird:slot %}
  {% bird:slot default %}{% endbird:slot %}
  {% bird:slot name="trailing" %}{% endbird:slot %}
</button>
{% endbird:var classes %}
{% bird:css %}
{% bird:js %}
{% bird:load button icon %}
```

```snapshot
✓ no diagnostics
```

## slot argument counts and closers are checked

`django_bird/__init__.py`:

```py
```

`django_bird/templatetags/__init__.py`:

```py
```

`django_bird/templatetags/django_bird.py`:

```py
from django import template

from .tags import slot

register = template.Library()
register.tag(slot.TAG, slot.do_slot)
```

`django_bird/templatetags/tags/__init__.py`:

```py
```

`django_bird/templatetags/tags/slot.py`:

```py
from django import template

TAG = "bird:slot"
END_TAG = "endbird:slot"

DEFAULT_SLOT = "default"


def do_slot(parser, token):
    _tag, *bits = token.split_contents()
    if len(bits) > 1:
        msg = f"{TAG} tag requires either one or no arguments"
        raise template.TemplateSyntaxError(msg)

    if len(bits) == 0:
        name = DEFAULT_SLOT
    else:
        name = bits[0]
        if name.startswith("name="):
            _, name = name.split("=")
        name = name.strip("'\"")

    nodelist = parser.parse((END_TAG,))
    parser.delete_first_token()

    return SlotNode(name, nodelist)
```

`bird/card.html`:

```htmldjango
{% bird:slot header footer %}{% endbird:slot %}
{% bird:slot body %}
{% endbird:slot %}{% endbird:slot %}
```

```snapshot
error[S117]: Tag 'bird:slot' accepts at most 1 argument
 --> bird/card.html:1:1
  |
1 | {% bird:slot header footer %}{% endbird:slot %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
error[S101]: 'endbird:slot' has no matching 'bird:slot' block
 --> bird/card.html:3:19
  |
3 | {% endbird:slot %}{% endbird:slot %}
  |                   ^^^^^^^^^^^^^^^^^^
```

## Known gaps

### component closers and self-closing components

`do_bird` parses its body in the `parse_nodelist` helper, and only when the opening tag does not end in `/`. DJLS does not follow the helper or model the `/` form, so it treats `bird` as a Standalone Tag and reports every `{% endbird %}`.

`django_bird/__init__.py`:

```py
```

`django_bird/templatetags/__init__.py`:

```py
```

`django_bird/templatetags/django_bird.py`:

```py
from django import template

from .tags import bird

register = template.Library()
register.tag(bird.TAG, bird.do_bird)
```

`django_bird/templatetags/tags/__init__.py`:

```py
```

`django_bird/templatetags/tags/bird.py`:

```py
from django import template

TAG = "bird"
END_TAG = "endbird"


def do_bird(parser, token):
    _tag, *bits = token.split_contents()
    if not bits:
        msg = f"{TAG} tag requires at least one argument"
        raise template.TemplateSyntaxError(msg)

    name = bits.pop(0)
    attrs = {}
    explicit_context_mode = None

    for bit in bits:
        match bit:
            case "only" | "inherit":
                if explicit_context_mode and explicit_context_mode != bit:
                    msg = f"{TAG} tag cannot use both 'only' and 'inherit'"
                    raise template.TemplateSyntaxError(msg)
                explicit_context_mode = bit
            case "/":
                continue
            case _:
                if "=" in bit:
                    key, value = bit.split("=", 1)
                else:
                    key = bit
                    value = "True"
                attrs[key] = parser.compile_filter(value)

    nodelist = parse_nodelist(bits, parser)
    return BirdNode(name, attrs, nodelist)


def parse_nodelist(bits, parser):
    # self-closing tag
    # {% bird name / %}
    if len(bits) > 0 and bits[-1] == "/":
        nodelist = None
    else:
        nodelist = parser.parse((END_TAG,))
        parser.delete_first_token()
    return nodelist
```

```htmldjango
{% bird button variant="primary" disabled %}Click{% endbird %}
{% bird icon.arrow-down / %}
{% bird "button" only / %}
```

```snapshot
error[S108]: Unknown tag 'endbird'
 --> test.html:1:50
  |
1 | {% bird button variant="primary" disabled %}Click{% endbird %}
  |                                                  ^^^^^^^^^^^^^
```
