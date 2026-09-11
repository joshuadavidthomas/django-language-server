# Mixed diagnostics

## expression and filter arity errors

```htmldjango
{% if and x %}bad expr{% endif %}
{{ value|truncatewords }}
{{ value|title:"bad" }}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}bad expr{% endif %}
  | ^^^^^^^^^^^^^^
error[S115]: Filter 'truncatewords' requires an argument
 --> test.html:2:10
  |
2 | {{ value|truncatewords }}
  |          ^^^^^^^^^^^^^
error[S116]: Filter 'title' does not accept an argument
 --> test.html:3:10
  |
3 | {{ value|title:"bad" }}
  |          ^^^^^^^^^^^
```

## expression and unloaded tag errors

```htmldjango
{% if or x %}bad{% endif %}
{% trans "hello" %}
```

```snapshot
error[S114]: Not expecting 'or' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if or x %}bad{% endif %}
  | ^^^^^^^^^^^^^
error[S109]: Tag 'trans' requires the 'i18n' tag library
 --> test.html:2:1
  |
2 | {% trans "hello" %}
  | ^^^^^^^^^^^^^^^^^^^
```

## loaded tags stay valid beside a filter error

```htmldjango
{% load i18n %}
{% trans "hello" %}
{{ value|truncatewords }}
```

```snapshot
error[S115]: Filter 'truncatewords' requires an argument
 --> test.html:3:10
  |
3 | {{ value|truncatewords }}
  |          ^^^^^^^^^^^^^
```

## mixed rendered diagnostics

```htmldjango
{% if and x %}oops{% endif %}
{{ name|title:"arg" }}
{{ text|truncatewords }}
{% trans "hello" %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}oops{% endif %}
  | ^^^^^^^^^^^^^^
error[S116]: Filter 'title' does not accept an argument
 --> test.html:2:9
  |
2 | {{ name|title:"arg" }}
  |         ^^^^^^^^^^^
error[S115]: Filter 'truncatewords' requires an argument
 --> test.html:3:9
  |
3 | {{ text|truncatewords }}
  |         ^^^^^^^^^^^^^
error[S109]: Tag 'trans' requires the 'i18n' tag library
 --> test.html:4:1
  |
4 | {% trans "hello" %}
  | ^^^^^^^^^^^^^^^^^^^
```

## clean template

```htmldjango
{% if user.is_authenticated %}
  <h1>{{ user.name|title }}</h1>
  {{ user.joined|date:"Y-m-d" }}
{% endif %}
```

```snapshot
✓ no diagnostics
```

## complex valid template

```htmldjango
{% load i18n %}
{% if user.is_staff and not user.is_superuser %}
  <p>{{ greeting|default:"Hello" }}</p>
  {% for item in items %}
    <li>{{ item.name|title }} - {{ item.date|date }}</li>
  {% endfor %}
  {% trans "Welcome" %}
{% endif %}
{% verbatim %}
  {{ raw_template_syntax }}
{% endverbatim %}
```

```snapshot
✓ no diagnostics
```

## multiple rendered error types

```htmldjango
{{ value|title:"unwanted" }}
{% if == broken %}bad{% endif %}
{{ text|lower:"arg" }}
{% comment %}{% if and %}{% endcomment %}
{{ result|truncatewords }}
```

```snapshot
error[S116]: Filter 'title' does not accept an argument
 --> test.html:1:10
  |
1 | {{ value|title:"unwanted" }}
  |          ^^^^^^^^^^^^^^^^
error[S114]: Not expecting '==' in this position in if tag.
 --> test.html:2:1
  |
2 | {% if == broken %}bad{% endif %}
  | ^^^^^^^^^^^^^^^^^^
error[S116]: Filter 'lower' does not accept an argument
 --> test.html:3:9
  |
3 | {{ text|lower:"arg" }}
  |         ^^^^^^^^^^^
error[S115]: Filter 'truncatewords' requires an argument
 --> test.html:5:11
  |
5 | {{ result|truncatewords }}
  |           ^^^^^^^^^^^^^
```
