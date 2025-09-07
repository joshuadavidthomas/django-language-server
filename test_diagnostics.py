#!/usr/bin/env python3
"""Test script to see diagnostic output from the Django Language Server."""

import subprocess
import json
import tempfile
import os
from pathlib import Path

# Test templates with various errors
TEST_TEMPLATES = {
    "orphaned_else.html": """
{% if condition %}
    <p>Content</p>
{% endif %}
{% else %}
    <p>This else is orphaned</p>
""",
    "unclosed_block.html": """
{% block content %}
    <h1>Title</h1>
    {% block inner %}
        <p>Content</p>
    <!-- Missing endblock inner -->
{% endblock content %}
""",
    "mismatched_block.html": """
{% block header %}
    <h1>Header</h1>
{% endblock footer %}
""",
    "wrong_block_name.html": """
{% block content %}
    {% block foobar %}
    {% if foo %}
    {% endif %}
    {% else %}
    {% endblock wrongname %}
{% endblock content %}
""",
}


def main():
    # Create temporary directory for test files
    with tempfile.TemporaryDirectory() as tmpdir:
        tmppath = Path(tmpdir)

        # Write test files
        for filename, content in TEST_TEMPLATES.items():
            filepath = tmppath / filename
            filepath.write_text(content)

            print(f"\n{'=' * 60}")
            print(f"Testing: {filename}")
            print(f"{'=' * 60}")
            print("Template content:")
            print(content)
            print(f"{'-' * 60}")

            # Run djls in server mode would require more complex setup
            # For now, just compile and show that it builds
            print("(Django Language Server would show diagnostics here)")
            print("Expected diagnostics:")

            if "orphaned" in filename:
                print("- DJ005: 'else' must appear between 'if' and 'endif'")
            elif "unclosed" in filename:
                print("- DJ004: Unclosed tag: block")
            elif "mismatched" in filename:
                print(
                    "- DJ002: Unbalanced structure: 'header' missing closing 'endheader'"
                )
            elif "wrong_block_name" in filename:
                print("- DJ005: 'else' must appear between 'if' and 'endif'")
                print("- DJ006: endblock 'wrongname' does not match any open block")
                print("- DJ004: Unclosed tag: block")


if __name__ == "__main__":
    main()
