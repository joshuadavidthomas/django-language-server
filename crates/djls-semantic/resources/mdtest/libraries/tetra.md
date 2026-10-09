# Tetra

[Tetra](https://github.com/tetra-framework/tetra) registers a fixed set of tags in its `tetra` Template Library. When a component class is decorated with `@library.register`, `tetra/library.py` imports that library's `register` and adds `{% ComponentName %}` and `{% library.ComponentName %}` tags at runtime. A block component closes with `{% /ComponentName %}`. The Python below copies the compile functions from Tetra 0.9.2 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['tetra']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

## the fixed tags validate

`tetra/__init__.py`:

```py
```

`tetra/templatetags/__init__.py`:

```py
```

`tetra/templatetags/tetra.py`:

```py
from django import template
from django.template import TemplateSyntaxError
from django.utils.safestring import mark_safe

register = template.Library()


@register.simple_tag(takes_context=True, name="tetra_scripts")
def scripts_placeholder_tag(context, include_alpine=False):
    return mark_safe("")


@register.simple_tag(takes_context=True, name="tetra_styles")
def styles_placeholder_tag(context):
    return mark_safe("")


@register.tag(name="component")
def do_component(parser, token):
    split_contents = token.split_contents()
    if len(split_contents) < 2:
        raise TemplateSyntaxError("Component tag requires a component name")
    component_name = split_contents[1]
    bits = split_contents[2:]

    component_name = component_name.strip("'\"")

    # If the tag ends with a / than it has no content, otherwise it does.
    has_content = True
    if (len(bits) > 0) and (bits[-1] == "/"):
        has_content = False
        bits = bits[:-1]

    nodelist = None
    if has_content:
        nodelist = parser.parse((f"/{component_name}",))
        parser.delete_first_token()

    return ComponentNode(component_name, bits, nodelist)


@register.tag(name="...")
def do_attr_tag(parser, token):
    split_contents = token.split_contents()
    if len(split_contents) < 2:
        raise TemplateSyntaxError("Attr tag requires at least one argument")
    bits = split_contents[1:]
    return AttrsNode([parser.compile_filter(bit) for bit in bits])


@register.tag("slot")
def do_slot(parser, token):
    bits = token.contents.split()
    if len(bits) not in (2, 3, 5):
        raise TemplateSyntaxError("'%s' tag takes one, two or four arguments" % bits[0])
    if len(bits) > 2 and bits[2] != "expose":
        raise TemplateSyntaxError(
            "'%s' tag second argument can only be 'expose' if given" % bits[0]
        )
    if len(bits) == 5 and bits[3] != "as":
        raise TemplateSyntaxError(
            "'%s' tag third argument can only be 'as' if given" % bits[0]
        )

    slot_name = bits[1]
    nodelist = parser.parse(("endslot",))

    endslot = parser.next_token()
    acceptable_endblocks = ("endslot", "endslot %s" % slot_name)
    if endslot.contents not in acceptable_endblocks:
        parser.invalid_block_tag(endslot, "endslot", acceptable_endblocks)

    return BlockNode(slot_name, nodelist)


@register.tag(name="livevar")
def live_variable(parser, token):
    split_contents = token.split_contents()
    if len(split_contents) > 3:
        raise TemplateSyntaxError(
            "livevar tag requires maximum two arguments: the variable name, "
            "and optionally the HTML tag."
        )
    var_name = split_contents[1]
    return LiveVariableNode(var_name, "span")


@register.filter(name="if")
def if_filter(value, condition):
    if condition:
        return value
    else:
        return ""


@register.filter(name="else")
def else_filter(value, else_value):
    if value:
        return value
    else:
        return else_value
```

`components/card.html`:

```htmldjango
{% load tetra %}
{% tetra_styles %}{% tetra_scripts include_alpine=True %}
<div class="card" {% ... attrs class="p-4" %}>
  {% slot title expose as card_title %}Untitled{% endslot %}
  {% slot default %}{% endslot %}
  {% livevar count tag=span %}
  {{ label|if:active|else:"inactive" }}
</div>
{% component Avatar user=user / %}
```

```snapshot
✓ no diagnostics
```

## the fixed tags are still checked

`tetra/__init__.py`:

```py
```

`tetra/templatetags/__init__.py`:

```py
```

`tetra/templatetags/tetra.py`:

```py
from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.tag(name="livevar")
def live_variable(parser, token):
    split_contents = token.split_contents()
    if len(split_contents) > 3:
        raise TemplateSyntaxError(
            "livevar tag requires maximum two arguments: the variable name, "
            "and optionally the HTML tag."
        )
    var_name = split_contents[1]
    return LiveVariableNode(var_name, "span")


@register.filter(name="if")
def if_filter(value, condition):
    if condition:
        return value
    else:
        return ""
```

```htmldjango
{% load tetra %}
{% livevar count tag=span extra %}
{{ label|if }}
```

```snapshot
error[S117]: livevar tag requires maximum two arguments: the variable name, and optionally the HTML tag.
 --> test.html:2:1
  |
2 | {% livevar count tag=span extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
error[S115]: Filter 'if' requires an argument
 --> test.html:3:10
  |
3 | {{ label|if }}
  |          ^^
```

## Known gaps

### component tags added at runtime and `/Name` closers

DJLS reads `tetra.templatetags.tetra` as a closed library. It does not see `tetra/library.py` add `Card` to that library, so it reports `{% Card %}` and its closer. `do_component` builds its closer from the component name, so DJLS also cannot pair `{% component Card %}` with `{% /Card %}`.

`tetra/__init__.py`:

```py
```

`tetra/templatetags/__init__.py`:

```py
```

`tetra/templatetags/tetra.py`:

```py
from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.tag(name="component")
def do_component(parser, token):
    split_contents = token.split_contents()
    if len(split_contents) < 2:
        raise TemplateSyntaxError("Component tag requires a component name")
    component_name = split_contents[1]
    bits = split_contents[2:]

    component_name = component_name.strip("'\"")

    # If the tag ends with a / than it has no content, otherwise it does.
    has_content = True
    if (len(bits) > 0) and (bits[-1] == "/"):
        has_content = False
        bits = bits[:-1]

    nodelist = None
    if has_content:
        nodelist = parser.parse((f"/{component_name}",))
        parser.delete_first_token()

    return ComponentNode(component_name, bits, nodelist)
```

`tetra/library.py`:

```py
class Library:
    def register(self, component_cls=None, name=None):
        def component_tag_compile_function(parser, token):
            tag = token.contents.split()[0]
            if tag != "component":
                token.contents = (
                    f"component {tag} {' '.join(token.contents.split()[1:])}"
                )
            return do_component(parser, token)

        from .templatetags.tetra import do_component, register as tetra_register

        tetra_register.tag(name=name, compile_function=component_tag_compile_function)
        tetra_register.tag(
            name=f"{self.name}.{name}",
            compile_function=component_tag_compile_function,
        )
        return component_cls
```

```htmldjango
{% load tetra %}
{% component Card title="Hi" %}Body{% /Card %}
{% Card title="Hi" / %}
{% Card %}Body{% /Card %}
```

```snapshot
error[S108]: Unknown tag '/Card'
 --> test.html:2:36
  |
2 | {% component Card title="Hi" %}Body{% /Card %}
  |                                    ^^^^^^^^^^^
error[S108]: Unknown tag 'Card'
 --> test.html:3:1
  |
3 | {% Card title="Hi" / %}
  | ^^^^^^^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'Card'
 --> test.html:4:1
  |
4 | {% Card %}Body{% /Card %}
  | ^^^^^^^^^^
error[S108]: Unknown tag '/Card'
 --> test.html:4:15
  |
4 | {% Card %}Body{% /Card %}
  |               ^^^^^^^^^^^
```
