"""Validation for `{% if %}` / `{% elif %}` expression syntax."""

from __future__ import annotations

from ..template_syntax.if_expression import validate_if_expression
from ..types import TemplateTag
from ..types import ValidationError
from ..types import simple_error


def validate_if_expression_tag(tag: TemplateTag) -> list[ValidationError]:
    """
    Validate IfParser-style expression syntax for `{% if %}` and `{% elif %}`.

    This is intentionally separate from extracted tag validation rules, since:
    - `{% elif %}` is a delimiter tag (not registered), so it has no TagValidation.
    - Django's expression syntax errors are produced by `TemplateIfParser`, not
      by `do_if()` guard clauses we can extract from source.
    """
    if tag.name not in {"if", "elif"}:
        return []
    message = validate_if_expression(tag.tokens[1:])
    if not message:
        return []
    return [simple_error(tag, message)]
