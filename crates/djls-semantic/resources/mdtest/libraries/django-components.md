# django-components

[django-components](https://github.com/django-components/django-components) defines each tag as a `BaseNode` subclass and registers it with `ComponentNode.register(register)`, a classmethod that calls `library.tag(cls.tag, cls.parse)`. Its `ComponentRegistry` also adds tags to the same library at runtime, with names chosen by the configured tag formatter. DJLS cannot enumerate those registrations, so it treats `component_tags` as an open Template Library. The Python below copies the registration module and Node attributes from django-components 0.152.0 and omits parsing and rendering.

`settings.py`:

```py
INSTALLED_APPS = ['django_components']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['django_components.templatetags.component_tags'], 'libraries': {}}}]
```

## component templates validate without false positives

The documented setup lists `component_tags` in `builtins`. Self-closing tags, literal lists and dicts, spread arguments, `attrs:` keys, and flags all pass through.

`django_components/__init__.py`:

```py
```

`django_components/node.py`:

```py
from django.template import Node
from django.template.library import Library


class BaseNode(Node):
    tag = ""
    end_tag = None
    allowed_flags = None

    @classmethod
    def parse(cls, parser, token, **kwargs):
        tag = parse_template_tag(cls.tag, cls.end_tag, parser_config, parser, token)
        body, contents = tag.parse_body()
        return cls(nodelist=body, params=tag.params, flags=tag.flags, contents=contents, **kwargs)

    @classmethod
    def register(cls, library: Library) -> None:
        library.tag(cls.tag, cls.parse)
```

`django_components/attributes.py`:

```py
from django_components.node import BaseNode


class HtmlAttrsNode(BaseNode):
    tag = "html_attrs"
    end_tag = None  # inline-only
    allowed_flags = ()


class AttrsNode(BaseNode):
    tag = "attrs"
    end_tag = None
    allowed_flags = ()
```

`django_components/component.py`:

```py
from django_components.node import BaseNode

COMP_ONLY_FLAG = "only"


class ComponentNode(BaseNode):
    tag = "component"
    end_tag = "endcomponent"
    allowed_flags = (COMP_ONLY_FLAG,)
```

`django_components/dependencies.py`:

```py
from django_components.node import BaseNode


class ComponentCssDependenciesNode(BaseNode):
    tag = "component_css_dependencies"
    end_tag = None  # inline-only
    allowed_flags = ()


class ComponentJsDependenciesNode(BaseNode):
    tag = "component_js_dependencies"
    end_tag = None  # inline-only
    allowed_flags = ()
```

`django_components/provide.py`:

```py
from django_components.node import BaseNode


class ProvideNode(BaseNode):
    tag = "provide"
    end_tag = "endprovide"
    allowed_flags = ()
```

`django_components/slots.py`:

```py
from django_components.node import BaseNode

SLOT_DEFAULT_FLAG = "default"
SLOT_REQUIRED_FLAG = "required"


class SlotNode(BaseNode):
    tag = "slot"
    end_tag = "endslot"
    allowed_flags = (SLOT_DEFAULT_FLAG, SLOT_REQUIRED_FLAG)


class FillNode(BaseNode):
    tag = "fill"
    end_tag = "endfill"
    allowed_flags = ()
```

`django_components/cache_tag.py`:

```py
from django.templatetags.cache import do_cache
from django.templatetags.cache import register as django_cache_register


def do_djc_cache(parser, token):
    """Identical to Django's {% cache %} parser, but produces a DjcCacheNode."""
    node = do_cache(parser, token)
    node.__class__ = DjcCacheNode
    return node


django_cache_register.tag("cache", do_djc_cache)
```

`django_components/templatetags/__init__.py`:

```py
```

`django_components/templatetags/component_tags.py`:

```py
import django.template

from django_components.attributes import AttrsNode, HtmlAttrsNode
from django_components.cache_tag import do_djc_cache
from django_components.component import ComponentNode
from django_components.dependencies import ComponentCssDependenciesNode, ComponentJsDependenciesNode
from django_components.provide import ProvideNode
from django_components.slots import FillNode, SlotNode

register = django.template.Library()

AttrsNode.register(register)
ComponentNode.register(register)
ComponentCssDependenciesNode.register(register)
ComponentJsDependenciesNode.register(register)
FillNode.register(register)
HtmlAttrsNode.register(register)
ProvideNode.register(register)
SlotNode.register(register)

register.tag("cache", do_djc_cache)

attrs = AttrsNode.parse
component = ComponentNode.parse
component_css_dependencies = ComponentCssDependenciesNode.parse
component_js_dependencies = ComponentJsDependenciesNode.parse
fill = FillNode.parse
html_attrs = HtmlAttrsNode.parse
provide = ProvideNode.parse
slot = SlotNode.parse
```

`calendar/calendar.html`:

```htmldjango
{% component_css_dependencies %}
{% component "calendar" date="2015-06-19" / %}
{% component "calendar" date=date only / %}
{% component "card" title="Hi" attrs:class="p-4" ...extra_attrs %}
  {% fill "header" %}Header{% endfill %}
  {% fill name="body" data="data" fallback="fallback" %}{{ data.x }}{{ fallback }}{% endfill %}
  {% fill "footer" / %}
{% endcomponent %}
{% component "table" items=[1, 2, 3] options={"a": 1, "b": "x"} label="{{ user.name }}"|upper %}{% endcomponent %}
{% provide "theme" color="red" %}
  {% component "child" / %}
{% endprovide %}
<div {% html_attrs attrs class="pa-4" class=extra_class data-id=123 %}></div>
<header>{% slot "header" default %}Default{% endslot %}</header>
<main>{% slot "body" required %}{% endslot %}</main>
{% slot "footer" / %}
{% component_js_dependencies %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

### the open builtin hides every unknown name and orphaned closer

Because `component_tags` is a builtin whose inventory is open, any unrecognized name might come from it. DJLS therefore reports neither unknown tags and filters nor orphaned closers in any Template.

`django_components/__init__.py`:

```py
```

`django_components/node.py`:

```py
from django.template import Node
from django.template.library import Library


class BaseNode(Node):
    tag = ""
    end_tag = None

    @classmethod
    def register(cls, library: Library) -> None:
        library.tag(cls.tag, cls.parse)
```

`django_components/component.py`:

```py
from django_components.node import BaseNode


class ComponentNode(BaseNode):
    tag = "component"
    end_tag = "endcomponent"
```

`django_components/templatetags/__init__.py`:

```py
```

`django_components/templatetags/component_tags.py`:

```py
import django.template

from django_components.component import ComponentNode

register = django.template.Library()

ComponentNode.register(register)
```

```htmldjango
{% bogus %}{% endif %}{{ value|missing_filter }}
```

```snapshot
✓ no diagnostics
```

### installing the app without the builtin still hides unknown names

When `component_tags` is only loadable, structure checks work again. A Template that never loads it still gets no unknown-name reports: the name might be an unloaded tag from the open library, and DJLS reports neither "unknown" nor "unloaded" when it cannot tell which. Django rejects the name either way.

`settings.py`:

```py
INSTALLED_APPS = ['django_components']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

`django_components/__init__.py`:

```py
```

`django_components/node.py`:

```py
from django.template import Node
from django.template.library import Library


class BaseNode(Node):
    tag = ""
    end_tag = None

    @classmethod
    def register(cls, library: Library) -> None:
        library.tag(cls.tag, cls.parse)
```

`django_components/component.py`:

```py
from django_components.node import BaseNode


class ComponentNode(BaseNode):
    tag = "component"
    end_tag = "endcomponent"
```

`django_components/templatetags/__init__.py`:

```py
```

`django_components/templatetags/component_tags.py`:

```py
import django.template

from django_components.component import ComponentNode

register = django.template.Library()

ComponentNode.register(register)
```

```htmldjango
{% bogus %}{% endif %}{{ value|missing_filter }}
```

```snapshot
error[S101]: 'endif' has no matching 'if' block
 --> test.html:1:12
  |
1 | {% bogus %}{% endif %}{{ value|missing_filter }}
  |            ^^^^^^^^^^^
```
