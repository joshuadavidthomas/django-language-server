# django-viewcomponent

[django-viewcomponent](https://github.com/rails-inspire-django/django-viewcomponent) renders Python component classes through `{% component %}` and fills their slots with `{% call %}`. Its library is built with `import django.template` and `django.template.Library()`. Its compile functions pass the closer as `parser.parse(parse_until=[...])`, and one assigns the result with a type annotation. The Python below copies the compile functions from django-viewcomponent 1.0.11 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_viewcomponent']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['django_viewcomponent.templatetags.viewcomponent_tags'], 'libraries': {}}}]
```

## components and slot calls validate

`django_viewcomponent/__init__.py`:

```py
```

`django_viewcomponent/templatetags/__init__.py`:

```py
```

`django_viewcomponent/templatetags/viewcomponent_tags.py`:

```py
import django.template
from django.template.base import NodeList
from django.template.exceptions import TemplateSyntaxError
from django.template.library import parse_bits

register = django.template.Library()


@register.tag("call")
def do_call(parser, token):
    bits = token.split_contents()

    # check as keyword
    target_var = None
    if len(bits) >= 4 and bits[-2] == "as":
        target_var = bits[-1]
        bits = bits[:-2]

    tag_name = "call"
    tag_args, tag_kwargs = parse_bits(
        parser=parser,
        bits=bits,
        params=[],
        takes_context=False,
        name=tag_name,
        varargs=True,
        varkw=[],
        defaults=None,
        kwonly=[],
        kwonly_defaults=None,
    )

    if len(tag_args) > 1:
        # At least one position arg, so take the first as the component name
        args = tag_args[1:]
        kwargs = tag_kwargs
    else:
        raise TemplateSyntaxError(f"Syntax error in '{tag_name}' tag")

    nodelist = parser.parse(parse_until=["endcall"])
    parser.delete_first_token()

    return CallNode(parser=parser, nodelist=nodelist, target_var=target_var, args=args, kwargs=kwargs)


@register.tag(name="component")
def do_component(parser, token):
    bits = token.split_contents()

    # check as keyword
    target_var = None
    if len(bits) >= 4 and bits[-2] == "as":
        target_var = bits[-1]
        bits = bits[:-2]

    component_name, context_args, context_kwargs = parse_component_with_arguments(
        parser,
        bits,
        "component",
    )
    nodelist: NodeList = parser.parse(parse_until=["endcomponent"])
    parser.delete_first_token()

    return ComponentNode(component_name, context_args, context_kwargs, nodelist, target_var)


def parse_component_with_arguments(parser, bits, tag_name):
    tag_args, tag_kwargs = parse_bits(
        parser=parser,
        bits=bits,
        params=["tag_name", "name"],
        takes_context=False,
        name=tag_name,
        varargs=True,
        varkw=[],
        defaults=None,
        kwonly=[],
        kwonly_defaults=None,
    )

    if len(tag_args) > 1:
        component_name = tag_args[1].token
        context_args = tag_args[2:]
        context_kwargs = tag_kwargs
    else:
        raise TemplateSyntaxError(
            f"Call the '{tag_name}' tag with a component name as the first parameter",
        )

    return component_name, context_args, context_kwargs
```

```htmldjango
{% component "blog" title="News" as component %}
  {% call component.header classes="text-lg" %}Head{% endcall %}
  {% for post in posts %}
    {% call component.posts post=post %}{{ post.title }}{% endcall %}
  {% endfor %}
{% endcomponent %}
{% component "button" %}{% endcomponent %}
```

```snapshot
✓ no diagnostics
```

## unclosed calls and orphaned closers are reported

`django_viewcomponent/__init__.py`:

```py
```

`django_viewcomponent/templatetags/__init__.py`:

```py
```

`django_viewcomponent/templatetags/viewcomponent_tags.py`:

```py
import django.template
from django.template.base import NodeList

register = django.template.Library()


@register.tag("call")
def do_call(parser, token):
    nodelist = parser.parse(parse_until=["endcall"])
    parser.delete_first_token()
    return CallNode(parser=parser, nodelist=nodelist)


@register.tag(name="component")
def do_component(parser, token):
    nodelist: NodeList = parser.parse(parse_until=["endcomponent"])
    parser.delete_first_token()
    return ComponentNode(nodelist)
```

```htmldjango
{% component "blog" as component %}
  {% call component.header %}Head
{% endcomponent %}
{% endcomponent %}
```

```snapshot
error[S100]: Unclosed 'call' tag
 --> test.html:2:3
  |
2 |   {% call component.header %}Head
  |   ^^^^^^^^^^^^^^^^^^^^^^^^^^^
error[S101]: 'endcomponent' has no matching 'component' block
 --> test.html:4:1
  |
4 | {% endcomponent %}
  | ^^^^^^^^^^^^^^^^^^
```
