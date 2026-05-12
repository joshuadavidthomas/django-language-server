# spaceless

## Valid

### removes whitespace between html tags

```htmldjango
{% spaceless %}
  <p>
    <a href="/">Home</a>
  </p>
{% endspaceless %}
```

```snapshot
✓ no diagnostics
```

## Invalid

### reports unclosed spaceless block

```htmldjango
{% spaceless %}
  <p>Never closed.</p>
```

```snapshot
error[S100]: Unclosed 'spaceless' tag
 --> test.html:1:1
  |
1 | {% spaceless %}
  | ^^^^^^^^^^^^^^^
```
