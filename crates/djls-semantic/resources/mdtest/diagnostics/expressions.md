# Expression diagnostics

Django compiles some tag arguments as template expressions while it parses a template. `if` and `elif` parse a boolean condition whose operands are filter expressions. Other tags pass individual arguments to `parser.compile_filter()`. DJLS reports the `TemplateSyntaxError` Django would raise.

## if tag

### starts with infix operator

```htmldjango
{% if and x %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:1
  |
1 | {% if and x %}{% endif %}
  | ^^^^^^^^^^^^^^
```

### ends after infix operator

```htmldjango
{% if x or %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if x or %}{% endif %}
  | ^^^^^^^^^^^^^
```

### contains unused token

```htmldjango
{% if x y %}{% endif %}
```

```snapshot
error[S114]: Unused 'y' at end of if expression.
 --> test.html:1:1
  |
1 | {% if x y %}{% endif %}
  | ^^^^^^^^^^^^
```

### has no condition

```htmldjango
{% if %}{% endif %}
```

```snapshot
error[S114]: Unexpected end of expression in if tag.
 --> test.html:1:1
  |
1 | {% if %}{% endif %}
  | ^^^^^^^^
```

### elif starts with infix operator

```htmldjango
{% if x %}{% elif and y %}{% endif %}
```

```snapshot
error[S114]: Not expecting 'and' in this position in if tag.
 --> test.html:1:11
  |
1 | {% if x %}{% elif and y %}{% endif %}
  |           ^^^^^^^^^^^^^^^^
```

### operand is not a valid filter expression

```htmldjango
{% if user| %}{% endif %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'user|'
 --> test.html:1:7
  |
1 | {% if user| %}{% endif %}
  |       ^^^^^
```

### elif operand starts with an underscore

```htmldjango
{% if x %}{% elif _secret %}{% endif %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_secret'
 --> test.html:1:19
  |
1 | {% if x %}{% elif _secret %}{% endif %}
  |                   ^^^^^^^
```

### single equals sign is an operand

```htmldjango
{% if x = y %}{% endif %}
```

```snapshot
error[S114]: Could not parse the remainder: '=' from '='
 --> test.html:1:9
  |
1 | {% if x = y %}{% endif %}
  |         ^
```

### operand uses an unknown filter

```htmldjango
{% if x|nope %}{% endif %}
```

```snapshot
error[S111]: Unknown filter 'nope'
 --> test.html:1:9
  |
1 | {% if x|nope %}{% endif %}
  |         ^^^^
```

### accepts operators and filtered operands

