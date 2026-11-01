# include

## Valid

### includes a literal template name

```htmldjango
{% include "djls_app/header.html" %}
```

```snapshot
✓ no diagnostics
```

### passes context variables

```htmldjango
{% include "djls_app/header.html" with title="Hello" %}
```

```snapshot
✓ no diagnostics
```

### limits context to included template

```htmldjango
{% include "djls_app/header.html" only %}
```

```snapshot
✓ no diagnostics
```

### combines context variables with only

```htmldjango
{% include "djls_app/header.html" with title="Hello" only %}
```

```snapshot
✓ no diagnostics
```

### accepts a variable template name

```htmldjango
{% include template_name %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires a template name

```htmldjango
{% include %}
```

```snapshot
error[S117]: 'include' tag takes at least one argument: the name of the template to be included.
 --> test.html:1:1
  |
1 | {% include %}
  | ^^^^^^^^^^^^^
```

### reports diagnostics in a support template

```htmldjango
{% include "support.html" %}
```

`support.html`:

```htmldjango
{% include %}
```

```snapshot
error[S117]: 'include' tag takes at least one argument: the name of the template to be included.
 --> support.html:1:1
  |
1 | {% include %}
  | ^^^^^^^^^^^^^
```
