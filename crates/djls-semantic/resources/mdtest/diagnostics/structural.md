# Structural diagnostics

## if block is not closed

```htmldjango
{% if user %}
  Hello
```

```snapshot
error[S100]: Unclosed 'if' tag
 --> test.html:1:1
  |
1 | {% if user %}
  | ^^^^^^^^^^^^^
```

## else outside if

```htmldjango
{% else %}
```

```snapshot
error[S102]: 'else' must be inside an open 'if' or 'ifchanged' block
 --> test.html:1:1
  |
1 | {% else %}
  | ^^^^^^^^^^
```

## endif outside if

```htmldjango
{% endif %}
```

```snapshot
error[S101]: 'endif' has no matching 'if' block
 --> test.html:1:1
  |
1 | {% endif %}
  | ^^^^^^^^^^^
```

## closing block name mismatch

```htmldjango
{% block content %}
{% endblock sidebar %}
```

```snapshot
error[S103]: Closing block 'sidebar' does not match opening block 'content'
 --> test.html:2:1
  |
2 | {% endblock sidebar %}
  | ^^^^^^^^^^^^^^^^^^^^^^
```

## closing tag arguments

Django's `Parser.parse` matches a closing tag by its first word. A closing-tag argument is checked only when the opener's compile function inspects the closing token.

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['section_tags'], 'libraries': {}}}]
```

### checked when the compile function compares the closing token

`section_tags.py`:

```py
from django import template
register = template.Library()

@register.tag
def section(parser, token):
    bits = token.split_contents()
    name = bits[1]
    nodelist = parser.parse(("endsection",))
    closing = parser.next_token()
    accepted = ("endsection", f"endsection {name}")
    if closing.contents not in accepted:
        parser.invalid_block_tag(closing, "endsection", accepted)
    return template.Node()
```

```htmldjango
{% section intro %}{% endsection intro %}
{% section body %}{% endsection %}
{% section footer %}{% endsection sidebar %}
```

```snapshot
error[S103]: Closing block 'sidebar' does not match opening block 'footer'
 --> test.html:3:21
  |
3 | {% section footer %}{% endsection sidebar %}
  |                     ^^^^^^^^^^^^^^^^^^^^^^^^
```

### ignored when the compile function deletes the closing token

`section_tags.py`:

```py
from django import template
register = template.Library()

@register.tag
def section(parser, token):
    bits = token.split_contents()
    nodelist = parser.parse(("endsection",))
    parser.delete_first_token()
    return template.Node()
```

```htmldjango
{% section footer %}{% endsection sidebar %}
```

```snapshot
✓ no diagnostics
```
