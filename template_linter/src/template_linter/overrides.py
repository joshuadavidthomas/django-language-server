"""
Centralized "escape hatches" for cases where pure AST-derived validation isn't
practical (or would be too fragile).

Goal:
- Keep hard-coded tag/filter behavior out of core logic.
- Make it obvious where to extend/override behavior when integrating this
  library into a more capable host tool (e.g., one with an installed-app
  registry).
"""

from __future__ import annotations

from .types import ConditionalInnerTagRule
from .types import OpaqueBlockSpec

DEFAULT_OPAQUE_BLOCKS: dict[str, OpaqueBlockSpec] = {}

# Optional aliases for when rules exist under a canonical name.
# Not enabled by default; callers can pass these into `validate_template()`.
TAG_ALIASES: dict[str, str] = {}

# Optional hard-coded structural rules. Prefer extracting these from Django
# source via `template_linter.extraction.structural`.
DEFAULT_STRUCTURAL_RULES: list[ConditionalInnerTagRule] = []


# Variables that are known to represent token lists in our model.
#
# Keep this intentionally small. Treating arbitrary names like "args" as a token
# list creates false positives for third-party tags that use "args"/"kwargs" to
# mean parsed positional/keyword arguments (not raw template tokens).
#
# When a variable is actually derived from `token.split_contents()`, the
# extractor will record it in `TokenEnv.variables`, and validation will resolve
# it without needing a hardcoded allowlist here.
TOKEN_LIST_VARS = {"bits"}
