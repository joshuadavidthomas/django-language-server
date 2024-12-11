from __future__ import annotations

import json
import sys
from typing import Any

from .scripts import django_setup
from .scripts import has_import
from .scripts import python_setup


def handle_command(command: str) -> str:
    parts = command.strip().split()
    command = parts[0]
    args = parts[1:] if len(parts) > 1 else []

    if command == "django_setup":
        return json.dumps(django_setup.get_django_setup_info())
    if command == "has_import":
        if not args:
            return "error: Missing module name argument"
        return json.dumps({"can_import": has_import.check_import(args[0])})
    if command == "health":
        return "ok"
    if command == "installed_apps_check":
        import django
        from django.conf import settings

        django.setup()
        if not args:
            return "error: Missing module name argument"
        return json.dumps({"has_app": args[0] in settings.INSTALLED_APPS})
    if command == "python_setup":
        return json.dumps(python_setup.get_python_info())
    if command == "version":
        return "0.1.0"
    return f"Unknown command: {command}"


def handle_json_command(data: dict[str, Any]) -> dict[str, Any]:
    command = data["command"]
    args = data.get("args", [])  # Get args if they exist

    if command == "django_setup":
        import django

        django.setup()
        return {"status": "ok", "data": django_setup.get_django_setup_info()}
    if command == "has_import":
        if not args:
            return {"status": "error", "error": "Missing module name argument"}
        return {
            "status": "ok",
            "data": {"can_import": has_import.check_import(args[0])},
        }
    if command == "health":
        return {"status": "ok"}
    if command == "installed_apps_check":
        import django
        from django.conf import settings

        django.setup()
        if not args:
            return {"status": "error", "error": "Missing module name argument"}
        return {
            "status": "ok",
            "data": {"has_app": args[0] in settings.INSTALLED_APPS},
        }
    if command == "python_setup":
        return {"status": "ok", "data": python_setup.get_python_info()}
    if command == "version":
        return {"status": "ok", "data": "0.1.0"}

    return {"status": "error", "error": f"Unknown command: {command}"}


def main():
    transport_type = sys.stdin.readline().strip()
    print("ready", flush=True)

    while True:
        try:
            line = sys.stdin.readline()
            if not line:
                break

            if transport_type == "json":
                data = json.loads(line)
                response = handle_json_command(data)
                print(json.dumps(response), flush=True)
            else:
                command = line.strip()
                response = handle_command(command)
                print(response, flush=True)

        except Exception as e:
            if transport_type == "json":
                print(json.dumps({"status": "error", "error": str(e)}), flush=True)
            else:
                print(f"error: {str(e)}", flush=True)


if __name__ == "__main__":
    main()
