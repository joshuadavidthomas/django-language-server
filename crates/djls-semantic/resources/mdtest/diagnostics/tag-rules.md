# Tag argument diagnostics

## tag requires an argument

```htmldjango
{% one_arg_tag %}
```

```snapshot
error[S117]: 'one_arg_tag' takes exactly 1 argument, 0 given
 --> test.html:1:1
  |
1 | {% one_arg_tag %}
  | ^^^^^^^^^^^^^^^^^
  |
  = note: in tag: one_arg_tag
```

## tag accepts exactly one argument

```htmldjango
{% one_arg_tag first second %}
```

```snapshot
error[S117]: 'one_arg_tag' takes exactly 1 argument, 2 given
 --> test.html:1:1
  |
1 | {% one_arg_tag first second %}
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: in tag: one_arg_tag
```
