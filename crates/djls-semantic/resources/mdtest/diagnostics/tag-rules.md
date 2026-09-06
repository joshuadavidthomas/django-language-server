# Tag argument diagnostics

## tag requires an argument

```htmldjango
{% one_arg_tag %}
```

```snapshot
error[S117]: 'one_arg_tag' did not receive value(s) for the argument(s): 'value'
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
error[S117]: 'one_arg_tag' received too many positional arguments
 --> test.html:1:1
  |
1 | {% one_arg_tag first second %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```
