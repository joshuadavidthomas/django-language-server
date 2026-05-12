# url

## Valid

### reverses a named url

```htmldjango
{% url 'home' %}
```

```snapshot
✓ no diagnostics
```

### passes positional arguments

```htmldjango
{% url 'article-detail' article.pk %}
```

```snapshot
✓ no diagnostics
```

### passes keyword arguments

```htmldjango
{% url 'article-detail' pk=article.pk %}
```

```snapshot
✓ no diagnostics
```

### assigns reversed url to a variable

```htmldjango
{% url 'home' as home_url %}
```

```snapshot
✓ no diagnostics
```

### accepts a variable view name

```htmldjango
{% url view_name %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### requires a view name

```htmldjango
{% url %}
```

```snapshot
error[S117]: 'url' requires at least 1 argument
 --> test.html:1:1
  |
1 | {% url %}
  | ^^^^^^^^^
  |
  = note: in tag: url
```
