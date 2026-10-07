# django-web-components

[django-web-components](https://github.com/Xzya/django-web-components) registers `slot`, `render_slot`, and `merge_attrs` in its `components` Template Library. Each `@component.register("card")` call in project code imports that library's `register` and adds `{% card %}...{% endcard %}` and `{% #card %}` tags at runtime. The Python below copies the compile functions from django-web-components 0.2.0 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_web_components']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['django_web_components.templatetags.components'], 'libraries': {}}}]
```

## slot and attribute tags validate

`django_web_components/__init__.py`:

```py
```

`django_web_components/templatetags/__init__.py`:

```py
```

`django_web_components/templatetags/components.py`:

```py
from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.tag("slot")
def do_slot(parser, token):
    tag_name, *remaining_bits = token.split_contents()

    if len(remaining_bits) < 1:
        raise TemplateSyntaxError("'%s' tag takes at least one argument, the slot name" % tag_name)

    slot_name = remaining_bits.pop(0).strip('"')

    nodelist = parser.parse(("endslot",))
    parser.delete_first_token()

    return SlotNode(name=slot_name, nodelist=nodelist)


@register.tag("render_slot")
def do_render_slot(parser, token):
    tag_name, *remaining_bits = token.split_contents()
    if not remaining_bits:
        raise TemplateSyntaxError("'%s' tag takes at least one argument, the slot" % tag_name)

    if len(remaining_bits) > 2:
        raise TemplateSyntaxError("'%s' tag takes at most two arguments, the slot and the argument" % tag_name)

    values = [parser.compile_filter(bit) for bit in remaining_bits]

    if len(values) == 2:
        [slot, argument] = values
    else:
        slot = values.pop()
        argument = None

    return RenderSlotNode(slot, argument)


@register.tag("merge_attrs")
def do_merge_attrs(parser, token):
    tag_name, *remaining_bits = token.split_contents()
    if not remaining_bits:
        raise TemplateSyntaxError("'%s' tag takes at least one argument, the attributes" % tag_name)

    attributes = parser.compile_filter(remaining_bits[0])
    return MergeAttrsNode(attributes, remaining_bits[1:])
```

`components/card.html`:

```htmldjango
<div {% merge_attrs attributes class="card" data-id+="x" %}>
  <header>{% render_slot slots.header %}</header>
  {% for row in slots.row %}{% render_slot row item %}{% endfor %}
  {% render_slot inner_block %}
</div>
```

```snapshot
✓ no diagnostics
```

## render_slot argument counts are checked

`django_web_components/__init__.py`:

```py
```

`django_web_components/templatetags/__init__.py`:

```py
```

`django_web_components/templatetags/components.py`:

```py
from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.tag("render_slot")
def do_render_slot(parser, token):
    tag_name, *remaining_bits = token.split_contents()
    if not remaining_bits:
        raise TemplateSyntaxError("'%s' tag takes at least one argument, the slot" % tag_name)

    if len(remaining_bits) > 2:
        raise TemplateSyntaxError("'%s' tag takes at most two arguments, the slot and the argument" % tag_name)

    return RenderSlotNode(remaining_bits)
```

```htmldjango
{% render_slot %}
{% render_slot slots.row item extra %}
```

```snapshot
error[S117]: 'render_slot' tag takes at least one argument, the slot
 --> test.html:1:1
  |
1 | {% render_slot %}
  | ^^^^^^^^^^^^^^^^^
error[S117]: 'render_slot' tag takes at most two arguments, the slot and the argument
 --> test.html:2:1
  |
2 | {% render_slot slots.row item extra %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## Known gaps

### component tags added at runtime

DJLS reads `components` as a closed library. It does not see `django_web_components/component.py` add component tags to it, so it reports each component tag and its closer.

`django_web_components/__init__.py`:

```py
```

`django_web_components/templatetags/__init__.py`:

```py
```

`django_web_components/templatetags/components.py`:

```py
from django import template
from django.template import TemplateSyntaxError

register = template.Library()


@register.tag("slot")
def do_slot(parser, token):
    tag_name, *remaining_bits = token.split_contents()

    if len(remaining_bits) < 1:
        raise TemplateSyntaxError("'%s' tag takes at least one argument, the slot name" % tag_name)

    nodelist = parser.parse(("endslot",))
    parser.delete_first_token()

    return SlotNode(name=remaining_bits[0], nodelist=nodelist)
```

`django_web_components/component.py`:

```py
from django import template


def register(name=None, component=None, target_register: template.Library = None):
    from django_web_components.templatetags.components import (
        create_component_tag,
        register as tag_register,
    )

    if target_register is None:
        target_register = tag_register

    target_register.tag(name, create_component_tag(name))
    target_register.tag(f"#{name}", create_component_tag(name))
```

```htmldjango
{% card title="Hello" %}
  {% slot header %}Header{% endslot %}
{% endcard %}
{% #alert %}
```

```snapshot
error[S108]: Unknown tag 'card'
 --> test.html:1:1
  |
1 | {% card title="Hello" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endcard'
 --> test.html:3:1
  |
3 | {% endcard %}
  | ^^^^^^^^^^^^^
error[S108]: Unknown tag '#alert'
 --> test.html:4:1
  |
4 | {% #alert %}
  | ^^^^^^^^^^^^
```
