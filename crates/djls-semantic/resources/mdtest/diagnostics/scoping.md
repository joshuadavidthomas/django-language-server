# Scoping diagnostics

## unknown tag

```htmldjango
{% completelymadetuptag %}
```

```snapshot
error[S108]: Unknown tag 'completelymadetuptag'
 --> test.html:1:1
  |
1 | {% completelymadetuptag %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^
```

## tag requires load when not configured as a builtin

```htmldjango
{% static "before.css" %}
```

```snapshot
error[S109]: Tag 'static' requires the 'static' tag library
 --> test.html:1:1
  |
1 | {% static "before.css" %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^
```

## tag is available from multiple unloaded libraries

```htmldjango
{% ambiguous_tag %}
```

```snapshot
error[S110]: Tag 'ambiguous_tag' is available from multiple libraries: 'alpha', 'beta'
 --> test.html:1:1
  |
1 | {% ambiguous_tag %}
  | ^^^^^^^^^^^^^^^^^^^
```

## unknown filter

```htmldjango
{{ value|completelymadetupfilter }}
```

```snapshot
error[S111]: Unknown filter 'completelymadetupfilter'
 --> test.html:1:10
  |
1 | {{ value|completelymadetupfilter }}
  |          ^^^^^^^^^^^^^^^^^^^^^^^
```

## filter requires load when not configured as a builtin

```htmldjango
{{ value|intcomma }}
```

```snapshot
error[S112]: Filter 'intcomma' requires the 'humanize' tag library
 --> test.html:1:10
  |
1 | {{ value|intcomma }}
  |          ^^^^^^^^
```

## filter is available from multiple unloaded libraries

```htmldjango
{{ value|ambiguous_filter }}
```

```snapshot
error[S113]: Filter 'ambiguous_filter' is available from multiple libraries: 'alpha', 'beta'
 --> test.html:1:10
  |
1 | {{ value|ambiguous_filter }}
  |          ^^^^^^^^^^^^^^^^
```

## tag library is unknown

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

## tag app is not installed

```htmldjango
{% widget_tag %}
```

```snapshot
error[S118]: Add 'example.widgets' to INSTALLED_APPS to use tag 'widget_tag'
 --> test.html:1:1
  |
1 | {% widget_tag %}
  | ^^^^^^^^^^^^^^^^
  |
  = note: load_name: widgets
```

## filter app is not installed

```htmldjango
{{ value|widget_filter }}
```

```snapshot
error[S119]: Add 'example.widgets' to INSTALLED_APPS to use filter 'widget_filter'
 --> test.html:1:10
  |
1 | {{ value|widget_filter }}
  |          ^^^^^^^^^^^^^
  |
  = note: load_name: widgets
```
