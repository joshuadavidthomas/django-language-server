# Filter diagnostics

## filter requires an argument

```htmldjango
{{ value|default }}
```

```snapshot
error[S115]: Filter 'default' requires an argument
 --> test.html:1:10
  |
1 | {{ value|default }}
  |          ^^^^^^^
```

## filter does not accept an argument

```htmldjango
{{ value|upper:"arg" }}
```

```snapshot
error[S116]: Filter 'upper' does not accept an argument
 --> test.html:1:10
  |
1 | {{ value|upper:"arg" }}
  |          ^^^^^^^^^^^
```
