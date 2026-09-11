# Tag argument diagnostics

```toml
builtins = ["custom_tags"]
```

## tag requires an argument

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def one_arg_tag(value): pass
```

```htmldjango
{% one_arg_tag %}
```

```snapshot
error[S117]: 'one_arg_tag' did not receive value(s) for the argument(s): 'value'
 --> test.html:1:1
  |
1 | {% one_arg_tag %}
  | ^^^^^^^^^^^^^^^^^
```

## tag accepts exactly one argument

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def one_arg_tag(value): pass
```

```htmldjango
{% one_arg_tag first second %}
```

```snapshot
error[S117]: 'one_arg_tag' received too many positional arguments
 --> test.html:1:1
  |
1 | {% one_arg_tag first second %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
