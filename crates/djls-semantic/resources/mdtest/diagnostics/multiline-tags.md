# Multi-line tags

Django's default `django.template.base.tag_re` only matches a tag that closes on the line where it opens. Projects that compile `tag_re` with `re.DOTALL` while Django starts let tags span lines.

## default tag_re keeps a split tag as text

`settings.py`:

```py
INSTALLED_APPS = ['blog']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

`blog/__init__.py`:

```py
```

```htmldjango
{% if user
   and user.is_staff %}
  Staff
{% endif %}
```

```snapshot
error[S101]: 'endif' has no matching 'if' block
 --> test.html:4:1
  |
4 | {% endif %}
  | ^^^^^^^^^^^
```

## a DOTALL patch in an app lets tags span lines

`settings.py`:

```py
INSTALLED_APPS = ['blog']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

`blog/__init__.py`:

```py
from . import patches
```

`blog/patches.py`:

```py
import re

from django.template import base as template_base

template_base.tag_re = re.compile(template_base.tag_re.pattern, re.DOTALL)
```

```htmldjango
{% if user
   and user.is_staff %}
  Staff
{% endif %}
```

```snapshot
✓ no diagnostics
```

## a DOTALL patch in AppConfig.ready lets tags span lines

`settings.py`:

```py
INSTALLED_APPS = ['blog.apps.BlogConfig']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

`blog/__init__.py`:

```py
```

`blog/apps.py`:

```py
import re

from django.apps import AppConfig


class BlogConfig(AppConfig):
    name = "blog"

    def ready(self):
        from django.template import base

        base.tag_re = re.compile(base.tag_re.pattern, re.DOTALL)
```

```htmldjango
{% if user
   and user.is_staff %}
  Staff
{% endif %}
```

```snapshot
✓ no diagnostics
```

## an installed app outside the search paths keeps multi-line tags

The app could patch `tag_re`, so lexing keeps tags that span lines rather than reporting them as errors.

`settings.py`:

```py
INSTALLED_APPS = ['missing_app']
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': [], 'libraries': {}}}]
```

```htmldjango
{% if user
   and user.is_staff %}
  Staff
{% endif %}
```

```snapshot
✓ no diagnostics
```
