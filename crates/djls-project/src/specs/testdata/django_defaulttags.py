# Vendored unit-test fixture.
# Corpus: django-6.0/django/template/defaulttags.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

@register.tag
def autoescape(parser, token):
    """
    Force autoescape behavior for this block.
    """
    # token.split_contents() isn't useful here because this tag doesn't accept
    # variable as arguments.
    args = token.contents.split()
    if len(args) != 2:
        raise TemplateSyntaxError("'autoescape' tag requires exactly one argument.")
    arg = args[1]
    if arg not in ("on", "off"):
        raise TemplateSyntaxError("'autoescape' argument should be 'on' or 'off'")
    nodelist = parser.parse(("endautoescape",))
    parser.delete_first_token()
    return AutoEscapeControlNode((arg == "on"), nodelist)

@register.tag
def comment(parser, token):
    """
    Ignore everything between ``{% comment %}`` and ``{% endcomment %}``.
    """
    parser.skip_past("endcomment")
    return CommentNode()

@register.tag
def cycle(parser, token):
    """
    Cycle among the given strings each time this tag is encountered.

    Within a loop, cycles among the given strings each time through
    the loop::

        {% for o in some_list %}
            <tr class="{% cycle 'row1' 'row2' %}">
                ...
            </tr>
        {% endfor %}

    Outside of a loop, give the values a unique name the first time you call
    it, then use that name each successive time through::

            <tr class="{% cycle 'row1' 'row2' 'row3' as rowcolors %}">...</tr>
            <tr class="{% cycle rowcolors %}">...</tr>
            <tr class="{% cycle rowcolors %}">...</tr>

    You can use any number of values, separated by spaces. Commas can also
    be used to separate values; if a comma is used, the cycle values are
    interpreted as literal strings.

    The optional flag "silent" can be used to prevent the cycle declaration
    from returning any value::

        {% for o in some_list %}
            {% cycle 'row1' 'row2' as rowcolors silent %}
            <tr class="{{ rowcolors }}">{% include "subtemplate.html " %}</tr>
        {% endfor %}
    """
    # Note: This returns the exact same node on each {% cycle name %} call;
    # that is, the node object returned from {% cycle a b c as name %} and the
    # one returned from {% cycle name %} are the exact same object. This
    # shouldn't cause problems (heh), but if it does, now you know.
    #
    # Ugly hack warning: This stuffs the named template dict into parser so
    # that names are only unique within each template (as opposed to using
    # a global variable, which would make cycle names have to be unique across
    # *all* templates.
    #
    # It keeps the last node in the parser to be able to reset it with
    # {% resetcycle %}.

    args = token.split_contents()

    if len(args) < 2:
        raise TemplateSyntaxError("'cycle' tag requires at least two arguments")

    if len(args) == 2:
        # {% cycle foo %} case.
        name = args[1]
        if not hasattr(parser, "_named_cycle_nodes"):
            raise TemplateSyntaxError(
                "No named cycles in template. '%s' is not defined" % name
            )
        if name not in parser._named_cycle_nodes:
            raise TemplateSyntaxError("Named cycle '%s' does not exist" % name)
        return parser._named_cycle_nodes[name]

    as_form = False

    if len(args) > 4:
        # {% cycle ... as foo [silent] %} case.
        if args[-3] == "as":
            if args[-1] != "silent":
                raise TemplateSyntaxError(
                    "Only 'silent' flag is allowed after cycle's name, not '%s'."
                    % args[-1]
                )
            as_form = True
            silent = True
            args = args[:-1]
        elif args[-2] == "as":
            as_form = True
            silent = False

    if as_form:
        name = args[-1]
        values = [parser.compile_filter(arg) for arg in args[1:-2]]
        node = CycleNode(values, name, silent=silent)
        if not hasattr(parser, "_named_cycle_nodes"):
            parser._named_cycle_nodes = {}
        parser._named_cycle_nodes[name] = node
    else:
        values = [parser.compile_filter(arg) for arg in args[1:]]
        node = CycleNode(values)
    parser._last_cycle_node = node
    return node

