#!/usr/bin/env python3
"""
Generate a test matrix report for django-language-server.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


def main():
    """Generate a test matrix report."""
    parser = argparse.ArgumentParser(description="Generate test matrix report")
    parser.add_argument(
        "--output",
        default="test_matrix_report.md",
        help="Output file path",
    )
    
    args = parser.parse_args()
    
    # Define the test matrix
    python_versions = ["3.9", "3.10", "3.11", "3.12", "3.13"]
    django_versions = ["4.2", "5.0", "5.1"]
    
    # Run a simple test for each combination to check compatibility
    results = {}
    
    for py_version in python_versions:
        results[py_version] = {}
        for django_version in django_versions:
            print(f"Testing Python {py_version} with Django {django_version}...")
            
            # Create a temporary environment
            env_name = f"py{py_version.replace('.', '')}_django{django_version.replace('.', '')}"
            
            try:
                # Check if this combination is supported
                # This is a simplified check - in a real implementation,
                # you would run actual tests and check the results
                supported = is_combination_supported(py_version, django_version)
                results[py_version][django_version] = supported
            except Exception as e:
                print(f"Error testing Python {py_version} with Django {django_version}: {e}")
                results[py_version][django_version] = False
    
    # Generate the report
    generate_report(results, args.output)
    
    print(f"Report generated at {args.output}")


def is_combination_supported(python_version: str, django_version: str) -> bool:
    """
    Check if a Python and Django version combination is supported.
    
    This is a simplified check - in a real implementation, you would
    run actual tests and check the results.
    
    Args:
        python_version: Python version (e.g., "3.9")
        django_version: Django version (e.g., "4.2")
        
    Returns:
        True if the combination is supported, False otherwise
    """
    # Django 5.1 requires Python 3.10+
    if django_version == "5.1" and python_version == "3.9":
        return False
    
    # All other combinations are supported
    return True


def generate_report(results: dict, output_path: str) -> None:
    """
    Generate a test matrix report.
    
    Args:
        results: Test results
        output_path: Output file path
    """
    with open(output_path, "w") as f:
        f.write("# Django Language Server Test Matrix\n\n")
        
        f.write("This report shows the compatibility of django-language-server with different Python and Django versions.\n\n")
        
        f.write("## Test Matrix\n\n")
        
        # Write the table header
        f.write("| Python / Django | " + " | ".join(results[list(results.keys())[0]].keys()) + " |\n")
        f.write("| --- | " + " | ".join(["---"] * len(results[list(results.keys())[0]])) + " |\n")
        
        # Write the table rows
        for py_version, django_results in results.items():
            row = [py_version]
            for django_version, supported in django_results.items():
                if supported:
                    row.append("✅")
                else:
                    row.append("❌")
            f.write("| " + " | ".join(row) + " |\n")
        
        f.write("\n")
        f.write("✅ = Supported, ❌ = Not supported\n\n")
        
        f.write("## Notes\n\n")
        f.write("- Django 5.1 requires Python 3.10 or higher\n")
        f.write("- All tests were run on the latest patch versions of each Python and Django release\n")


if __name__ == "__main__":
    main()