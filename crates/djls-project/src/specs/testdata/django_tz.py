# Vendored unit-test fixture.
# Corpus: django-6.0/django/templatetags/tz.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.tag("get_current_timezone")
def get_current_timezone_tag(parser, token):
    """
    Store the name of the current time zone in the context.

    Usage::

        {% get_current_timezone as TIME_ZONE %}

    This will fetch the currently active time zone and put its name
    into the ``TIME_ZONE`` context variable.
    """
    # token.split_contents() isn't useful here because this tag doesn't accept
    # variable as arguments.
    args = token.contents.split()
    if len(args) != 3 or args[1] != "as":
        raise TemplateSyntaxError(
            "'get_current_timezone' requires 'as variable' (got %r)" % args
        )
    return GetCurrentTimezoneNode(args[2])

@register.tag("localtime")
def localtime_tag(parser, token):
    """
    Force or prevent conversion of datetime objects to local time,
    regardless of the value of ``settings.USE_TZ``.

    Sample usage::

        {% localtime off %}{{ value_in_utc }}{% endlocaltime %}
    """
    bits = token.split_contents()
    if len(bits) == 1:
        use_tz = True
    elif len(bits) > 2 or bits[1] not in ("on", "off"):
        raise TemplateSyntaxError("%r argument should be 'on' or 'off'" % bits[0])
    else:
        use_tz = bits[1] == "on"
    nodelist = parser.parse(("endlocaltime",))
    parser.delete_first_token()
    return LocalTimeNode(nodelist, use_tz)

@register.tag("timezone")
def timezone_tag(parser, token):
    """
    Enable a given time zone just for this block.

    The ``timezone`` argument must be an instance of a ``tzinfo`` subclass, a
    time zone name, or ``None``. If it is ``None``, the default time zone is
    used within the block.

    Sample usage::

        {% timezone "Europe/Paris" %}
            It is {{ now }} in Paris.
        {% endtimezone %}
    """
    bits = token.split_contents()
    if len(bits) != 2:
        raise TemplateSyntaxError("'%s' takes one argument (timezone)" % bits[0])
    tz = parser.compile_filter(bits[1])
    nodelist = parser.parse(("endtimezone",))
    parser.delete_first_token()
    return TimezoneNode(nodelist, tz)
