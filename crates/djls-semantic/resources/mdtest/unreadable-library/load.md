# Unreadable Template Library loads

## loaded library has a registration DJLS could not read

```htmldjango
{% load open %}
{% known_tag %}
```

```snapshot
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 5 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:9
  |
1 | {% load open %}
  |         ^^^^
```

## unrecognized tag from an unreadable library is not reported

```htmldjango
{% load open %}
{% other_tag %}
```

```snapshot
hint[S124]: DJLS could not read a registration in `open_tags.py` at line 5 (the registered name cannot be resolved), so unrecognized tags and filters from `open` are not reported
 --> test.html:1:9
  |
1 | {% load open %}
  |         ^^^^
```
