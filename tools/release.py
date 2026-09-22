# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx2>=2.0", "stamina>=26.1.0", "typer>=0.27.0", "pydantic>=2.0,<3", "tomli>=2.0; python_version < '3.11'"]
# ///
"""Check and run a django-language-server release."""

from __future__ import annotations

import glob
import hashlib
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Literal, TypeVar

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

import httpx2
import stamina
import typer
from pydantic import AliasPath
from pydantic import BaseModel
from pydantic import ConfigDict
from pydantic import Field
from pydantic import ValidationError
from pydantic import model_validator

ROOT = Path(__file__).resolve().parents[1]

PythonVersion = Annotated[
    str,
    Field(pattern=r"^\d+\.\d+\.\d+(?:(?:a|b|rc)\d+)?$"),
]
CargoVersion = Annotated[
    str,
    Field(
        pattern=r"^\d+\.\d+\.\d+(?:-(?:alpha|beta|a|b|rc)(?:\.?\d+)?)?$"
    ),
]
ReleaseTag = Annotated[
    str,
    Field(pattern=r"^v\d+\.\d+\.\d+(?:(?:a|b|rc)\d+)?$"),
]


class ReleaseError(Exception):
    pass


class DjlsVersion(BaseModel):
    version: CargoVersion


def normalize_cargo_version(version: str) -> str:
    match = re.fullmatch(
        r"(?P<base>\d+\.\d+\.\d+)(?:-(?P<tag>alpha|beta|a|b|rc)(?:\.?(?P<num>\d+))?)?",
        version,
    )
    if match is None:
        return version
    if match["tag"] is None:
        return match["base"]
    tag = {"alpha": "a", "beta": "b"}.get(match["tag"], match["tag"])
    return f"{match['base']}{tag}{match['num'] or '0'}"


def parse_djls_version(output: str) -> str:
    match = re.fullmatch(r"djls (?P<version>\S+)\s*", output)
    if match is None:
        raise ValueError(f"Unexpected djls version output: {output!r}")
    version = DjlsVersion(version=match["version"])
    return normalize_cargo_version(version.version)


class Release(BaseModel):
    model_config = ConfigDict(frozen=True)

    tag: ReleaseTag

    @property
    def version(self) -> str:
        return self.tag.removeprefix("v")

    @property
    def is_prerelease(self) -> bool:
        return bool(re.search("[a-zA-Z]", self.version))

    @property
    def standalone_archive(self) -> str:
        return f"django-language-server-{self.tag}-linux-x64.tar.gz"

    @property
    def version_tuple(self) -> tuple[int, int, int]:
        major, minor, patch = (int(part) for part in self.version.split("."))
        return major, minor, patch

    def verify_cli_version(self, output: str) -> None:
        try:
            reported = parse_djls_version(output)
        except (ValueError, ValidationError) as error:
            raise ReleaseError(str(error)) from error
        if reported != self.version:
            raise ReleaseError(
                f"djls reported version {reported!r}, expected {self.version!r}"
            )


class LockPackage(BaseModel):
    name: str
    version: str


class Pyproject(BaseModel):
    project_version: PythonVersion = Field(
        validation_alias=AliasPath("project", "version")
    )
    bumpver_version: CargoVersion = Field(
        validation_alias=AliasPath("tool", "bumpver", "current_version")
    )


class CargoManifest(BaseModel):
    version: CargoVersion = Field(validation_alias=AliasPath("package", "version"))


class Lockfile(BaseModel):
    package: list[LockPackage]


ModelT = TypeVar("ModelT", bound=BaseModel)


def load_toml(path: str, model: type[ModelT]) -> ModelT:
    with (ROOT / path).open("rb") as source:
        return model.model_validate(tomllib.load(source))


def package_version(document: Lockfile, name: str) -> str | None:
    return next(
        (
            package.version
            for package in document.package
            if package.name == name
        ),
        None,
    )