@register.tag("for")
def do_for(parser, token):
    """
    Loop over each item in an array.

    For example, to display a list of athletes given ``athlete_list``::

        <ul>
        {% for athlete in athlete_list %}
            <li>{{ athlete.name }}</li>
        {% endfor %}
        </ul>

    You can loop over a list in reverse by using
    ``{% for obj in list reversed %}``.

    You can also unpack multiple values from a two-dimensional array::

        {% for key,value in dict.items %}
            {{ key }}: {{ value }}
        {% endfor %}

    The ``for`` tag can take an optional ``{% empty %}`` clause that will
    be displayed if the given array is empty or could not be found::

        <ul>
          {% for athlete in athlete_list %}
            <li>{{ athlete.name }}</li>
          {% empty %}
            <li>Sorry, no athletes in this list.</li>
          {% endfor %}
        <ul>

    The above is equivalent to -- but shorter, cleaner, and possibly faster
    than -- the following::

        <ul>
          {% if athlete_list %}
            {% for athlete in athlete_list %}
              <li>{{ athlete.name }}</li>
            {% endfor %}
          {% else %}
            <li>Sorry, no athletes in this list.</li>
          {% endif %}
        </ul>

    The for loop sets a number of variables available within the loop:

        =======================  ==============================================
        Variable                 Description
        =======================  ==============================================
        ``forloop.counter``      The current iteration of the loop (1-indexed)
        ``forloop.counter0``     The current iteration of the loop (0-indexed)
        ``forloop.revcounter``   The number of iterations from the end of the
                                 loop (1-indexed)
        ``forloop.revcounter0``  The number of iterations from the end of the
                                 loop (0-indexed)
        ``forloop.first``        True if this is the first time through the
                                 loop
        ``forloop.last``         True if this is the last time through the loop
        ``forloop.parentloop``   For nested loops, this is the loop "above" the
                                 current one
        =======================  ==============================================
    """
    bits = token.split_contents()
    if len(bits) < 4:
        raise TemplateSyntaxError(
            "'for' statements should have at least four words: %s" % token.contents
        )

    is_reversed = bits[-1] == "reversed"
    in_index = -3 if is_reversed else -2
    if bits[in_index] != "in":
        raise TemplateSyntaxError(
            "'for' statements should use the format"
            " 'for x in y': %s" % token.contents
        )

    invalid_chars = frozenset((" ", '"', "'", FILTER_SEPARATOR))
    loopvars = re.split(r" *, *", " ".join(bits[1:in_index]))
    for var in loopvars:
        if not var or not invalid_chars.isdisjoint(var):
            raise TemplateSyntaxError(
                "'for' tag received an invalid argument: %s" % token.contents
            )

    sequence = parser.compile_filter(bits[in_index + 1])
    nodelist_loop = parser.parse(
        (
            "empty",
            "endfor",
        )
    )
    token = parser.next_token()
    if token.contents == "empty":
        nodelist_empty = parser.parse(("endfor",))
        parser.delete_first_token()
    else:
        nodelist_empty = None
    return ForNode(loopvars, sequence, is_reversed, nodelist_loop, nodelist_empty)

