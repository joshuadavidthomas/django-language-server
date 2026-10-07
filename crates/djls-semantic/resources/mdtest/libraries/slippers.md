# Slippers

[Slippers](https://github.com/mixxorz/slippers) reads component names from `components.yaml` when the app starts and registers `{% name %}` and `{% #name %}` tags with `register_components`. A block component closes with `{% /name %}`. Because `register_components` mutates the module's `register`, DJLS treats `slippers` as an open Template Library. The Python below copies the registration code and compile functions from Slippers 0.7.1 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['slippers']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['slippers.templatetags.slippers'], 'libraries': {}}}]
```

## YAML-registered components validate without false positives

`slippers/__init__.py`:

```py
```

`slippers/templatetags/__init__.py`:

```py
```

`slippers/templatetags/slippers.py`:

```py
from django import template
from django.template import NodeList

register = template.Library()


def create_component_tag(template_path):
    def do_component(parser, token):
        tag_name, *remaining_bits = token.split_contents()

        # Block components start with `#`
        # Expect a closing tag
        if tag_name[0] == "#":
            nodelist = parser.parse((f"/{tag_name[1:]}",))
            parser.delete_first_token()
        else:
            nodelist = NodeList()

        return ComponentNode(tag_name=tag_name, nodelist=nodelist, template_path=template_path)

    return do_component


def register_components(components, target_register=None):
    if target_register is None:
        target_register = register
    for tag_name, template_path in components.items():
        # Inline component
        target_register.tag(f"{tag_name}", create_component_tag(template_path))

        # Block component
        target_register.tag(f"#{tag_name}", create_component_tag(template_path))


@register.tag(name="attrs")
def do_attrs(parser, token):
    tag_name, *attrs = token.split_contents()
    return AttrsNode(attrs)


@register.tag(name="var")
def do_var(parser, token):
    bits = token.split_contents()
    if len(bits) < 2:
        raise template.TemplateSyntaxError(f"{bits[0]} tag requires at least one keyword argument")
    return VarNode(bits[1:])


@register.filter(name="match")
def do_match(match_key, mapping):
    return mapping


@register.tag(name="fragment")
def do_fragment(parser, token):
    error_message = "The syntax for fragment is {% fragment as variable_name %}"
    try:
        tag_name, _, target_var = token.split_contents()
        nodelist = parser.parse(("endfragment",))
        parser.delete_first_token()
    except ValueError:
        raise template.TemplateSyntaxError(error_message)
    return FragmentNode(nodelist, target_var)


@register.inclusion_tag("slippers/overlay.html")
def slippers_overlay():
    return {}
```

`components/page.html`:

```htmldjango
{% #button variant="primary" %}Save{% /button %}
{% avatar user=user size="sm" %}
{% fragment as heading %}<h1>{{ title }}</h1>{% endfragment %}
{% var classes="btn" %}
<button {% attrs type id %} class="{{ variant|match:'primary:btn-primary,secondary:btn-secondary' }}">
{% slippers_overlay %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

### the open builtin hides every unknown name and orphaned closer

`slippers` is a builtin whose inventory is open, so DJLS reports neither unknown names nor orphaned closers in any Template. Reading `components.yaml` would let DJLS close the inventory.

`slippers/__init__.py`:

```py
```

`slippers/templatetags/__init__.py`:

```py
```

`slippers/templatetags/slippers.py`:

```py
from django import template

register = template.Library()


def register_components(components, target_register=None):
    if target_register is None:
        target_register = register
    for tag_name, template_path in components.items():
        target_register.tag(f"{tag_name}", create_component_tag(template_path))
        target_register.tag(f"#{tag_name}", create_component_tag(template_path))
```

```htmldjango
{% bogus %}{% endif %}{{ value|missing_filter }}
```

```snapshot
✓ no diagnostics
```
