# Tag argument diagnostics

## tag requires an argument

```htmldjango
{% one_arg_tag %}
```

```snapshot
error[S117]: Tag 'one_arg_tag' requires at least 1 argument
 --> test.html:1:1
  |
1 | {% one_arg_tag %}
  | ^^^^^^^^^^^^^^^^^
```

## tag accepts exactly one argument

```htmldjango
{% one_arg_tag first second %}
```

```snapshot
error[S117]: Tag 'one_arg_tag' accepts at most 1 argument
 --> test.html:1:1
  |
1 | {% one_arg_tag first second %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
