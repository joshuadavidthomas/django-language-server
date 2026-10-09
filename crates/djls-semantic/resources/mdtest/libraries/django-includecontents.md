# django-includecontents

[django-includecontents](https://github.com/SmileyChris/django-includecontents) provides `{% includecontents %}`, `{% attrs %}`, and `{% wrapif %}`. Its documented setup replaces the `BACKEND` with `includecontents.django.DjangoTemplates`, a `DjangoTemplates` subclass whose engine preloads the library and whose lexer turns `<include:card>` elements into tags. The Python below copies the compile functions from django-includecontents 4.0.2 and omits Node classes, rendering, and argument parsing.

## Templates under the custom backend validate without false positives

DJLS treats a `BACKEND` that Django does not ship as one that may preload more builtins. It still applies Django's builtins and keeps unknown names inconclusive.

`settings.py`:

```py
INSTALLED_APPS = ['includecontents']
TEMPLATES = [{'BACKEND': 'includecontents.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {}}]
```

`includecontents/__init__.py`:

```py
```

`includecontents/templatetags/__init__.py`:

```py
```

`includecontents/templatetags/includecontents.py`:

```py
from django import template
from django.template.loader_tags import do_include

register = template.Library()


@register.filter(name="not")
def not_filter(value):
    return not value


@register.tag
def includecontents(parser, token):
    bits = token.split_contents()
    token_name = bits[0]
    nodelist, named_nodelists = get_contents_nodelists(parser, token_name)
    include_node = do_include(parser, token)
    return IncludeContentsNode(token_name=token_name, include_node=include_node, nodelist=nodelist)


def get_contents_nodelists(parser, token_name):
    if token_name.endswith("/>"):
        return NodeList(), {}
    end_tag = (
        f"</{token_name[1:]}" if token_name.startswith("<") else f"end{token_name}"
    )
    named_nodelists = {}
    while parser.tokens:
        token = parser.next_token()
        bits = token.split_contents()
        tag_name = bits[0]
        if tag_name == "contents":
            named_nodelists[bits[1]] = parser.parse((f"end{tag_name}",))
            parser.delete_first_token()
            continue
        elif tag_name == end_tag:
            nodelist = parser.parse((end_tag,))
            parser.delete_first_token()
            return nodelist, named_nodelists
    parser.unclosed_block_tag((end_tag,))


@register.tag
def attrs(parser, token):
    bits = smart_split(token.contents)
    tag_name = next(bits)
    if "." in tag_name:
        sub_key = tag_name.split(".", 1)[1]
    else:
        sub_key = None
    return AttrsNode(sub_key, {})
```

`components/card.html`:

```htmldjango
{# props title, dismissable=False #}
<div {% attrs class="card" %}>
  {% if contents.title %}<h3>{{ contents.title }}</h3>{% endif %}
  <section {% attrs.inner class='& inner' %}>{{ contents }}</section>
  {% if not dismissable %}{{ dismissable|not }}{% endif %}
</div>
```

```htmldjango
<include:card title="Hello" dismissable>
  <content:title>Greeting</content:title>
  Body
</include:card>
{% includecontents "components/card.html" with dismissable=False %}
  {% contents title %}Named area{% endcontents %}
  Default contents
{% endincludecontents %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

### contents areas and the closer under the stock backend

With Django's own backend, the tags are available from `builtins`. `includecontents` walks the token stream itself: it consumes `{% contents %}` areas and finds its closer in a helper, so neither `contents` nor `endincludecontents` is a registered tag or an extracted closer.

`settings.py`:

```py
INSTALLED_APPS = ['includecontents']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['includecontents.templatetags.includecontents'], 'libraries': {}}}]
```

`includecontents/__init__.py`:

```py
```

`includecontents/templatetags/__init__.py`:

```py
```

`includecontents/templatetags/includecontents.py`:

```py
from django import template
from django.template.loader_tags import do_include

register = template.Library()


@register.tag
def includecontents(parser, token):
    bits = token.split_contents()
    token_name = bits[0]
    nodelist, named_nodelists = get_contents_nodelists(parser, token_name)
    include_node = do_include(parser, token)
    return IncludeContentsNode(token_name=token_name, include_node=include_node, nodelist=nodelist)


def get_contents_nodelists(parser, token_name):
    if token_name.endswith("/>"):
        return NodeList(), {}
    end_tag = (
        f"</{token_name[1:]}" if token_name.startswith("<") else f"end{token_name}"
    )
    named_nodelists = {}
    while parser.tokens:
        token = parser.next_token()
        bits = token.split_contents()
        tag_name = bits[0]
        if tag_name == "contents":
            named_nodelists[bits[1]] = parser.parse((f"end{tag_name}",))
            parser.delete_first_token()
            continue
        elif tag_name == end_tag:
            nodelist = parser.parse((end_tag,))
            parser.delete_first_token()
            return nodelist, named_nodelists
    parser.unclosed_block_tag((end_tag,))
```

```htmldjango
{% includecontents "components/card.html" %}
  {% contents title %}Named area{% endcontents %}
  Default contents
{% endincludecontents %}
```

```snapshot
error[S108]: Unknown tag 'contents'
 --> test.html:2:3
  |
2 |   {% contents title %}Named area{% endcontents %}
  |   ^^^^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endcontents'
 --> test.html:2:33
  |
2 |   {% contents title %}Named area{% endcontents %}
  |                                 ^^^^^^^^^^^^^^^^^
error[S108]: Unknown tag 'endincludecontents'
 --> test.html:4:1
  |
4 | {% endincludecontents %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^
```
