#!/usr/bin/env python3
"""
Verification script for diagnostics.toml structure.
This simulates what the Rust build.rs script does to validate the TOML structure.
"""

import sys
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ImportError:
    import tomli as tomllib  # Fallback for older Python


def main():
    toml_path = Path(__file__).parent / "crates" / "djls-ide" / "diagnostics.toml"

    if not toml_path.exists():
        print(f"ERROR: {toml_path} not found", file=sys.stderr)
        return 1

    # Read and parse TOML
    with open(toml_path, "rb") as f:
        data = tomllib.load(f)

    rules = data.get("rule", [])
    print(f"Found {len(rules)} diagnostic rules\n")

    # Verify structure and generate preview of what Rust code would look like
    template_error_arms = []
    validation_error_arms = []

    for rule in rules:
        code = rule["code"]
        error_types = rule["error_type"]
        name = rule["name"]
        category = rule["category"]

        print(f"{code} ({category}): {name}")

        # Handle pipe-separated error types
        for error_type in error_types.split("|"):
            error_type = error_type.strip()

            if error_type.startswith("TemplateError::"):
                variant = error_type.replace("TemplateError::", "")
                template_error_arms.append(
                    f"            TemplateError::{variant}(_) => \"{code}\","
                )
                print(f"  -> {error_type}")
            elif error_type.startswith("ValidationError::"):
                variant = error_type.replace("ValidationError::", "")
                validation_error_arms.append(
                    f"            ValidationError::{variant} {{ .. }} => \"{code}\","
                )
                print(f"  -> {error_type}")

    print("\n" + "=" * 70)
    print("Generated Rust code preview:")
    print("=" * 70)

    print("\n// TemplateError match arms:")
    for arm in template_error_arms:
        print(arm)

    print("\n// ValidationError match arms:")
    for arm in validation_error_arms:
        print(arm)

    # Verify all required fields
    required_fields = ["code", "category", "error_type", "name", "description", "severity"]
    for i, rule in enumerate(rules):
        for field in required_fields:
            if field not in rule:
                print(f"\nERROR: Rule #{i+1} ({rule.get('code', 'unknown')}) missing required field: {field}", file=sys.stderr)
                return 1

    print("\n" + "=" * 70)
    print("✓ All rules validated successfully!")
    print("=" * 70)
    return 0


if __name__ == "__main__":
    sys.exit(main())
