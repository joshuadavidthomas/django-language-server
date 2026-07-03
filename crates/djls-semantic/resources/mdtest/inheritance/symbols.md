# Template symbols

## plain blocks

```htmldjango
{% block title %}Title{% endblock %}
{% block content %}Body{% endblock %}
```

```snapshot
extends: none
blocks:
  - title name@9..14 full@0..36
  - content name@46..53 full@37..74
partials:
  none
chain:
  ancestors: none
  end: root
```

## nested blocks

```htmldjango
{% block content %}
  {% block title %}Title{% endblock %}
{% endblock %}
```

```snapshot
extends: none
blocks:
  - content name@9..16 full@0..73
  - title name@31..36 full@22..58
partials:
  none
chain:
  ancestors: none
  end: root
```

## duplicate names

```htmldjango
{% block content %}One{% endblock %}
{% block content %}Two{% endblock %}
```

```snapshot
extends: none
blocks:
  - content name@9..16 full@0..36
  - content name@46..53 full@37..73
partials:
  none
chain:
  ancestors: none
  end: root
```

## nameless block

```htmldjango
{% block %}Body{% endblock %}
```

```snapshot
extends: none
blocks:
  none
partials:
  none
chain:
  ancestors: none
  end: root
```

## dynamic extends

```htmldjango
{% extends parent_template %}
{% block content %}Body{% endblock %}
```

```snapshot
extends: dynamic @11..26
blocks:
  - content name@39..46 full@30..67
partials:
  none
chain:
  ancestors: none
  end: dynamic @11..26
```

## extends not first

```htmldjango
{% include "before.html" %}
{% extends "first.html" %}
{% extends "second.html" %}
```

```snapshot
extends: literal "first.html" @40..50
blocks:
  none
partials:
  none
chain:
  ancestors: none
  end: unresolved "first.html"
```

# Template inheritance

## two-file chain

```htmldjango
{% extends "base.html" %}
{% block content %}Child{% endblock %}
```

`base.html`:

```htmldjango
{% block content %}Base{% endblock %}
```

```snapshot
extends: literal "base.html" @12..21
blocks:
  - content name@35..42 full@26..64
partials:
  none
chain:
  ancestors:
    - base.html
  end: root
```

## three-file chain

```htmldjango
{% extends "layout.html" %}
{% block content %}Child{% endblock %}
```

`layout.html`:

```htmldjango
{% extends "base.html" %}
{% block content %}Layout{% endblock %}
```

`base.html`:

```htmldjango
{% block content %}Base{% endblock %}
```

```snapshot
extends: literal "layout.html" @12..23
blocks:
  - content name@37..44 full@28..66
partials:
  none
chain:
  ancestors:
    - layout.html
    - base.html
  end: root
```

## unresolved parent

```htmldjango
{% extends "missing.html" %}
{% block content %}Child{% endblock %}
```

```snapshot
extends: literal "missing.html" @12..24
blocks:
  - content name@38..45 full@29..67
partials:
  none
chain:
  ancestors: none
  end: unresolved "missing.html"
```

## direct cycle

`a.html`:

```htmldjango
{% extends "b.html" %}
{% block content %}A{% endblock %}
```

`b.html`:

```htmldjango
{% extends "a.html" %}
{% block content %}B{% endblock %}
```

```snapshot
extends: literal "b.html" @12..18
blocks:
  - content name@32..39 full@23..57
partials:
  none
chain:
  ancestors:
    - b.html
  end: cycle
```

## self-cycle single origin

```htmldjango
{% extends "test.html" %}
{% block content %}Self{% endblock %}
```

```snapshot
extends: literal "test.html" @12..21
blocks:
  - content name@35..42 full@26..63
partials:
  none
chain:
  ancestors: none
  end: cycle
```

## multiple extends first wins

```htmldjango
{% extends "first.html" %}
{% extends "second.html" %}
{% block content %}Child{% endblock %}
```

`first.html`:

```htmldjango
{% block content %}First{% endblock %}
```

`second.html`:

```htmldjango
{% block content %}Second{% endblock %}
```

```snapshot
extends: literal "first.html" @12..22
blocks:
  - content name@64..71 full@55..93
partials:
  none
chain:
  ancestors:
    - first.html
  end: root
```