```htmldjango
{% if not user.is_staff and items|length > 0 %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### accepts elif operators

```htmldjango
{% if x %}{% elif y is not None %}{% endif %}
```

```snapshot
✓ no diagnostics
```

## firstof tag

### value is not a valid filter expression

```htmldjango
{% firstof a| b %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'a|'
 --> test.html:1:12
  |
1 | {% firstof a| b %}
  |            ^^
```

### filter is missing its argument

```htmldjango
{% firstof value|default %}
```

```snapshot
error[S115]: Filter 'default' requires an argument
 --> test.html:1:18
  |
1 | {% firstof value|default %}
  |                  ^^^^^^^
```

### operator words are variable names

```htmldjango
{% firstof and x %}
```

```snapshot
✓ no diagnostics
```

### does not compile the as target

```htmldjango
{% firstof a b|default:"x" as _c %}
```

```snapshot
✓ no diagnostics
```

## cycle tag

### value starts with an underscore

```htmldjango
{% cycle 'a' _b %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_b'
 --> test.html:1:14
  |
1 | {% cycle 'a' _b %}
  |              ^^
```

### does not compile the as target

```htmldjango
{% cycle 'a' 'b' as _rows silent %}
```

```snapshot
✓ no diagnostics
```

## for tag

### sequence is not a valid filter expression

```htmldjango
{% for x in items| %}{% endfor %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'items|'
 --> test.html:1:13
  |
1 | {% for x in items| %}{% endfor %}
  |             ^^^^^^
```

### sequence uses an unknown filter

```htmldjango
{% for x in items|nope %}{% endfor %}
```

```snapshot
error[S111]: Unknown filter 'nope'
 --> test.html:1:19
  |
1 | {% for x in items|nope %}{% endfor %}
  |                   ^^^^
```

### does not compile loop variables

```htmldjango
{% for _x in items|dictsort:"name" reversed %}{% endfor %}
```

```snapshot
✓ no diagnostics
```

## ifchanged tag

### value is not a valid filter expression

```htmldjango
{% ifchanged date| %}{% endifchanged %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'date|'
 --> test.html:1:14
  |
1 | {% ifchanged date| %}{% endifchanged %}
  |              ^^^^^
```

### accepts several values

```htmldjango
{% ifchanged date.date date.hour %}{% endifchanged %}
```

```snapshot
✓ no diagnostics
```

## regroup tag

### grouper starts with an underscore

```htmldjango
{% regroup people by _gender as groups %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: 'groups._gender'
 --> test.html:1:22
  |
1 | {% regroup people by _gender as groups %}
  |                      ^^^^^^^^^^^^^^^^^
```

### accepts a filtered target

```htmldjango
{% regroup people|dictsort:"gender" by gender as groups %}
```

```snapshot
✓ no diagnostics
```

## url tag

### keyword argument has no value

```htmldjango
{% url 'home' page= %}
```

```snapshot
error[S114]: Could not parse the remainder: '=' from 'page='
 --> test.html:1:15
  |
1 | {% url 'home' page= %}
  |               ^^^^^
```

### does not compile the as target

```htmldjango
{% url 'home' page=1 as _link %}
```

```snapshot
✓ no diagnostics
```

## widthratio tag

### value is not a valid filter expression

```htmldjango
{% widthratio this max 100| %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from '100|'
 --> test.html:1:24
  |
1 | {% widthratio this max 100| %}
  |                        ^^^^
```

### does not compile the as target

```htmldjango
{% widthratio this max 100 as _w %}
```

```snapshot
✓ no diagnostics
```

## with tag

### value starts with an underscore

```htmldjango
{% with total=_count %}{% endwith %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_count'
 --> test.html:1:15
  |
1 | {% with total=_count %}{% endwith %}
  |               ^^^^^^
```

### legacy value starts with an underscore

```htmldjango
{% with _count as total %}{% endwith %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_count'
 --> test.html:1:9
  |
1 | {% with _count as total %}{% endwith %}
  |         ^^^^^^
```

### accepts modern and legacy assignments

```htmldjango
{% with total=items|length %}{% endwith %}
{% with items|length as total and user.name as name %}{% endwith %}
```

```snapshot
✓ no diagnostics
```

## filter tag

### has a dangling argument separator

```htmldjango
{% filter lower: %}{% endfilter %}
```

```snapshot
error[S114]: Could not parse the remainder: ':' from 'var|lower:'
 --> test.html:1:11
  |
1 | {% filter lower: %}{% endfilter %}
  |           ^^^^^^
```

### uses escape

```htmldjango
{% filter escape %}{% endfilter %}
```

```snapshot
error[S114]: "filter escape" is not permitted. Use the "autoescape" tag instead.
 --> test.html:1:11
  |
1 | {% filter escape %}{% endfilter %}
  |           ^^^^^^
```

### uses safe after another filter

```htmldjango
{% filter lower|safe %}{% endfilter %}
```

```snapshot
error[S114]: "filter safe" is not permitted. Use the "autoescape" tag instead.
 --> test.html:1:17
  |
1 | {% filter lower|safe %}{% endfilter %}
  |                 ^^^^
```

### uses an unknown filter

```htmldjango
{% filter lower|nope %}{% endfilter %}
```

```snapshot
error[S111]: Unknown filter 'nope'
 --> test.html:1:17
  |
1 | {% filter lower|nope %}{% endfilter %}
  |                 ^^^^
```

### accepts a filter chain

```htmldjango
{% filter lower|force_escape %}{% endfilter %}
```

```snapshot
✓ no diagnostics
```

## lorem tag

### count starts with an underscore

```htmldjango
{% lorem _n w %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_n'
 --> test.html:1:10
  |
1 | {% lorem _n w %}
  |          ^^
```

### accepts count, method, and random

```htmldjango
{% lorem 2 w random %}
```

```snapshot
✓ no diagnostics
```

## extends tag

### parent starts with an underscore

```htmldjango
{% extends _parent %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_parent'
 --> test.html:1:12
  |
1 | {% extends _parent %}
  |            ^^^^^^^
```

### accepts a variable parent

```htmldjango
{% extends parent %}
```

```snapshot
✓ no diagnostics
```

## include tag

### assignment is not a valid filter expression

```htmldjango
{% include "card.html" with item=card| %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'card|'
 --> test.html:1:34
  |
1 | {% include "card.html" with item=card| %}
  |                                  ^^^^^
```

### accepts assignments and only

```htmldjango
{% include card_template with item=card only %}
```

```snapshot
✓ no diagnostics
```

## i18n tags

### translate message starts with an underscore

```htmldjango
{% load i18n %}
{% translate _msg %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_msg'
 --> test.html:2:14
  |
2 | {% translate _msg %}
  |              ^^^^
```

### blocktranslate count starts with an underscore

```htmldjango
{% load i18n %}
{% blocktranslate count counter=_n %}one{% plural %}many{% endblocktranslate %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_n'
 --> test.html:2:33
  |
2 | {% blocktranslate count counter=_n %}one{% plural %}many{% endblocktranslate %}
  |                                 ^^
```

### blocktranslate context reports its own message

```htmldjango
{% load i18n %}
{% blocktranslate context _c %}{% endblocktranslate %}
```

```snapshot
error[S114]: "context" in 'blocktranslate' tag expected exactly one argument.
 --> test.html:2:27
  |
2 | {% blocktranslate context _c %}{% endblocktranslate %}
  |                           ^^
```

### language code starts with an underscore

```htmldjango
{% load i18n %}
{% language _code %}{% endlanguage %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_code'
 --> test.html:2:13
  |
2 | {% language _code %}{% endlanguage %}
  |             ^^^^^
```

### get_language_info code starts with an underscore

```htmldjango
{% load i18n %}
{% get_language_info for _code as lang %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_code'
 --> test.html:2:26
  |
2 | {% get_language_info for _code as lang %}
  |                          ^^^^^
```

### accepts expression arguments and plain targets

```htmldjango
{% load i18n %}
{% translate "Hello" context "greeting" as _hello %}
{% blocktranslate with name=user.name asvar _greeting %}Hi {{ name }}{% endblocktranslate %}
{% language "de" %}{% endlanguage %}
{% get_language_info for "de" as lang %}
{% get_language_info_list for languages as _langs %}
```

```snapshot
✓ no diagnostics
```

## static tag

### path starts with an underscore

```htmldjango
{% load static %}
{% static _path %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_path'
 --> test.html:2:11
  |
2 | {% static _path %}
  |           ^^^^^
```

### does not compile the as target

```htmldjango
{% load static %}
{% static "app.css" as _css %}
```

```snapshot
✓ no diagnostics
```

## timezone tag

### value is not a valid filter expression

```htmldjango
{% load tz %}
{% timezone tz| %}{% endtimezone %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from 'tz|'
 --> test.html:2:13
  |
2 | {% timezone tz| %}{% endtimezone %}
  |             ^^^
```

### accepts a literal zone

```htmldjango
{% load tz %}
{% timezone "Europe/Paris" %}{% endtimezone %}
```

```snapshot
✓ no diagnostics
```

## cache tag

### timeout is not a valid filter expression

```htmldjango
{% load cache %}
{% cache 500| sidebar %}{% endcache %}
```

```snapshot
error[S114]: Could not parse the remainder: '|' from '500|'
 --> test.html:2:10
  |
2 | {% cache 500| sidebar %}{% endcache %}
  |          ^^^^
```

### does not compile the fragment name

```htmldjango
{% load cache %}
{% cache 500 _sidebar request.user.pk using="default" %}{% endcache %}
```

```snapshot
✓ no diagnostics
```

## querystring tag

### keyword value starts with an underscore

```htmldjango
{% querystring page=_page %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_page'
 --> test.html:1:21
  |
1 | {% querystring page=_page %}
  |                     ^^^^^
```

### accepts keyword expressions

```htmldjango
{% querystring page=page.number %}
```

```snapshot
✓ no diagnostics
```

## simple tags

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {}}}]
```

### argument starts with an underscore

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def greet(name): return f"Hello, {name}"
```

```htmldjango
{% greet _name %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_name'
 --> test.html:1:10
  |
1 | {% greet _name %}
  |          ^^^^^
```

### does not compile the as target

`custom_tags.py`:

```py
from django import template
register = template.Library()
@register.simple_tag
def greet(name): return f"Hello, {name}"
```

```htmldjango
{% greet name=user.name as _greeting %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

### signs in variable names depend on the Django version

Django 5.2 rejects the remainder after `a`; Django 6.0 and later reject the `-` in the variable name. Both reject the template, but with different messages, so DJLS does not report it.

```htmldjango
{% firstof a-b %}
```

```snapshot
✓ no diagnostics
```

### hand-written compile functions are not analyzed for expression arguments

Only Django's built-in tags and `parse_bits()` / `token_kwargs()` argument syntax identify expression arguments.

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {}}}]
```

`custom_tags.py`:

```py
from django import template
register = template.Library()
class ValueNode(template.Node):
    def __init__(self, value): self.value = value
    def render(self, context): return str(self.value.resolve(context))
@register.tag
def show(parser, token):
    bits = token.split_contents()
    return ValueNode(parser.compile_filter(bits[1]))
```

```htmldjango
{% show _value %}
```

```snapshot
✓ no diagnostics
```

### template variables are not checked as filter expressions

Django compiles `{{ }}` contents with the same `FilterExpression` grammar.

```htmldjango
{{ _private }}
```

```snapshot
✓ no diagnostics
```