@register.tag("if")
def do_if(parser, token):
    """
    Evaluate a variable, and if that variable is "true" (i.e., exists, is not
    empty, and is not a false boolean value), output the contents of the block:

    ::

        {% if athlete_list %}
            Number of athletes: {{ athlete_list|count }}
        {% elif athlete_in_locker_room_list %}
            Athletes should be out of the locker room soon!
        {% else %}
            No athletes.
        {% endif %}

    In the above, if ``athlete_list`` is not empty, the number of athletes will
    be displayed by the ``{{ athlete_list|count }}`` variable.

    The ``if`` tag may take one or several `` {% elif %}`` clauses, as well as
    an ``{% else %}`` clause that will be displayed if all previous conditions
    fail. These clauses are optional.

    ``if`` tags may use ``or``, ``and`` or ``not`` to test a number of
    variables or to negate a given variable::

        {% if not athlete_list %}
            There are no athletes.
        {% endif %}

        {% if athlete_list or coach_list %}
            There are some athletes or some coaches.
        {% endif %}

        {% if athlete_list and coach_list %}
            Both athletes and coaches are available.
        {% endif %}

        {% if not athlete_list or coach_list %}
            There are no athletes, or there are some coaches.
        {% endif %}

        {% if athlete_list and not coach_list %}
            There are some athletes and absolutely no coaches.
        {% endif %}

    Comparison operators are also available, and the use of filters is also
    allowed, for example::

        {% if articles|length >= 5 %}...{% endif %}

    Arguments and operators _must_ have a space between them, so
    ``{% if 1>2 %}`` is not a valid if tag.

    All supported operators are: ``or``, ``and``, ``in``, ``not in``
    ``==``, ``!=``, ``>``, ``>=``, ``<`` and ``<=``.

    Operator precedence follows Python.
    """
    # {% if ... %}
    bits = token.split_contents()[1:]
    condition = TemplateIfParser(parser, bits).parse()
    nodelist = parser.parse(("elif", "else", "endif"))
    conditions_nodelists = [(condition, nodelist)]
    token = parser.next_token()

    # {% elif ... %} (repeatable)
    while token.contents.startswith("elif"):
        bits = token.split_contents()[1:]
        condition = TemplateIfParser(parser, bits).parse()
        nodelist = parser.parse(("elif", "else", "endif"))
        conditions_nodelists.append((condition, nodelist))
        token = parser.next_token()

    # {% else %} (optional)
    if token.contents == "else":
        nodelist = parser.parse(("endif",))
        conditions_nodelists.append((None, nodelist))
        token = parser.next_token()

    # {% endif %}
    if token.contents != "endif":
        raise TemplateSyntaxError(
            'Malformed template tag at line {}: "{}"'.format(
                token.lineno, token.contents
            )
        )

    return IfNode(conditions_nodelists)

@register.tag
def now(parser, token):
    """
    Display the date, formatted according to the given string.

    Use the same format as PHP's ``date()`` function; see https://php.net/date
    for all the possible values.

    Sample usage::

        It is {% now "jS F Y H:i" %}
    """
    bits = token.split_contents()
    asvar = None
    if len(bits) == 4 and bits[-2] == "as":
        asvar = bits[-1]
        bits = bits[:-2]
    if len(bits) != 2:
        raise TemplateSyntaxError("'now' statement takes one argument")
    format_string = bits[1][1:-1]
    return NowNode(format_string, asvar)

@register.tag(name="partial")
def partial_func(parser, token):
    """
    Render a partial previously declared with the ``{% partialdef %}`` tag.

    Usage::

        {% partial partial_name %}
    """
    match token.split_contents():
        case "partial", partial_name:
            extra_data = parser.extra_data
            partial_mapping = DeferredSubDict(extra_data, "partials")
            return PartialNode(partial_name, partial_mapping=partial_mapping)
        case _:
            raise TemplateSyntaxError("'partial' tag requires a single argument")

