# load

## Valid

### loads one library

```htmldjango
{% load static %}
```

```snapshot
✓ no diagnostics
```

### loads multiple libraries

```htmldjango
{% load static i18n %}
```

```snapshot
✓ no diagnostics
```

### imports a symbol from a library

```htmldjango
{% load trans from i18n %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### rejects selective import from unknown library

```htmldjango
{% load static from staticfiles %}
```

```snapshot
error[S120]: Unknown template tag library 'staticfiles'
 --> test.html:1:1
  |
1 | {% load static from staticfiles %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

### rejects unknown library

```htmldjango
{% load nonexistent_library %}
```

```snapshot
error[S120]: Unknown template tag library 'nonexistent_library'
 --> test.html:1:1
  |
1 | {% load nonexistent_library %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
