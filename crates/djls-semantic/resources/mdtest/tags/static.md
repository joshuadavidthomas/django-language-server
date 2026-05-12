# static

## Valid

### resolves a literal static path after load

```htmldjango
{% load static %}

{% static "css/style.css" %}
```

```snapshot
✓ no diagnostics
```

### assigns static path to a variable

```htmldjango
{% load static %}

{% static "css/style.css" as css_url %}
```

```snapshot
✓ no diagnostics
```

### accepts a variable static path

```htmldjango
{% load static %}

{% static path_var %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires load before use

```htmldjango
{% static "before.css" %}
```

```snapshot
error[S109]: Tag 'static' requires {% load static %}
 --> test.html:1:1
  |
1 | {% static "before.css" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```