class ReleaseMetadata(BaseModel):
    model_config = ConfigDict(frozen=True)

    tag: ReleaseTag
    project_version: PythonVersion
    bumpver_version: CargoVersion
    cargo_version: CargoVersion
    cargo_lock_version: CargoVersion | None
    uv_lock_version: PythonVersion | None
    changelog: str
    installation: str
    pre_commit: str

    @classmethod
    def load(cls, tag: str | None) -> ReleaseMetadata:
        pyproject = load_toml("pyproject.toml", Pyproject)
        cargo = load_toml("crates/djls/Cargo.toml", CargoManifest)
        return cls(
            tag=tag or f"v{pyproject.project_version}",
            project_version=pyproject.project_version,
            bumpver_version=pyproject.bumpver_version,
            cargo_version=cargo.version,
            cargo_lock_version=package_version(
                load_toml("Cargo.lock", Lockfile), "djls"
            ),
            uv_lock_version=package_version(
                load_toml("uv.lock", Lockfile), "django-language-server"
            ),
            changelog=(ROOT / "CHANGELOG.md").read_text(),
            installation=(ROOT / "docs/installation.md").read_text(),
            pre_commit=(ROOT / "docs/pre-commit.md").read_text(),
        )

    @property
    def version(self) -> str:
        return self.tag.removeprefix("v")

    @model_validator(mode="after")
    def versions_match(self) -> ReleaseMetadata:
        versions = {
            "Python project": self.project_version,
            "bumpver": normalize_cargo_version(self.bumpver_version),
            "Cargo package": normalize_cargo_version(self.cargo_version),
            "Cargo.lock": normalize_cargo_version(
                self.cargo_lock_version or "missing"
            ),
            "uv.lock": self.uv_lock_version or "missing",
        }
        errors = [
            f"{name} version is {actual!r}, expected {self.version!r}"
            for name, actual in versions.items()
            if actual != self.version
        ]
        if errors:
            raise ValueError("; ".join(errors))
        return self

    def content_errors(self, require_empty_unreleased: bool) -> list[str]:
        heading = f"## [{self.version}]"
        unreleased_heading = "## [Unreleased]"
        required_text = {
            "CHANGELOG.md": (
                self.changelog,
                [
                    unreleased_heading,
                    heading,
                    (
                        f"[unreleased]: https://github.com/joshuadavidthomas/"
                        f"django-language-server/compare/{self.tag}...HEAD"
                    ),
                    (
                        f"[{self.version}]: https://github.com/joshuadavidthomas/"
                        f"django-language-server/releases/tag/{self.tag}"
                    ),
                ],
            ),
            "docs/installation.md": (
                self.installation,
                [
                    f'VERSION="{self.version}"',
                    f"django-language-server-v{self.version}-windows-x64.zip",
                    f"releases/download/v{self.version}/$Archive",
                    "$Directory = [IO.Path]::GetFileNameWithoutExtension($Archive)",
                    'Join-Path $Directory "djls.exe"',
                ],
            ),
            "docs/pre-commit.md": (self.pre_commit, [f"rev: {self.tag}"]),
        }
        errors = [
            f"{path} is missing required text: {text}"
            for path, (content, expected) in required_text.items()
            for text in expected
            if text not in content
        ]
        if (
            require_empty_unreleased
            and unreleased_heading in self.changelog
            and heading in self.changelog
        ):
            unreleased = self.changelog.split(unreleased_heading, 1)[1]
            unreleased = unreleased.split(heading, 1)[0]
            if unreleased.strip():
                errors.append("CHANGELOG.md [Unreleased] section is not empty")
        return errors


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


class Checksum(BaseModel):
    digest: Annotated[str, Field(pattern=r"^[0-9a-fA-F]{64}$")]


@dataclass(frozen=True)
class Artifact:
    archive: Path
    checksum: Path

    @classmethod
    def from_archive(cls, archive: Path) -> Artifact:
        return cls(archive, Path(f"{archive}.sha256"))

    @property
    def names(self) -> set[str]:
        return {self.archive.name, self.checksum.name}

    def verify(self) -> None:
        fields = self.checksum.read_text().split(maxsplit=1)
        expected = Checksum(digest=fields[0] if fields else "").digest
        actual = sha256(self.archive)
        if actual != expected:
            raise ReleaseError(
                f"SHA-256 mismatch for {self.archive.name}: "
                f"expected {expected}, got {actual}"
            )
        typer.echo(f"{self.archive.name}: OK")

    def write_checksum(self, source: Path) -> None:
        self.checksum.write_text(f"{sha256(source)}  {self.archive.name}\n")


class PyPIFile(BaseModel):
    filename: str


class PyPIResponse(BaseModel):
    urls: list[PyPIFile]


class GitHubAsset(BaseModel):
    id: int
    name: str
    state: Literal["open", "starter", "uploaded"]
    size: Annotated[int, Field(ge=0)]

    @property
    def uploaded(self) -> bool:
        return self.state == "uploaded" and self.size > 0


