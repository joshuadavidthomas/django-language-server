#!/usr/bin/env python3
"""
Script to run the end-to-end tests for django-language-server.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


def main():
    """Run the tests."""
    parser = argparse.ArgumentParser(description="Run django-language-server tests")
    parser.add_argument(
        "--python",
        choices=["3.9", "3.10", "3.11", "3.12", "3.13"],
        default=None,
        help="Python version to test with",
    )
    parser.add_argument(
        "--django",
        choices=["4.2", "5.0", "5.1"],
        default=None,
        help="Django version to test with",
    )
    parser.add_argument(
        "--client",
        choices=["vscode", "neovim"],
        default=None,
        help="Client to test with",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all tests (all Python and Django versions)",
    )
    parser.add_argument(
        "test_path",
        nargs="?",
        default=None,
        help="Path to specific test file or directory",
    )
    
    args = parser.parse_args()
    
    # Determine the tox environment
    if args.all:
        # Run all environments
        tox_env = None
    elif args.python and args.django:
        # Run specific Python and Django version
        py_version = args.python.replace(".", "")
        django_version = args.django.replace(".", "")
        tox_env = f"py{py_version}-django{django_version}"
    elif args.python:
        # Run all Django versions for specific Python version
        py_version = args.python.replace(".", "")
        tox_env = f"py{py_version}"
    elif args.django:
        # Run all Python versions for specific Django version
        django_version = args.django.replace(".", "")
        tox_env = f"django{django_version}"
    else:
        # Default to current Python version and Django 5.0
        tox_env = None
    
    # Build the tox command
    tox_cmd = ["tox"]
    if tox_env:
        tox_cmd.extend(["-e", tox_env])
    
    # Add test path if specified
    if args.test_path:
        tox_cmd.append("--")
        tox_cmd.append(args.test_path)
    
    # Add client tests if specified
    if args.client:
        if args.test_path:
            print("Warning: --client and test_path cannot be used together. Ignoring test_path.")
        tox_cmd.append("--")
        tox_cmd.append(f"tests/clients/test_{args.client}.py")
    
    # Run the tests
    try:
        subprocess.run(tox_cmd, check=True)
    except subprocess.CalledProcessError as e:
        sys.exit(e.returncode)


if __name__ == "__main__":
    main()