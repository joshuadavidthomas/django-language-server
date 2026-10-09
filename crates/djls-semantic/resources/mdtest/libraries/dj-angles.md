# dj-angles

[dj-angles](https://github.com/adamghill/dj-angles) adds a template loader that rewrites HTML elements such as `<dj-include>` and `<dj-block>`, and attributes such as `dj-if`, into Django tags before Django parses the Template. It also registers `call`, `model`, `template`, and `view` tags. DJLS analyzes the source file, so it treats the HTML forms as text and validates only the Django syntax written directly; tags the loader generates do not take part in structure or inheritance analysis. The Python below copies the registration module and compile functions from dj-angles 0.27.0 and omits Node classes and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['dj_angles']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'loaders': ['dj_angles.template_loader.Loader', 'django.template.loaders.filesystem.Loader', 'django.template.loaders.app_directories.Loader'], 'builtins': ['dj_angles.templatetags.dj_angles'], 'libraries': {}}}]
```

## HTML forms and registered tags validate

`dj_angles/__init__.py`:

```py
```

`dj_angles/templatetags/__init__.py`:

```py
```

`dj_angles/templatetags/dj_angles.py`:

```py
from datetime import date, datetime, time

from django import template

from dj_angles.templatetags.call import do_call
from dj_angles.templatetags.model import do_model
from dj_angles.templatetags.template import do_template
from dj_angles.templatetags.view import do_view

register = template.Library()


@register.filter(name="dateformat")
def dateformat(value: datetime | date | time, date_format: str):
    return value.strftime(date_format)


@register.inclusion_tag("dj_angles/scripts.html")
def dj_angles_scripts(*, ajax_form: bool = True):
    return {"ajax_form": ajax_form}


# Register custom template tags
register.tag("call", do_call)
register.tag("model", do_model)
register.tag("template", do_template)
register.tag("view", do_view)
```

`dj_angles/templatetags/call.py`:

```py
from django.template import TemplateSyntaxError


def get_tag_args(token, tag_name: str, min_args: int = 1):
    contents = list(yield_tokens(token.contents, " ", handle_quotes=True, handle_parenthesis=True))

    # The first content is always the name of the tag, so pop it off
    contents.pop(0)

    if len(contents) < min_args:
        raise TemplateSyntaxError(f"{tag_name} template tag requires at least {min_args} argument")

    parsed_function = ParsedFunction(contents[0])
    context_variable_name = None
    template_tag_arguments = contents[1:]
    args = []

    for idx, arg in enumerate(template_tag_arguments):
        if arg == "as":
            if len(template_tag_arguments) < idx + 2:
                raise TemplateSyntaxError("Missing variable name after 'as'")
            elif len(template_tag_arguments) > idx + 2:
                raise TemplateSyntaxError("Too many arguments after 'as'")

            context_variable_name = template_tag_arguments[idx + 1]
            break
        else:
            args.append(arg)

    return (parsed_function, args, context_variable_name)


def do_call(parser, token):
    (parsed_function, _, context_variable_name) = get_tag_args(token, "call")

    return CallNode(parsed_function, context_variable_name)
```

`dj_angles/templatetags/model.py`:

```py
from dj_angles.templatetags.call import do_call


def do_model(parser, token):
    call_node = do_call(parser, token)
    return ModelNode(call_node.parsed_function, call_node.context_variable_name)
```

`dj_angles/templatetags/template.py`:

```py
def do_template(parser, token):
    # Get the nodelist up until the endtemplate tag
    nodelist = parser.parse(("endtemplate",))
    parser.delete_first_token()

    return TemplateNode(nodelist, token)
```

`dj_angles/templatetags/view.py`:

```py
from dj_angles.templatetags.call import get_tag_args


def do_view(parser, token):
    (parsed_function, args, context_variable_name) = get_tag_args(token, "view")

    return ViewNode(parsed_function, args, context_variable_name)
```

```htmldjango
<dj-extends 'base.html' />
<dj-block name='content'>
  <dj-include 'partials/header.html' />
  <dj-partial name='row'>{{ row.name }}</dj-partial>
  <ul>
    <li dj-for="book in books">{{ book.title }} {{ book.published|dateformat:"%Y" }}</li>
    <li dj-empty>No books</li>
  </ul>
  <p dj-if="user.is_authenticated">Hello</p>
  {% call slugify('Hello World') as slug %}
  {% model Book.objects.filter(published=True) as books %}
  {% view books.views.detail(book.pk) %}
  {% template books.templates.card %}{{ book.title }}{% endtemplate %}
</dj-block>
{% dj_angles_scripts ajax_form=False %}
```

```snapshot
✓ no diagnostics
```

## registered tag structure and filter arity are checked

`dj_angles/__init__.py`:

```py
```

`dj_angles/templatetags/__init__.py`:

```py
```

`dj_angles/templatetags/dj_angles.py`:

```py
from datetime import date, datetime, time

from django import template

from dj_angles.templatetags.template import do_template

register = template.Library()


@register.filter(name="dateformat")
def dateformat(value: datetime | date | time, date_format: str):
    return value.strftime(date_format)


register.tag("template", do_template)
```

`dj_angles/templatetags/template.py`:

```py
def do_template(parser, token):
    # Get the nodelist up until the endtemplate tag
    nodelist = parser.parse(("endtemplate",))
    parser.delete_first_token()

    return TemplateNode(nodelist, token)
```

```htmldjango
{{ book.published|dateformat }}
{% template books.templates.card %}
```

```snapshot
error[S115]: Filter 'dateformat' requires an argument
 --> test.html:1:19
  |
1 | {{ book.published|dateformat }}
  |                   ^^^^^^^^^^
error[S100]: Unclosed 'template' tag
 --> test.html:2:1
  |
2 | {% template books.templates.card %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
