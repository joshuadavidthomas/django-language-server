# /// script
# requires-python = ">=3.11"
# dependencies = ["typer>=0.27.0", "pydantic>=2.0,<3"]
# ///
"""Refresh pinned, embedded Django sources.

uv run tools/bundle_django.py
uv run tools/bundle_django.py --check
"""

from __future__ import annotations

import ast
import hashlib
import io
import urllib.request
import zipfile
from pathlib import Path
from typing import Annotated

import typer
from pydantic import BaseModel
from pydantic import ConfigDict
from pydantic import Field
from pydantic import HttpUrl
from pydantic import TypeAdapter
from pydantic import ValidationError

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "crates/djls-project/vendor"
app = typer.Typer(help=__doc__, add_completion=False)


class DjangoRelease(BaseModel):
    model_config = ConfigDict(extra="forbid")

    version: Annotated[str, Field(pattern=r"^\d+\.\d+\.\d+$")]
    url: HttpUrl
    sha256: Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]


Manifest = dict[Annotated[str, Field(pattern=r"^\d+\.\d+$")], DjangoRelease]


def load_manifest() -> Manifest:
    try:
        manifest = TypeAdapter(Manifest).validate_json(
            (DEST / "django.json").read_text()
        )
    except ValidationError as error:
        raise ValueError(f"Invalid Django manifest: {error}") from error
    # Use the same support matrix as README and package metadata, without importing nox.
    tree = ast.parse((ROOT / "noxfile.py").read_text())
    values = {}
    lines = set()
    for node in tree.body:
        if isinstance(node, ast.Assign) and isinstance(node.value, ast.Constant):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    values[target.id] = node.value.value
        if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == "DJ_VERSIONS" for t in node.targets
        ):
            lines = {values[item.id] for item in node.value.elts} - {"main"}
            break
    if set(manifest) != lines:
        raise ValueError("Bundle pins must match DJ_VERSIONS (excluding main)")
    for line, release in manifest.items():
        if release.version.rsplit(".", 1)[0] != line:
            raise ValueError(f"Django {release.version} does not belong to {line}")
    return manifest


def build_bundle(release: DjangoRelease) -> bytes:
    with urllib.request.urlopen(str(release.url)) as response:
        wheel = response.read()
    if hashlib.sha256(wheel).hexdigest() != release.sha256:
        raise ValueError(f"Django {release.version}: wheel SHA-256 mismatch")
    output = io.BytesIO()
    with (
        zipfile.ZipFile(io.BytesIO(wheel)) as source,
        zipfile.ZipFile(
            output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
        ) as archive,
    ):
        for name in sorted(source.namelist()):
            if name.endswith("/"):
                continue
            path = Path(name)
            is_license = ".dist-info" in name and "licenses" in path.parts
            if not is_license and not (
                name.startswith("django/")
                and (name.endswith(".py") or "templates" in path.parts[:-1])
            ):
                continue
            entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.create_system = 3  # Canonical Unix metadata, including on Windows.
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, source.read(name), compresslevel=9)
    return output.getvalue()


@app.command()
def main(
    check: Annotated[
        bool, typer.Option("--check", help="Verify without writing")
    ] = False,
) -> None:
    try:
        for line, release in load_manifest().items():
            data = build_bundle(release)
            destination = DEST / f"django-{line}.zip"
            if check:
                if destination.read_bytes() != data:
                    raise ValueError(f"Stale archive: {destination}")
            else:
                destination.write_bytes(data)
            typer.echo(
                f"Django {release.version}: {len(data):,} bytes ({destination.name})"
            )
    except ValueError as error:
        raise typer.BadParameter(str(error)) from error


if __name__ == "__main__":
    app()
