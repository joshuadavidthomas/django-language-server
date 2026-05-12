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
error[S122]: The 'extends' tag must be the first tag in the template
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
error[S123]: The 'extends' tag can only appear once in a template
 --> test.html:2:1
  |
2 | {% extends "other.html" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
```