@register.tag(name="partialdef")
def partialdef_func(parser, token):
    """
    Declare a partial that can be used in the template.

    Usage::

        {% partialdef partial_name %}
        Content goes here.
        {% endpartialdef %}

    Store the nodelist in the context under the key "partials". It can be
    retrieved using the ``{% partial %}`` tag.

    The optional ``inline`` argument renders the partial's contents
    immediately, at the point where it is defined.
    """
    match token.split_contents():
        case "partialdef", partial_name, "inline":
            inline = True
        case "partialdef", partial_name, _:
            raise TemplateSyntaxError(
                "The 'inline' argument does not have any parameters; either use "
                "'inline' or remove it completely."
            )
        case "partialdef", partial_name:
            inline = False
        case ["partialdef"]:
            raise TemplateSyntaxError("'partialdef' tag requires a name")
        case _:
            raise TemplateSyntaxError("'partialdef' tag takes at most 2 arguments")

    # Parse the content until the end tag.
    valid_endpartials = ("endpartialdef", f"endpartialdef {partial_name}")

    pos_open = getattr(token, "position", None)
    source_start = pos_open[0] if isinstance(pos_open, tuple) else None

    nodelist = parser.parse(valid_endpartials)
    endpartial = parser.next_token()
    if endpartial.contents not in valid_endpartials:
        parser.invalid_block_tag(endpartial, "endpartialdef", valid_endpartials)

    pos_close = getattr(endpartial, "position", None)
    source_end = pos_close[1] if isinstance(pos_close, tuple) else None

    # Store the partial nodelist in the parser.extra_data attribute.
    partials = parser.extra_data.setdefault("partials", {})
    if partial_name in partials:
        raise TemplateSyntaxError(
            f"Partial '{partial_name}' is already defined in the "
            f"'{parser.origin.name}' template."
        )
    partials[partial_name] = PartialTemplate(
        nodelist,
        parser.origin,
        partial_name,
        source_start=source_start,
        source_end=source_end,
    )

    return PartialDefNode(partial_name, inline, nodelist)

@register.tag
def regroup(parser, token):
    """
    Regroup a list of alike objects by a common attribute.

    This complex tag is best illustrated by use of an example: say that
    ``musicians`` is a list of ``Musician`` objects that have ``name`` and
    ``instrument`` attributes, and you'd like to display a list that
    looks like:

        * Guitar:
            * Django Reinhardt
            * Emily Remler
        * Piano:
            * Lovie Austin
            * Bud Powell
        * Trumpet:
            * Duke Ellington

    The following snippet of template code would accomplish this dubious task::

        {% regroup musicians by instrument as grouped %}
        <ul>
        {% for group in grouped %}
            <li>{{ group.grouper }}
            <ul>
                {% for musician in group.list %}
                <li>{{ musician.name }}</li>
                {% endfor %}
            </ul>
        {% endfor %}
        </ul>

    As you can see, ``{% regroup %}`` populates a variable with a list of
    objects with ``grouper`` and ``list`` attributes. ``grouper`` contains the
    item that was grouped by; ``list`` contains the list of objects that share
    that ``grouper``. In this case, ``grouper`` would be ``Guitar``, ``Piano``
    and ``Trumpet``, and ``list`` is the list of musicians who play this
    instrument.

    Note that ``{% regroup %}`` does not work when the list to be grouped is
    not sorted by the key you are grouping by! This means that if your list of
    musicians was not sorted by instrument, you'd need to make sure it is
    sorted before using it, i.e.::

        {% regroup musicians|dictsort:"instrument" by instrument as grouped %}
    """
    bits = token.split_contents()
    if len(bits) != 6:
        raise TemplateSyntaxError("'regroup' tag takes five arguments")
    target = parser.compile_filter(bits[1])
    if bits[2] != "by":
        raise TemplateSyntaxError("second argument to 'regroup' tag must be 'by'")
    if bits[4] != "as":
        raise TemplateSyntaxError("next-to-last argument to 'regroup' tag must be 'as'")
    var_name = bits[5]
    # RegroupNode will take each item in 'target', put it in the context under
    # 'var_name', evaluate 'var_name'.'expression' in the current context, and
    # group by the resulting value. After all items are processed, it will
    # save the final result in the context under 'var_name', thus clearing the
    # temporary values. This hack is necessary because the template engine
    # doesn't provide a context-aware equivalent of Python's getattr.
    expression = parser.compile_filter(
        var_name + VARIABLE_ATTRIBUTE_SEPARATOR + bits[3]
    )
    return RegroupNode(target, expression, var_name)