class GitHubRelease(BaseModel):
    assets: list[GitHubAsset]


def run(*args: str, capture: bool = False) -> str:
    process = subprocess.run(
        args,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    )
    return process.stdout.strip() if process.stdout else ""


def binary_artifacts() -> list[Artifact]:
    artifacts = sorted(
        (
            Artifact.from_archive(Path(path))
            for pattern in ("binary-*/*.tar.gz", "binary-*/*.zip")
            for path in glob.glob(pattern)
        ),
        key=lambda artifact: artifact.archive,
    )
    if not artifacts:
        raise ReleaseError("No standalone release archives were downloaded")
    return artifacts


def release_assets(release: Release) -> list[GitHubAsset]:
    output = run(
        "gh",
        "api",
        f"repos/{{owner}}/{{repo}}/releases/tags/{release.tag}",
        capture=True,
    )
    return GitHubRelease.model_validate_json(output).assets


def download_asset(release: Release, destination: Path) -> None:
    run(
        "gh",
        "release",
        "download",
        release.tag,
        "--pattern",
        destination.name,
        "--dir",
        str(destination.parent),
    )


def upload_assets(release: Release, *paths: Path) -> None:
    run("gh", "release", "upload", release.tag, *(str(path) for path in paths))


def extract(archive: Path, destination: Path) -> None:
    with tarfile.open(archive) as source:
        root = destination.resolve()
        for member in source.getmembers():
            path = (root / member.name).resolve()
            outside = path != root and root not in path.parents
            if outside or member.issym() or member.islnk():
                raise ReleaseError(
                    f"Archive {archive.name} contains an unsafe member: {member.name}"
                )
        source.extractall(destination)


app = typer.Typer(help=__doc__, add_completion=False)


@app.command()
def check(
    tag: Annotated[
        str | None,
        typer.Argument(
            help="Expected release tag, such as v6.1.0 (defaults to the package version)."
        ),
    ] = None,
    release: Annotated[
        bool,
        typer.Option("--release", help="Require an empty Unreleased section."),
    ] = False,
) -> None:
    try:
        metadata = ReleaseMetadata.load(tag)
    except ValidationError as error:
        typer.echo(f"Release metadata is invalid:\n{error}", err=True)
        raise typer.Exit(1) from error
    errors = metadata.content_errors(require_empty_unreleased=release)
    if errors:
        typer.echo("Release metadata is inconsistent:", err=True)
        for error in errors:
            typer.echo(f"- {error}", err=True)
        raise typer.Exit(1)

    typer.echo(f"Release metadata is consistent for {metadata.tag}")


@app.command()
def preflight(tag: str, workflow_sha: str) -> None:
    release = Release(tag=tag)
    run("git", "fetch", "--no-tags", "origin", "main:refs/remotes/origin/main")
    commit = run(
        "git",
        "rev-parse",
        "--verify",
        f"refs/tags/{release.tag}^{{commit}}",
        capture=True,
    )
    if commit != run("git", "rev-parse", "HEAD", capture=True):
        raise ReleaseError(f"Checkout does not match release tag {release.tag}")
    workflow_commit = run(
        "git", "rev-parse", "--verify", f"{workflow_sha}^{{commit}}", capture=True
    )
    if commit != workflow_commit:
        raise ReleaseError(
            f"Workflow ref does not match release tag {release.tag}; "
            "dispatch the workflow from the tag"
        )
    if subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, "origin/main"]
    ).returncode:
        raise ReleaseError(f"Release tag {release.tag} is not on main")


@app.command("verify-binaries")
def verify_binaries() -> None:
    for artifact in binary_artifacts():
        artifact.verify()


