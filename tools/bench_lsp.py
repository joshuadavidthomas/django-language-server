"""Measure native stdio LSP latency, independently of in-process Divan benches."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import statistics
import tempfile
import time
from pathlib import Path


def change(start: tuple[int, int], end: tuple[int, int], text: str) -> dict:
    return {
        "range": {
            "start": {"line": start[0], "character": start[1]},
            "end": {"line": end[0], "character": end[1]},
        },
        "text": text,
    }


def fixture_workload(count: int) -> tuple[str, list[dict], list[dict]]:
    source = '  <p>Café 😀 {{ value|default:"—" }}</p>\r\n' * 512
    source += "Café 😀 {% benchmark_unknown %}"
    forward = [change((128, 0), (128, 0), "préface 😀\n")]
    reverse = []
    for index in range(count - 2):
        row = 129 + index * 10
        forward.append(change((row, 16), (row, 21), "product"))
        reverse.append(change((row, 16), (row, 23), "value"))
    if count > 1:
        forward.append(change((513, 8), (513, 31), "{% benchmark_changed %}"))
        reverse.insert(0, change((513, 8), (513, 31), "{% benchmark_unknown %}"))
    reverse.append(change((128, 0), (129, 0), ""))
    return source, forward, reverse


def template_workload(template: Path) -> tuple[str, list[dict], list[dict]]:
    source = template.read_bytes().decode("utf-8")
    lines = source.splitlines()
    candidates = [
        (row, line)
        for row, line in enumerate(lines)
        if line.strip()
        and not any(token in line for token in ("{%", "{{", "{#"))
        and line.lstrip().startswith("<")
        and not line.lstrip().startswith(("<!--", "<script", "<style"))
    ]
    if not candidates:
        raise ValueError("template has no markup-only line to edit")
    row, line = min(candidates, key=lambda item: abs(item[0] - len(lines) // 3))
    column = len(line.encode("utf-16-le")) // 2
    return (
        source,
        [change((row, column), (row, column), "x")],
        [change((row, column), (row, column + 1), "")],
    )


class Client:
    def __init__(self, process: asyncio.subprocess.Process):
        self.process = process
        self.request_id = 0

    async def send(self, message: dict) -> None:
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        await self.process.stdin.drain()

    async def receive(self) -> dict:
        headers = {}
        while (line := await self.process.stdout.readline()) != b"\r\n":
            if not line:
                raise RuntimeError("server closed stdout before a complete LSP message")
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        return json.loads(
            await self.process.stdout.readexactly(int(headers["content-length"]))
        )

    async def until(self, predicate) -> dict:
        while True:
            message = await self.receive()
            if "method" in message and "id" in message:
                # Registration/progress/configuration requests must not deadlock startup.
                result = None
                if message["method"] == "workspace/configuration":
                    result = [None] * len(message.get("params", {}).get("items", []))
                await self.send({"id": message["id"], "result": result})
            elif predicate(message):
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message

    async def request(self, method: str, params=None) -> dict:
        self.request_id += 1
        request_id = self.request_id
        await self.send({"id": request_id, "method": method, "params": params})
        return await self.until(lambda message: message.get("id") == request_id)

    async def diagnostics(self, uri: str, version: int) -> list[dict]:
        message = await self.until(
            lambda message: (
                message.get("method") == "textDocument/publishDiagnostics"
                and message.get("params", {}).get("uri") == uri
                and message["params"].get("version") == version
            )
        )
        return message["params"]["diagnostics"]


async def sample(args, root: Path, template: Path, options: dict, workload) -> dict:
    source, forward, reverse = workload
    uri = template.as_uri()
    with tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        process = await asyncio.create_subprocess_exec(
            str(args.binary),
            "serve",
            "--connection-type",
            "stdio",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=stderr,
            cwd=root,
        )
        client = Client(process)
        try:
            result = await client.request(
                "initialize",
                {
                    "processId": None,
                    "workspaceFolders": [{"uri": root.as_uri(), "name": "benchmark"}],
                    "initializationOptions": options,
                    "capabilities": {
                        "general": {"positionEncodings": ["utf-16"]},
                        "textDocument": {
                            "publishDiagnostics": {"versionSupport": True}
                        },
                    },
                },
            )
            encoding = result["result"]["capabilities"].get(
                "positionEncoding", "utf-16"
            )
            if encoding != "utf-16":
                raise RuntimeError(f"server negotiated {encoding}, expected utf-16")
            await client.send({"method": "initialized", "params": {}})
            await client.send(
                {
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "htmldjango",
                            "version": 1,
                            "text": source,
                        }
                    },
                }
            )
            initial = await client.diagnostics(uri, 1)
            cold_ns = time.perf_counter_ns() - started
            if args.template is None:
                verify_fixture(initial, 512, "benchmark_unknown")

            elapsed = []
            inserted = None
            for iteration in range(args.edits + 2):
                modified = iteration % 2 == 0
                version = iteration + 2
                message = {
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": {"uri": uri, "version": version},
                        "contentChanges": forward if modified else reverse,
                    },
                }
                started = time.perf_counter_ns()
                await client.send(message)
                diagnostics = await client.diagnostics(uri, version)
                duration = time.perf_counter_ns() - started
                if args.template is None:
                    name = (
                        "benchmark_changed"
                        if modified and args.changes > 1
                        else "benchmark_unknown"
                    )
                    verify_fixture(diagnostics, 513 if modified else 512, name)
                elif modified:
                    if inserted is None:
                        inserted = diagnostics
                    elif diagnostics != inserted:
                        raise RuntimeError(
                            "insert-state diagnostics changed between toggles"
                        )
                elif diagnostics != initial:
                    raise RuntimeError(
                        "delete-state diagnostics differ from initial output"
                    )
                # Warm both directions before recording notification latency.
                if iteration >= 2:
                    elapsed.append(duration)
            await client.request("shutdown")
            await client.send({"method": "exit"})
            await process.wait()
            if process.returncode != 0:
                raise RuntimeError(f"server exited with status {process.returncode}")
            return {
                "cold_start_ns": cold_ns,
                "did_change_ns": elapsed,
                "initial_diagnostics": initial,
                "inserted_diagnostics": inserted,
                "server_info": result["result"].get("serverInfo"),
            }
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()


def verify_fixture(diagnostics: list[dict], row: int, name: str) -> None:
    if (
        len(diagnostics) != 1
        or str(diagnostics[0].get("code")) != "S108"
        or diagnostics[0]["range"]["start"]["line"] != row
        or diagnostics[0]["range"]["start"]["character"] != 8
        or name not in diagnostics[0]["message"]
    ):
        raise RuntimeError(f"incorrect sequential-edit diagnostics: {diagnostics}")


async def run(args) -> dict:
    with tempfile.TemporaryDirectory(prefix="djls-lsp-bench-") as directory:
        root = args.project_root or Path(directory)
        if args.template is None:
            template = root / "templates" / "benchmark.html"
            (root / "settings.py").write_text(
                "INSTALLED_APPS = []\n"
                "TEMPLATES = [{'BACKEND': 'django.template.backends.django.DjangoTemplates', "
                "'DIRS': ['templates'], 'APP_DIRS': True}]\n"
            )
            options = {
                "django_settings_module": "settings",
                "django_version": "5.2",
                "venv_path": str(root / "no-venv"),
            }
            workload = fixture_workload(args.changes)
        else:
            template = args.template
            options = json.loads(args.initialization_options)
            workload = template_workload(template)
        samples = []
        for _ in range(args.samples):
            samples.append(
                await asyncio.wait_for(
                    sample(args, root, template, options, workload),
                    timeout=args.timeout,
                )
            )
        if any(
            s["initial_diagnostics"] != samples[0]["initial_diagnostics"]
            for s in samples
        ):
            raise RuntimeError("cold-start diagnostic output differs between samples")
        return {
            "schema_version": 1,
            "workload": "real_template" if args.template else "sequential_unicode_crlf",
            "source_sha256": hashlib.sha256(workload[0].encode()).hexdigest(),
            "changes_sha256": hashlib.sha256(
                json.dumps(workload[1:], sort_keys=True).encode()
            ).hexdigest(),
            "position_encoding": "utf-16",
            "changes_per_notification": len(workload[1]),
            "samples": samples,
            "median_cold_start_ns": statistics.median(
                s["cold_start_ns"] for s in samples
            ),
            "median_did_change_ns": statistics.median(
                value for sample in samples for value in sample["did_change_ns"]
            ),
        }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=lambda path: Path(path).resolve(), required=True
    )
    parser.add_argument("--project-root", type=lambda path: Path(path).resolve())
    parser.add_argument("--template", type=lambda path: Path(path).resolve())
    parser.add_argument(
        "--initialization-options", default="{}", help="LSP options as JSON"
    )
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--edits", type=int, default=20)
    parser.add_argument("--changes", type=int, choices=(1, 8, 32), default=8)
    parser.add_argument(
        "--timeout", type=float, default=60, help="seconds per server process"
    )
    args = parser.parse_args()
    if bool(args.project_root) != bool(args.template):
        parser.error("--project-root and --template must be supplied together")
    if args.samples < 1 or args.edits < 1:
        parser.error("--samples and --edits must be positive")
    print(json.dumps(asyncio.run(run(args)), indent=2))


if __name__ == "__main__":
    main()
