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
block queries:
  parent blocks:
    - title -> none
    - content -> none
  inherited blocks:
    none
  overrides:
    - title: none
    - content: none
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
block queries:
  parent blocks:
    - content -> none
    - title -> none
  inherited blocks:
    none
  overrides:
    - content: none
    - title: none
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
block queries:
  parent blocks:
    - content -> none
    - content -> none
  inherited blocks:
    none
  overrides:
    - content: none
    - content: none
```

## duplicate extends winning edge includes first parent

```htmldjango
{% block content %}Parent{% endblock %}
```

`child.html`:

```htmldjango
{% extends "test.html" %}
{% extends "second.html" %}
{% block content %}Child{% endblock %}
```

`second.html`:

```htmldjango
{% block content %}Second{% endblock %}
```

```snapshot
extends: none
blocks:
  - content name@9..16 full@0..39
partials:
  none
chain:
  ancestors: none
  end: root
block queries:
  parent blocks:
    - content -> none
  inherited blocks:
    none
  overrides:
    - content:
      - child.html name@63..70 full@54..92
```

## duplicate extends winning edge excludes second parent

```htmldjango
{% block content %}Second{% endblock %}
```

`child.html`:

```htmldjango
{% extends "first.html" %}
{% extends "test.html" %}
{% block content %}Child{% endblock %}
```

`first.html`:

```htmldjango
{% block content %}First{% endblock %}
```

```snapshot
extends: none
blocks:
  - content name@9..16 full@0..39
partials:
  none
chain:
  ancestors: none
  end: root
block queries:
  parent blocks:
    - content -> none
  inherited blocks:
    none
  overrides:
    - content: none
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
block queries:
  parent blocks:
    none
  inherited blocks:
    none
  overrides:
    none
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
block queries:
  parent blocks:
    - content -> none
  inherited blocks:
    none
  overrides:
    - content: none
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
block queries:
  parent blocks:
    none
  inherited blocks:
    none
  overrides:
    none
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
block queries:
  parent blocks:
    - content -> base.html name@9..16 full@0..37
  inherited blocks:
    - content -> base.html name@9..16 full@0..37
  overrides:
    - content: none
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
block queries:
  parent blocks:
    - content -> layout.html name@35..42 full@26..65
  inherited blocks:
    - content -> layout.html name@35..42 full@26..65
  overrides:
    - content: none
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
block queries:
  parent blocks:
    - content -> none
  inherited blocks:
    none
  overrides:
    - content: none
```

## direct cycle

```htmldjango
{% extends "b.html" %}
{% block content %}A{% endblock %}
```

`b.html`:

```htmldjango
{% extends "test.html" %}
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
block queries:
  parent blocks:
    - content -> b.html name@35..42 full@26..60
  inherited blocks:
    - content -> b.html name@35..42 full@26..60
  overrides:
    - content:
      - b.html name@35..42 full@26..60
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
block queries:
  parent blocks:
    - content -> none
  inherited blocks:
    none
  overrides:
    - content: none
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
block queries:
  parent blocks:
    - content -> first.html name@9..16 full@0..38
  inherited blocks:
    - content -> first.html name@9..16 full@0..38
  overrides:
    - content: none
```

# Block queries

## override shadowing

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
block queries:
  parent blocks:
    - content -> layout.html name@35..42 full@26..65
  inherited blocks:
    - content -> layout.html name@35..42 full@26..65
  overrides:
    - content: none
```

## nested-block override

```htmldjango
{% extends "base.html" %}
{% block outer %}
  {% block inner %}Child{% endblock %}
{% endblock %}
```

`base.html`:

```htmldjango
{% block outer %}
  {% block inner %}Base{% endblock %}
{% endblock %}
```

```snapshot
extends: literal "base.html" @12..21
blocks:
  - outer name@35..40 full@26..97
  - inner name@55..60 full@46..82
partials:
  none
chain:
  ancestors:
    - base.html
  end: root
block queries:
  parent blocks:
    - outer -> base.html name@9..14 full@0..70
    - inner -> base.html name@29..34 full@20..55
  inherited blocks:
    - outer -> base.html name@9..14 full@0..70
    - inner -> base.html name@29..34 full@20..55
  overrides:
    - outer: none
    - inner: none
```

## block only in grandparent

```htmldjango
{% extends "layout.html" %}
{% block footer %}Child{% endblock %}
```

`layout.html`:

```htmldjango
{% extends "base.html" %}
{% block content %}Layout{% endblock %}
```

`base.html`:

```htmldjango
{% block footer %}Base{% endblock %}
```

```snapshot
extends: literal "layout.html" @12..23
blocks:
  - footer name@37..43 full@28..65
partials:
  none
chain:
  ancestors:
    - layout.html
    - base.html
  end: root
block queries:
  parent blocks:
    - footer -> base.html name@9..15 full@0..36
  inherited blocks:
    - content -> layout.html name@35..42 full@26..65
    - footer -> base.html name@9..15 full@0..36
  overrides:
    - footer: none
```

## override of nonexistent name

```htmldjango
{% extends "base.html" %}
{% block missing %}Child{% endblock %}
```

`base.html`:

```htmldjango
{% block content %}Base{% endblock %}
```

```snapshot
extends: literal "base.html" @12..21
blocks:
  - missing name@35..42 full@26..64
partials:
  none
chain:
  ancestors:
    - base.html
  end: root
block queries:
  parent blocks:
    - missing -> none
  inherited blocks:
    - content -> base.html name@9..16 full@0..37
  overrides:
    - missing: none
```

## blocks inside includes excluded

```htmldjango
{% extends "base.html" %}
{% include "card.html" %}
{% block content %}Child{% endblock %}
```

`base.html`:

```htmldjango
{% block content %}Base{% endblock %}
```

`card.html`:

```htmldjango
{% block card %}Included{% endblock %}
```

```snapshot
extends: literal "base.html" @12..21
blocks:
  - content name@61..68 full@52..90
partials:
  none
chain:
  ancestors:
    - base.html
  end: root
block queries:
  parent blocks:
    - content -> base.html name@9..16 full@0..37
  inherited blocks:
    - content -> base.html name@9..16 full@0..37
  overrides:
    - content: none
```
