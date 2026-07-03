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
```
