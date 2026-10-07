# django-unicorn

[django-unicorn](https://github.com/django-commons/django-unicorn) renders components through one `{% unicorn %}` tag. The component's own Template is ordinary Django template syntax, and its behavior lives in `unicorn:*` HTML attributes. The Python below copies the registrations and compile function from django-unicorn 0.67.0 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_unicorn']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

## component tags and component Templates validate

`django_unicorn/__init__.py`:

```py
```

`django_unicorn/templatetags/__init__.py`:

```py
```

`django_unicorn/templatetags/unicorn.py`:

```py
from django import template

register = template.Library()


MINIMUM_ARGUMENT_COUNT = 2


@register.inclusion_tag("unicorn/scripts.html")
def unicorn_scripts():
    return {}


@register.inclusion_tag("unicorn/errors.html", takes_context=True)
def unicorn_errors(context):
    return {"unicorn": {"errors": context.get("unicorn", {}).get("errors", {})}}


def unicorn(parser, token):
    contents = token.split_contents()

    if len(contents) < MINIMUM_ARGUMENT_COUNT:
        first_arg = token.contents.split()[0]
        raise template.TemplateSyntaxError(f"{first_arg} tag requires at least a single argument")

    component_name = parser.compile_filter(contents[1])

    args = []
    kwargs = {}
    unparseable_kwargs = {}

    for arg in contents[2:]:
        try:
            parsed_kwarg = parse_kwarg(arg, raise_if_unparseable=True)
            kwargs.update(parsed_kwarg)
        except InvalidKwargError:
            if not kwargs:
                args.append(arg)
        except ValueError:
            parsed_kwarg = parse_kwarg(arg, raise_if_unparseable=False)
            unparseable_kwargs.update(parsed_kwarg)

    return UnicornNode(component_name, args, kwargs, unparseable_kwargs)


register.tag("unicorn", unicorn)
```

`unicorn/todo.html`:

```htmldjango
<div>
  <form unicorn:submit.prevent="add">
    <input type="text" unicorn:model.defer="task" placeholder="New task" />
  </form>
  {% for task in tasks %}
    <li unicorn:key="{{ task.pk }}">{{ task.title }}</li>
  {% empty %}
    <p>No tasks</p>
  {% endfor %}
  <button unicorn:click="clear_tasks" {% if not tasks %}disabled{% endif %}>Clear</button>
</div>
```

```htmldjango
{% load unicorn %}
<head>{% unicorn_scripts %}</head>
<body>
  {% csrf_token %}
  {% unicorn 'todo' %}
  {% unicorn 'counter' count=3 parent=view key='counter-1' %}
  {% unicorn_errors %}
</body>
```

```snapshot
✓ no diagnostics
```

## the component name is required

`django_unicorn/__init__.py`:

```py
```

`django_unicorn/templatetags/__init__.py`:

```py
```

`django_unicorn/templatetags/unicorn.py`:

```py
from django import template

register = template.Library()


MINIMUM_ARGUMENT_COUNT = 2


def unicorn(parser, token):
    contents = token.split_contents()

    if len(contents) < MINIMUM_ARGUMENT_COUNT:
        first_arg = token.contents.split()[0]
        raise template.TemplateSyntaxError(f"{first_arg} tag requires at least a single argument")

    component_name = parser.compile_filter(contents[1])
    return UnicornNode(component_name, [], {}, {})


register.tag("unicorn", unicorn)
```

```htmldjango
{% load unicorn %}
{% unicorn %}
```

```snapshot
error[S117]: Tag 'unicorn' requires at least 1 argument
 --> test.html:2:1
  |
2 | {% unicorn %}
  | ^^^^^^^^^^^^^
```
