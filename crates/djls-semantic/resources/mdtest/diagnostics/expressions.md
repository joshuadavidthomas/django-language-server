# Expression diagnostics

Django compiles some tag arguments as template expressions while it parses a template. Extraction records each tag's Argument Consumption: the Tag Bits its compile function passes to `parser.compile_filter()`, `FilterExpression`, or Django's `kwarg_re`. DJLS parses those bits with Django's `FilterExpression` grammar, reports the `TemplateSyntaxError` Django would raise, and checks the filters they use. `if` and `elif` parse a boolean condition. Arguments whose consumption extraction cannot prove are not checked; see [Known gaps](#known-gaps).

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

### value in the as form starts with an underscore

```htmldjango
{% cycle 'a' _b 'c' as rows %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_b'
 --> test.html:1:14
  |
1 | {% cycle 'a' _b 'c' as rows %}
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

### target starts with an underscore

```htmldjango
{% regroup _people by gender as groups %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_people'
 --> test.html:1:12
  |
1 | {% regroup _people by gender as groups %}
  |            ^^^^^^^
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

### keyword value uses an unknown filter

```htmldjango
{% url 'home' page=p|nope %}
```

```snapshot
error[S111]: Unknown filter 'nope'
 --> test.html:1:22
  |
1 | {% url 'home' page=p|nope %}
  |                      ^^^^
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

## include tag

### accepts assignments and only

```htmldjango
{% include card_template with item=card only %}
```

```snapshot
✓ no diagnostics
```

## extends tag

### accepts a variable parent

```htmldjango
{% extends parent %}
```

```snapshot
✓ no diagnostics
```

## lorem tag

### accepts count, method, and random

```htmldjango
{% lorem 2 w random %}
```

```snapshot
✓ no diagnostics
```

## filter tag

### accepts a filter chain

```htmldjango
{% filter lower|force_escape %}{% endfilter %}
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

### vary-on value starts with an underscore

```htmldjango
{% load cache %}
{% cache 500 sidebar _user request.path %}{% endcache %}
```

```snapshot
error[S114]: Variables and attributes may not begin with underscores: '_user'
 --> test.html:2:22
  |
2 | {% cache 500 sidebar _user request.path %}{% endcache %}
  |                      ^^^^^
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

## custom compile functions

Extraction records the consumption of hand-written compile functions the same way it does for Django's own.

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {}}}]
```

### compiled argument starts with an underscore

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
error[S114]: Variables and attributes may not begin with underscores: '_value'
 --> test.html:1:9
  |
1 | {% show _value %}
  |         ^^^^^^
```

### keyword arguments split by kwarg_re

`custom_tags.py`:

```py
from django import template
from django.template.base import FilterExpression, kwarg_re
register = template.Library()
class ValuesNode(template.Node):
    def __init__(self, values): self.values = values
    def render(self, context): return ""
@register.tag
def values(parser, token):
    compiled = []
    for bit in token.split_contents()[1:]:
        match = kwarg_re.match(bit)
        if match and match.group(1):
            name, value = match.groups()
            compiled.append(FilterExpression(value, parser))
        else:
            compiled.append(FilterExpression(bit, parser))
    return ValuesNode(compiled)
```

```htmldjango
{% values first=a|nope _second %}
```

```snapshot
error[S111]: Unknown filter 'nope'
 --> test.html:1:19
  |
1 | {% values first=a|nope _second %}
  |                   ^^^^
error[S114]: Variables and attributes may not begin with underscores: '_second'
 --> test.html:1:24
  |
1 | {% values first=a|nope _second %}
  |                        ^^^^^^^
```

### a tag an unknown load could shadow is not checked

`settings.py`:

```py
INSTALLED_APPS = []
TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', 'DIRS': ['/templates'], 'APP_DIRS': False, 'OPTIONS': {'builtins': ['custom_tags'], 'libraries': {**UNKNOWN}}}]
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
{% load unknown_library %}
{% show _value %}
```

```snapshot
✓ no diagnostics
```

## Known gaps

These templates fail to compile in Django, but DJLS cannot yet prove which arguments Django compiles, or reports a different message.

### signs in variable names depend on the Django version

Django 5.2 rejects the remainder after `a`; Django 6.0 and later reject the `-` in the variable name. Both reject the template, but with different messages, so DJLS does not report it.

```htmldjango
{% firstof a-b %}
```

```snapshot
✓ no diagnostics
```

### if operand is not a valid filter expression

The `if` and `elif` check parses the condition but does not yet check its operands as filter expressions. Operands arrive with Condition consumption.

```htmldjango
{% if user| %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### if operand uses an unknown filter

The `if` and `elif` check parses the condition but does not yet check its operands as filter expressions. Operands arrive with Condition consumption.

```htmldjango
{% if x|nope %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### if reports a lone equals sign as unused

Django compiles each operand before parsing the condition, so it reports the operand `=`; the condition parser reports it as unused instead.

```htmldjango
{% if x = y %}{% endif %}
```

```snapshot
error[S114]: Unused '=' at end of if expression.
 --> test.html:1:1
  |
1 | {% if x = y %}{% endif %}
  | ^^^^^^^^^^^^^^
```

### elif starts with infix operator

Captured `elif` intermediates do not reach the condition check on this branch.

```htmldjango
{% if x %}{% elif and y %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### elif operand starts with an underscore

Captured `elif` intermediates do not reach the condition check, and condition operands are not checked as filter expressions.

```htmldjango
{% if x %}{% elif _secret %}{% endif %}
```

```snapshot
✓ no diagnostics
```

### cycle without as form

The path for two or three values is bounded on both sides, which extraction does not project into an argument form, so its consumption is not recorded. The longer form only records the values before the last two, because the `as_form` flag is not tracked.

```htmldjango
{% cycle 'a' _b %}
```

```snapshot
✓ no diagnostics
```

### grouper starts with an underscore

Django compiles `name.grouper`, a string built from two bits, so the grouper bit is not consumed as written.

```htmldjango
{% regroup people by _gender as groups %}
```

```snapshot
✓ no diagnostics
```

### filter tag arguments

Django compiles `"var|%s" % remainder`, a string built from the tag contents, so no bit is consumed as written. Its escape and safe refusals are not extracted either.

```htmldjango
{% filter lower: %}{% endfilter %}
```

```snapshot
✓ no diagnostics
```

### filter tag refuses escape

See above.

```htmldjango
{% filter escape %}{% endfilter %}
```

```snapshot
✓ no diagnostics
```

### count starts with an underscore

The count position depends on which optional trailing words are present, and `lorem` compiles it after popping them conditionally.

```htmldjango
{% lorem _n w %}
```

```snapshot
✓ no diagnostics
```

### parent starts with an underscore

`construct_relative_path()` rewrites the bit before Django compiles it.

```htmldjango
{% extends _parent %}
```

```snapshot
✓ no diagnostics
```

### assignment is not a valid filter expression

`include` compiles the template name after `construct_relative_path()` rewrites it, and reads `with` assignments in an option loop whose positions are not fixed.

```htmldjango
{% include "card.html" with item=card| %}
```

```snapshot
✓ no diagnostics
```

### blocktranslate count starts with an underscore

`blocktranslate` reads its options in a loop whose positions are not fixed; consumption is recorded only at fixed positions.

```htmldjango
{% load i18n %}
{% blocktranslate count counter=_n %}one{% plural %}many{% endblocktranslate %}
```

```snapshot
✓ no diagnostics
```

### blocktranslate context reports its own message

See above.

```htmldjango
{% load i18n %}
{% blocktranslate context _c %}{% endblocktranslate %}
```

```snapshot
✓ no diagnostics
```

### path starts with an underscore

`static` delegates to the `StaticNode.handle_token` classmethod, which extraction does not follow.

```htmldjango
{% load static %}
{% static _path %}
```

```snapshot
✓ no diagnostics
```

### cache using backend and last vary-on value

`cache` drops a trailing `using=` bit before compiling the vary-on values. The last bit is compiled on one path and rewritten with `removeprefix()` on the other, so neither use is recorded.

```htmldjango
{% load cache %}
{% cache 500 sidebar _user %}{% endcache %}
{% cache 500 sidebar using=_backend %}{% endcache %}
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
