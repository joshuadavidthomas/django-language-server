# Vendored unit-test fixture.
# Corpus: django-6.0/django/template/defaultfilters.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.filter(is_safe=False)
def add(value, arg):
    """Add the arg to the value."""
    try:
        return int(value) + int(arg)
    except (ValueError, TypeError):
        try:
            return value + arg
        except Exception:
            return ""

@register.filter(is_safe=True)
@stringfilter
def addslashes(value):
    """
    Add slashes before quotes. Useful for escaping strings in CSV, for
    example. Less useful for escaping JavaScript; use the ``escapejs``
    filter instead.
    """
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("'", "\\'")

@register.filter
@stringfilter
def cut(value, arg):
    """Remove all values of arg from the given string."""
    safe = isinstance(value, SafeData)
    value = value.replace(arg, "")
    if safe and arg != ";":
        return mark_safe(value)
    return value

@register.filter(expects_localtime=True, is_safe=False)
def date(value, arg=None):
    """Format a date according to the given format."""
    if value in (None, ""):
        return ""
    try:
        return formats.date_format(value, arg)
    except AttributeError:
        try:
            return format(value, arg)
        except AttributeError:
            return ""

@register.filter(is_safe=False)
def default(value, arg):
    """If value is unavailable, use given default."""
    return value or arg

@register.filter("escapejs")
@stringfilter
def escapejs_filter(value):
    """Hex encode characters for use in JavaScript strings."""
    return escapejs(value)

@register.filter(is_safe=True)
def floatformat(text, arg=-1):
    """
    Display a float to a specified number of decimal places.

    If called without an argument, display the floating point number with one
    decimal place -- but only if there's a decimal place to be displayed:

    * num1 = 34.23234
    * num2 = 34.00000
    * num3 = 34.26000
    * {{ num1|floatformat }} displays "34.2"
    * {{ num2|floatformat }} displays "34"
    * {{ num3|floatformat }} displays "34.3"

    If arg is positive, always display exactly arg number of decimal places:

    * {{ num1|floatformat:3 }} displays "34.232"
    * {{ num2|floatformat:3 }} displays "34.000"
    * {{ num3|floatformat:3 }} displays "34.260"

    If arg is negative, display arg number of decimal places -- but only if
    there are places to be displayed:

    * {{ num1|floatformat:"-3" }} displays "34.232"
    * {{ num2|floatformat:"-3" }} displays "34"
    * {{ num3|floatformat:"-3" }} displays "34.260"

    If arg has the 'g' suffix, force the result to be grouped by the
    THOUSAND_SEPARATOR for the active locale. When the active locale is
    en (English):

    * {{ 6666.6666|floatformat:"2g" }} displays "6,666.67"
    * {{ 10000|floatformat:"g" }} displays "10,000"

    If arg has the 'u' suffix, force the result to be unlocalized. When the
    active locale is pl (Polish):

    * {{ 66666.6666|floatformat:"2" }} displays "66666,67"
    * {{ 66666.6666|floatformat:"2u" }} displays "66666.67"

    If the input float is infinity or NaN, display the string representation
    of that value.
    """
    force_grouping = False
    use_l10n = True
    if isinstance(arg, str):
        last_char = arg[-1]
        if arg[-2:] in {"gu", "ug"}:
            force_grouping = True
            use_l10n = False
            arg = arg[:-2] or -1
        elif last_char == "g":
            force_grouping = True
            arg = arg[:-1] or -1
        elif last_char == "u":
            use_l10n = False
            arg = arg[:-1] or -1
    try:
        input_val = str(text)
        d = Decimal(input_val)
    except InvalidOperation:
        try:
            d = Decimal(str(float(text)))
        except (ValueError, InvalidOperation, TypeError):
            return ""
    try:
        p = int(arg)
    except ValueError:
        return input_val

    _, digits, exponent = d.as_tuple()
    try:
        number_of_digits_and_exponent_sum = len(digits) + abs(exponent)
    except TypeError:
        # Exponent values can be "F", "n", "N".
        number_of_digits_and_exponent_sum = 0

    # Values with more than 200 digits, or with a large exponent, are returned
    # "as is" to avoid high memory consumption and potential denial-of-service
    # attacks. The cut-off of 200 is consistent with
    # django.utils.numberformat.floatformat().
    if number_of_digits_and_exponent_sum > 200:
        return input_val

    try:
        m = int(d) - d
    except (ValueError, OverflowError, InvalidOperation):
        return input_val

    if not m and p <= 0:
        return mark_safe(
            formats.number_format(
                "%d" % (int(d)),
                0,
                use_l10n=use_l10n,
                force_grouping=force_grouping,
            )
        )

    exp = Decimal(1).scaleb(-abs(p))
    # Set the precision high enough to avoid an exception (#15789).
    tupl = d.as_tuple()
    units = len(tupl[1])
    units += -tupl[2] if m else tupl[2]
    prec = abs(p) + units + 1
    prec = max(getcontext().prec, prec)

    # Avoid conversion to scientific notation by accessing `sign`, `digits`,
    # and `exponent` from Decimal.as_tuple() directly.
    rounded_d = d.quantize(exp, ROUND_HALF_UP, Context(prec=prec))
    sign, digits, exponent = rounded_d.as_tuple()
    digits = [str(digit) for digit in reversed(digits)]
    while len(digits) <= abs(exponent):
        digits.append("0")
    digits.insert(-exponent, ".")
    if sign and rounded_d:
        digits.append("-")
    number = "".join(reversed(digits))
    return mark_safe(
        formats.number_format(
            number,
            abs(p),
            use_l10n=use_l10n,
            force_grouping=force_grouping,
        )
    )

@register.filter(is_safe=True)
@stringfilter
def lower(value):
    """Convert a string into all lowercase."""
    return value.lower()

@register.filter(is_safe=True)
@stringfilter
def title(value):
    """Convert a string into titlecase."""
    t = re.sub("([a-z])'([A-Z])", lambda m: m[0].lower(), value.title())
    return re.sub(r"\d([A-Z])", lambda m: m[0].lower(), t)

@register.filter(is_safe=False)
@stringfilter
def upper(value):
    """Convert a string into all uppercase."""
    return value.upper()
