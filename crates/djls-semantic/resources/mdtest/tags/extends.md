# extends

## Valid

### accepts first tag in template

```htmldjango
{% extends "djls_app/base.html" %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects non-first tag

```htmldjango
{% load i18n %}
{% extends "base.html" %}
```

```snapshot
error[S122]: '{% extends %}' must be the first tag in the template
 --> test.html:2:1
  |
2 | {% extends "base.html" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects duplicate extends

```htmldjango
{% extends "base.html" %}
{% extends "other.html" %}
```

```snapshot
error[S123]: '{% extends %}' cannot appear more than once in the same template
 --> test.html:2:1
  |
2 | {% extends "other.html" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
```
