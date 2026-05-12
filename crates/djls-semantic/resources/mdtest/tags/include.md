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