@register.tag
def spaceless(parser, token):
    """
    Remove whitespace between HTML tags, including tab and newline characters.

    Example usage::

        {% spaceless %}
            <p>
                <a href="foo/">Foo</a>
            </p>
        {% endspaceless %}

    This example returns this HTML::

        <p><a href="foo/">Foo</a></p>

    Only space between *tags* is normalized -- not space between tags and text.
    In this example, the space around ``Hello`` isn't stripped::

        {% spaceless %}
            <strong>
                Hello
            </strong>
        {% endspaceless %}
    """
    nodelist = parser.parse(("endspaceless",))
    parser.delete_first_token()
    return SpacelessNode(nodelist)

@register.tag
def templatetag(parser, token):
    """
    Output one of the bits used to compose template tags.

    Since the template system has no concept of "escaping", to display one of
    the bits used in template tags, you must use the ``{% templatetag %}`` tag.

    The argument tells which template bit to output:

        ==================  =======
        Argument            Outputs
        ==================  =======
        ``openblock``       ``{%``
        ``closeblock``      ``%}``
        ``openvariable``    ``{{``
        ``closevariable``   ``}}``
        ``openbrace``       ``{``
        ``closebrace``      ``}``
        ``opencomment``     ``{#``
        ``closecomment``    ``#}``
        ==================  =======
    """
    # token.split_contents() isn't useful here because this tag doesn't accept
    # variable as arguments.
    bits = token.contents.split()
    if len(bits) != 2:
        raise TemplateSyntaxError("'templatetag' statement takes one argument")
    tag = bits[1]
    if tag not in TemplateTagNode.mapping:
        raise TemplateSyntaxError(
            "Invalid templatetag argument: '%s'."
            " Must be one of: %s" % (tag, list(TemplateTagNode.mapping))
        )
    return TemplateTagNode(tag)

@register.tag
def url(parser, token):
    r"""
    Return an absolute URL matching the given view with its parameters.

    This is a way to define links that aren't tied to a particular URL
    configuration::

        {% url "url_name" arg1 arg2 %}

        or

        {% url "url_name" name1=value1 name2=value2 %}

    The first argument is a URL pattern name. Other arguments are
    space-separated values that will be filled in place of positional and
    keyword arguments in the URL. Don't mix positional and keyword arguments.
    All arguments for the URL must be present.

    For example, if you have a view ``app_name.views.client_details`` taking
    the client's id and the corresponding line in a URLconf looks like this::

        path(
            'client/<int:id>/',
            views.client_details,
            name='client-detail-view',
        )

    and this app's URLconf is included into the project's URLconf under some
    path::

        path('clients/', include('app_name.urls'))

    then in a template you can create a link for a certain client like this::

        {% url "client-detail-view" client.id %}

    The URL will look like ``/clients/client/123/``.

    The first argument may also be the name of a template variable that will be
    evaluated to obtain the view name or the URL name, e.g.::

        {% with url_name="client-detail-view" %}
        {% url url_name client.id %}
        {% endwith %}
    """
    bits = token.split_contents()
    if len(bits) < 2:
        raise TemplateSyntaxError(
            "'%s' takes at least one argument, a URL pattern name." % bits[0]
        )
    viewname = parser.compile_filter(bits[1])
    args = []
    kwargs = {}
    asvar = None
    bits = bits[2:]
    if len(bits) >= 2 and bits[-2] == "as":
        asvar = bits[-1]
        bits = bits[:-2]

    for bit in bits:
        match = kwarg_re.match(bit)
        if not match:
            raise TemplateSyntaxError("Malformed arguments to url tag")
        name, value = match.groups()
        if name:
            kwargs[name] = parser.compile_filter(value)
        else:
            args.append(parser.compile_filter(value))

    return URLNode(viewname, args, kwargs, asvar)

