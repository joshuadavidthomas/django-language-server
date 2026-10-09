# django-cotton

[django-cotton](https://github.com/wrabit/django-cotton) components are written as HTML elements such as `<c-button>`. Its template loader compiles them into `{% cotton %}`, `{% cotton:slot %}`, and `{% cotton:vars %}` tags before Django parses the Template. DJLS analyzes the source file, so it sees the HTML elements as text and validates only the Django syntax written directly. The Python below copies the compile functions from django-cotton 2.7.2 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_cotton']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'loaders': [('django.template.loaders.cached.Loader', ['django_cotton.cotton_loader.Loader', 'django.template.loaders.filesystem.Loader', 'django.template.loaders.app_directories.Loader'])], 'builtins': ['django_cotton.templatetags.cotton'], 'libraries': {}}}]
```

## HTML component syntax and direct tags validate

`django_cotton/__init__.py`:

```py
```

`django_cotton/templatetags/__init__.py`:

```py
```

`django_cotton/templatetags/cotton.py`:

```py
from django import template
from django.utils.html import format_html_join

from django_cotton.templatetags._component import cotton_component
from django_cotton.templatetags._vars import cotton_cvars
from django_cotton.templatetags._slot import cotton_slot

register = template.Library()
register.tag("cotton", cotton_component)
register.tag("cotton:slot", cotton_slot)
register.tag("cotton:vars", cotton_cvars)


@register.filter
def merge(attrs, args):
    for arg in args.split(","):
        key, value = arg.split(":", 1)
        if key in attrs:
            attrs[key] = value + " " + attrs[key]
        else:
            attrs[key] = value
    return format_html_join(" ", '{0}="{1}"', attrs.items())


@register.filter
def get_item(dictionary, key):
    return dictionary.get(key)
```

`django_cotton/templatetags/_component.py`:

```py
from django.template import Library

register = Library()


def cotton_component(parser, token):
    from django_cotton.tag_parser import parse_component_tag
    from django.template import NodeList

    # Check if this is a self-closing tag
    is_self_closing = token.contents.rstrip().endswith('/') or token.contents.rstrip().endswith(' /')

    result = parse_component_tag(token.contents)

    active_library = snapshot_parser_library(parser)

    if is_self_closing:
        # Self-closing tag has no content
        nodelist = NodeList()
    else:
        nodelist = parser.parse(("endcotton",))
        parser.delete_first_token()

    return CottonComponentNode(result.name, nodelist, result.attrs, result.only, active_library)
```

`django_cotton/templatetags/_slot.py`:

```py
from django.template import Library
from django.template.base import TemplateSyntaxError

register = Library()


def cotton_slot(parser, token):
    bits = token.split_contents()[1:]
    if len(bits) < 1:
        raise TemplateSyntaxError("cotton slot tag must include a 'name'")

    nodelist = parser.parse(("endcotton:slot",))
    parser.delete_first_token()
    return CottonSlotNode(bits[0], nodelist)
```

`django_cotton/templatetags/_vars.py`:

```py
from django.template import Library

register = Library()


def cotton_cvars(parser, token):
    from django_cotton.tag_parser import parse_vars_tag

    result = parse_vars_tag(token.contents)
    var_dict = result.attrs
    active_library = snapshot_parser_library(parser)

    return CottonVarsNode(var_dict, result.empty_attrs, active_library)
```

`cotton/card.html`:

```htmldjango
<c-vars title="Untitled" :items="[]" size />
<div {{ attrs|merge:"class:card" }}>
  <h2>{{ title }}</h2>
  <c-slot name="header">{{ header }}</c-slot>
  {% for item in items %}
    <c-button class="btn" :item="item" label="{% if item.active %}On{% else %}Off{% endif %}" />
  {% endfor %}
  <c-icon.arrow-down />
  {{ slot }}
</div>
{% cotton button variant="primary" :count="3" %}Click{% endcotton %}
{% cotton card only %}{% cotton:slot header %}Head{% endcotton:slot %}{% endcotton %}
{% cotton:vars label="{% trans 'Loading' %}" size %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

### loader-only verbatim blocks and self-closing direct tags

`{% cotton:verbatim %}` is not a registered tag. The loader strips it before Django parses the Template, so DJLS reports both delimiters as unknown. `cotton_component` skips its closer when the opening tag ends in `/`; DJLS extracts `endcotton` as a required closer and reports the self-closing forms as unclosed.

`django_cotton/__init__.py`:

```py
```

`django_cotton/templatetags/__init__.py`:

```py
```

`django_cotton/templatetags/cotton.py`:

```py
from django import template

from django_cotton.templatetags._component import cotton_component

register = template.Library()
register.tag("cotton", cotton_component)
```

`django_cotton/templatetags/_component.py`:

```py
from django.template import Library

register = Library()


def cotton_component(parser, token):
    from django_cotton.tag_parser import parse_component_tag
    from django.template import NodeList

    # Check if this is a self-closing tag
    is_self_closing = token.contents.rstrip().endswith('/') or token.contents.rstrip().endswith(' /')

    result = parse_component_tag(token.contents)

    active_library = snapshot_parser_library(parser)

    if is_self_closing:
        # Self-closing tag has no content
        nodelist = NodeList()
    else:
        nodelist = parser.parse(("endcotton",))
        parser.delete_first_token()

    return CottonComponentNode(result.name, nodelist, result.attrs, result.only, active_library)
```

```htmldjango
{% cotton:verbatim %}<c-button>raw</c-button>{% endcotton:verbatim %}
{% cotton button / %}
{% cotton button /%}
```

```snapshot
error[S108]: Unknown tag 'cotton:verbatim'
 --> test.html:1:1
  |
1 | {% cotton:verbatim %}<c-button>raw</c-button>{% endcotton:verbatim %}
  | ^^^^^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endcotton:verbatim'
 --> test.html:1:46
  |
1 | {% cotton:verbatim %}<c-button>raw</c-button>{% endcotton:verbatim %}
  |                                              ^^^^^^^^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'cotton' tag
 --> test.html:2:1
  |
2 | {% cotton button / %}
  | ^^^^^^^^^^^^^^^^^^^^^
error[S100]: Unclosed 'cotton' tag
 --> test.html:3:1
  |
3 | {% cotton button /%}
  | ^^^^^^^^^^^^^^^^^^^^
```