@app.command("github-draft")
def github_draft(tag: str) -> None:
    release = Release(tag=tag)
    exists = subprocess.run(
        ["gh", "release", "view", release.tag],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if exists.returncode == 0:
        return
    args = [
        "gh",
        "release",
        "create",
        release.tag,
        "--draft",
        "--generate-notes",
        "--verify-tag",
    ]
    if release.is_prerelease:
        args.append("--prerelease")
    run(*args)


@app.command("github-assets")
def github_assets(tag: str) -> None:
    release = Release(tag=tag)
    artifacts = binary_artifacts()
    expected = set().union(*(artifact.names for artifact in artifacts))
    assets = release_assets(release)
    for asset in assets:
        if asset.name in expected and not asset.uploaded:
            run(
                "gh",
                "api",
                "--method",
                "DELETE",
                f"repos/{{owner}}/{{repo}}/releases/assets/{asset.id}",
            )
    attached = {asset.name for asset in assets if asset.uploaded}
    for artifact in artifacts:
        has_archive = artifact.archive.name in attached
        has_checksum = artifact.checksum.name in attached
        with tempfile.TemporaryDirectory() as temporary:
            downloads = Path(temporary)
            published = Artifact(
                downloads / artifact.archive.name,
                downloads / artifact.checksum.name,
            )
            archive = artifact.archive
            if has_archive:
                download_asset(release, published.archive)
                archive = published.archive
            if has_checksum:
                download_asset(release, published.checksum)
                Artifact(archive, published.checksum).verify()
            else:
                artifact.write_checksum(archive)

            if not has_archive:
                upload_assets(release, artifact.archive)
            if not has_checksum:
                upload_assets(release, artifact.checksum)

    missing = expected - {
        asset.name for asset in release_assets(release) if asset.uploaded
    }
    if missing:
        raise ReleaseError(f"GitHub release is missing assets: {sorted(missing)}")


@app.command("verify-pypi")
def verify_pypi(tag: str) -> None:
    release = Release(tag=tag)
    expected = {Path(path).name for path in glob.glob("wheels-*/*")}
    if not expected:
        raise ReleaseError("No Python release artifacts were downloaded")
    url = f"https://pypi.org/pypi/django-language-server/{release.version}/json"
    try:
        with httpx2.Client(timeout=10) as client:
            for attempt in stamina.retry_context(
                on=(httpx2.HTTPError, ReleaseError),
                attempts=12,
                wait_initial=10,
                wait_max=10,
                wait_jitter=0,
            ):
                with attempt:
                    response = client.get(url)
                    response.raise_for_status()
                    published = {
                        item.filename
                        for item in PyPIResponse.model_validate(response.json()).urls
                    }
                    missing = expected - published
                    if missing:
                        raise ReleaseError(
                            f"PyPI is missing release artifacts: {sorted(missing)}"
                        )
    except httpx2.HTTPError as error:
        raise ReleaseError(f"Could not fetch PyPI release metadata: {error}") from error
    typer.echo(f"PyPI contains all {len(expected)} release artifacts")


@app.command("smoke-pypi")
def smoke_pypi(tag: str) -> None:
    release = Release(tag=tag)
    output = run(
        "uvx",
        "--from",
        f"django-language-server=={release.version}",
        "djls",
        "--version",
        capture=True,
    )
    release.verify_cli_version(output)


@app.command("smoke-standalone")
def smoke_standalone(tag: str) -> None:
    release = Release(tag=tag)
    name = release.standalone_archive
    with tempfile.TemporaryDirectory(dir=os.environ.get("RUNNER_TEMP")) as temporary:
        destination = Path(temporary)
        artifact = Artifact(
            destination / name,
            destination / f"{name}.sha256",
        )
        download_asset(release, artifact.archive)
        download_asset(release, artifact.checksum)
        artifact.verify()
        extract(artifact.archive, destination)
        binary = destination / name.removesuffix(".tar.gz") / "djls"
        release.verify_cli_version(run(str(binary), "--version", capture=True))


@app.command("github-publish")
def github_publish(tag: str) -> None:
    release = Release(tag=tag)
    is_draft = run(
        "gh",
        "release",
        "view",
        release.tag,
        "--json",
        "isDraft",
        "--jq",
        ".isDraft",
        capture=True,
    )
    if is_draft != "true":
        return
    args = ["gh", "release", "edit", release.tag, "--draft=false"]
    if release.is_prerelease:
        args.append("--prerelease")
    else:
        output = run(
            "gh",
            "release",
            "list",
            "--exclude-drafts",
            "--exclude-pre-releases",
            "--limit",
            "100",
            "--json",
            "tagName",
            "--jq",
            ".[].tagName",
            capture=True,
        )
        newer_exists = any(
            Release(tag=tag).version_tuple > release.version_tuple
            for tag in output.splitlines()
        )
        args.append("--latest=false" if newer_exists else "--latest")
    run(*args)


if __name__ == "__main__":
    try:
        app()
    except (
        OSError,
        ReleaseError,
        subprocess.CalledProcessError,
        ValidationError,
    ) as error:
        typer.echo(f"Release failed: {error}", err=True)
        sys.exit(1)
