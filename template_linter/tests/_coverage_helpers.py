from __future__ import annotations

from collections.abc import Iterable

from template_linter.extraction.filters import FilterSpec
from template_linter.template_syntax.filter_syntax import (
    _extract_filter_exprs_from_token,
)
from template_linter.template_syntax.filter_syntax import _parse_filter_chain
from template_linter.template_syntax.parsing import parse_template_tags
from template_linter.template_syntax.parsing import parse_template_vars
from template_linter.types import OpaqueBlockSpec
from template_linter.types import TagValidation


def generate_tag_template(
    tag_name: str,
    validation: TagValidation,
    opaque_blocks: dict[str, OpaqueBlockSpec],
) -> tuple[str, str] | None:
    spec = validation.parse_bits_spec
    if spec:
        args: list[str] = []
        for i, _param in enumerate(spec.required_params, 1):
            args.append(f"arg{i}")
        for kw in spec.required_kwonly:
            args.append(f"{kw}=val")
        arg_str = f" {' '.join(args)}" if args else ""
        return (f"{{% {tag_name}{arg_str} %}}", "parse_bits")

    opaque = opaque_blocks.get(tag_name)
    if opaque and opaque.end_tags:
        end_tag = opaque.end_tags[0]
        if opaque.match_suffix:
            suffix = "block"
            return (
                f"{{% {tag_name} {suffix} %}}x{{% {end_tag} {suffix} %}}",
                "opaque_block_suffix",
            )
        return (f"{{% {tag_name} %}}x{{% {end_tag} %}}", "opaque_block")

    return None


def generate_filter_template(
    name: str,
    spec: FilterSpec,
) -> tuple[str, str] | None:
    min_required = max(1, spec.pos_args - spec.defaults)
    provided = max(0, min_required - 1)
    if provided > 1:
        return None

    if provided == 0:
        return (f"{{{{ value|{name} }}}}", "no_arg")
    return (f'{{{{ value|{name}:"1" }}}}', "one_arg")


def collect_filters_from_templates(
    templates: Iterable[str],
    opaque_blocks: dict[str, OpaqueBlockSpec],
) -> set[str]:
    filters: set[str] = set()
    for template in templates:
        for contents, _line in parse_template_vars(
            template, opaque_blocks=opaque_blocks
        ):
            for name, _has_arg in _parse_filter_chain(contents):
                filters.add(name)
        for tag in parse_template_tags(template, opaque_blocks=opaque_blocks):
            for token in tag.tokens[1:]:
                for expr in _extract_filter_exprs_from_token(token):
                    for name, _has_arg in _parse_filter_chain(expr):
                        filters.add(name)
    return filters