@register.tag
def verbatim(parser, token):
    """
    Stop the template engine from rendering the contents of this block tag.

    Usage::

        {% verbatim %}
            {% don't process this %}
        {% endverbatim %}

    You can also designate a specific closing tag block (allowing the
    unrendered use of ``{% endverbatim %}``)::

        {% verbatim myblock %}
            ...
        {% endverbatim myblock %}
    """
    nodelist = parser.parse(("endverbatim",))
    parser.delete_first_token()
    return VerbatimNode(nodelist.render(Context()))

@register.tag
def widthratio(parser, token):
    """
    For creating bar charts and such. Calculate the ratio of a given value to a
    maximum value, and then apply that ratio to a constant.

    For example::

        <img src="bar.png" alt="Bar"
             height="10"
             width="{% widthratio this_value max_value max_width %}">

    If ``this_value`` is 175, ``max_value`` is 200, and ``max_width`` is 100,
    the image in the above example will be 88 pixels wide
    (because 175/200 = .875; .875 * 100 = 87.5 which is rounded up to 88).

    In some cases you might want to capture the result of widthratio in a
    variable. It can be useful for instance in a blocktranslate like this::

        {% widthratio this_value max_value max_width as width %}
        {% blocktranslate %}The width is: {{ width }}{% endblocktranslate %}
    """
    bits = token.split_contents()
    if len(bits) == 4:
        tag, this_value_expr, max_value_expr, max_width = bits
        asvar = None
    elif len(bits) == 6:
        tag, this_value_expr, max_value_expr, max_width, as_, asvar = bits
        if as_ != "as":
            raise TemplateSyntaxError(
                "Invalid syntax in widthratio tag. Expecting 'as' keyword"
            )
    else:
        raise TemplateSyntaxError("widthratio takes at least three arguments")

    return WidthRatioNode(
        parser.compile_filter(this_value_expr),
        parser.compile_filter(max_value_expr),
        parser.compile_filter(max_width),
        asvar=asvar,
    )

@register.simple_tag(name="querystring", takes_context=True)
def querystring(context, *args, **kwargs):
    """
    Build a query string using `args` and `kwargs` arguments.

    This tag constructs a new query string by adding, removing, or modifying
    parameters from the given positional and keyword arguments. Positional
    arguments must be mappings (such as `QueryDict` or `dict`), and
    `request.GET` is used as the starting point if `args` is empty.

    Keyword arguments are treated as an extra, final mapping. These mappings
    are processed sequentially, with later arguments taking precedence.

    Passing `None` as a value removes the corresponding key from the result.
    For iterable values, `None` entries are ignored, but if all values are
    `None`, the key is removed.

    A query string prefixed with `?` is returned.

    Raise TemplateSyntaxError if a positional argument is not a mapping or if
    keys are not strings.

    For example::

        {# Set a parameter on top of `request.GET` #}
        {% querystring foo=3 %}

        {# Remove a key from `request.GET` #}
        {% querystring foo=None %}

        {# Use with pagination #}
        {% querystring page=page_obj.next_page_number %}

        {# Use a custom ``QueryDict`` #}
        {% querystring my_query_dict foo=3 %}

        {# Use multiple positional and keyword arguments #}
        {% querystring my_query_dict my_dict foo=3 bar=None %}
    """
    if not args:
        args = [context.request.GET]
    params = QueryDict(mutable=True)
    for d in [*args, kwargs]:
        if not isinstance(d, Mapping):
            raise TemplateSyntaxError(
                "querystring requires mappings for positional arguments (got "
                "%r instead)." % d
            )
        items = d.lists() if isinstance(d, QueryDict) else d.items()
        for key, value in items:
            if not isinstance(key, str):
                raise TemplateSyntaxError(
                    "querystring requires strings for mapping keys (got %r "
                    "instead)." % key
                )
            if value is None:
                params.pop(key, None)
            elif isinstance(value, Iterable) and not isinstance(value, str):
                # Drop None values; if no values remain, the key is removed.
                params.setlist(key, [v for v in value if v is not None])
            else:
                params[key] = value
    query_string = params.urlencode() if params else ""
    return f"?{query_string}"
