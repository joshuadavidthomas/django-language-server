#!/usr/bin/env python3
"""
Verification script to check that diagnostic attributes are properly set in Rust source.
This doesn't parse Rust directly but does basic regex matching to verify structure.
"""

import re
import sys
from pathlib import Path


def check_diagnostic_attributes(file_path, expected_count):
    """Check that a Rust file has the expected number of diagnostic attributes."""
    content = file_path.read_text()

    # Find all diagnostic attributes
    pattern = r'#\[diagnostic\(code\s*=\s*"([^"]+)",\s*category\s*=\s*"([^"]+)"\)\]'
    matches = re.findall(pattern, content)

    print(f"\nChecking {file_path.name}:")
    print(f"  Found {len(matches)} diagnostic attributes")

    if len(matches) != expected_count:
        print(f"  ❌ ERROR: Expected {expected_count}, found {len(matches)}")
        return False

    for code, category in matches:
        print(f"    - {code} ({category})")

    return True


def check_doc_comments(file_path, diagnostic_codes):
    """Check that diagnostic codes have associated doc comments."""
    content = file_path.read_text()

    all_good = True
    for code in diagnostic_codes:
        # Look for the code in a diagnostic attribute
        pattern = rf'///.*?#\[diagnostic\(code\s*=\s*"{code}"'
        if not re.search(pattern, content, re.DOTALL):
            print(f"  ❌ ERROR: Code {code} missing doc comments")
            all_good = False

    if all_good:
        print(f"  ✓ All {len(diagnostic_codes)} codes have doc comments")

    return all_good


def main():
    project_root = Path(__file__).parent

    # Check ValidationError (8 distinct codes, but S104 appears twice)
    validation_errors = project_root / "crates" / "djls-semantic" / "src" / "errors.rs"
    if not check_diagnostic_attributes(validation_errors, 9):  # 9 variants total
        return 1

    if not check_doc_comments(validation_errors, ["S100", "S101", "S102", "S103", "S104", "S105", "S106", "S107"]):
        return 1

    # Check TemplateError (3 codes)
    template_errors = project_root / "crates" / "djls-templates" / "src" / "error.rs"
    if not check_diagnostic_attributes(template_errors, 3):
        return 1

    if not check_doc_comments(template_errors, ["T100", "T900", "T901"]):
        return 1

    print("\n" + "="*70)
    print("✓ All diagnostic attributes verified!")
    print("="*70)
    print("\nThe build.rs script will:")
    print("  1. Parse these Rust files with syn")
    print("  2. Extract diagnostic codes and doc comments")
    print("  3. Generate markdown docs in docs/rules/*.md")
    print("\nThe diagnostic codes are manually implemented in:")
    print("  - crates/djls-ide/src/diagnostics.rs")
    print("\nTo test the full build, run: cargo build -p djls-ide")

    return 0


if __name__ == "__main__":
    sys.exit(main())
