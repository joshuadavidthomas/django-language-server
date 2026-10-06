from __future__ import annotations

import hashlib
import json
from pathlib import Path

import httpx2
import pytest

from tools import release as release_tool
from tools.release import Artifact
from tools.release import GitHubAsset
from tools.release import Release
from tools.release import ReleaseError


@pytest.mark.parametrize(
    ("tag", "output"),
    [
        ("v1.2.3", "djls 1.2.3"),
        ("v1.2.3a1", "djls 1.2.3-alpha.1"),
        ("v1.2.3b2", "djls 1.2.3-beta.2"),
        ("v1.2.3rc3", "djls 1.2.3-rc.3"),
    ],
)
def test_cli_version_accepts_cargo_equivalent(tag: str, output: str) -> None:
    Release(tag=tag).verify_cli_version(output)


@pytest.mark.parametrize(
    "output",
    [
        "djls 6.1.01",
        "djls 6.1.0 extra",
        "other 6.1.0",
    ],
)
def test_cli_version_rejects_non_exact_match(output: str) -> None:
    with pytest.raises(ReleaseError):
        Release(tag="v6.1.0").verify_cli_version(output)


def test_preflight_rejects_dispatch_from_wrong_ref(monkeypatch) -> None:
    def run(*args: str, capture: bool = False) -> str:
        if not capture:
            return ""
        revision = args[-1]
        if revision in {"refs/tags/v1.2.3^{commit}", "HEAD"}:
            return "release-commit"
        if revision == "dispatch-commit^{commit}":
            return "other-commit"
        raise AssertionError(args)

    monkeypatch.setattr(release_tool, "run", run)

    with pytest.raises(ReleaseError, match="dispatch the workflow from the tag"):
        release_tool.preflight("v1.2.3", "dispatch-commit")


def test_starter_asset_is_removed_before_retry(monkeypatch, tmp_path: Path) -> None:
    archive = tmp_path / "binary-linux-x64" / "example.tar.gz"
    archive.parent.mkdir()
    archive.write_bytes(b"release archive")
    artifact = Artifact.from_archive(archive)
    artifact.write_checksum(archive)

    published: dict[str, bytes] = {}
    stalled: dict[str, GitHubAsset] = {}
    uploads: list[tuple[str, ...]] = []
    deleted: list[int] = []
    interrupt_checksum = True

    def release_assets(_release: Release) -> list[GitHubAsset]:
        uploaded = [
            GitHubAsset(id=index, name=name, state="uploaded", size=len(content))
            for index, (name, content) in enumerate(published.items(), start=1)
        ]
        return [*uploaded, *stalled.values()]

    def download(_release: Release, destination: Path) -> None:
        destination.write_bytes(published[destination.name])

    def upload(_release: Release, *paths: Path) -> None:
        nonlocal interrupt_checksum
        uploads.append(tuple(path.name for path in paths))
        path = paths[0]
        if path == artifact.checksum and interrupt_checksum:
            interrupt_checksum = False
            stalled[path.name] = GitHubAsset(
                id=100,
                name=path.name,
                state="starter",
                size=0,
            )
            raise ReleaseError("simulated checksum upload failure")
        published[path.name] = path.read_bytes()

    def run(*args: str, capture: bool = False) -> str:
        assert not capture
        assert args[:4] == ("gh", "api", "--method", "DELETE")
        asset_id = int(args[4].rsplit("/", 1)[1])
        deleted.append(asset_id)
        stalled.clear()
        return ""

    monkeypatch.setattr(release_tool, "release_assets", release_assets)
    monkeypatch.setattr(release_tool, "download_asset", download)
    monkeypatch.setattr(release_tool, "upload_assets", upload)
    monkeypatch.setattr(release_tool, "binary_artifacts", lambda: [artifact])
    monkeypatch.setattr(release_tool, "run", run)

    with pytest.raises(ReleaseError, match="simulated checksum upload failure"):
        release_tool.github_assets("v1.2.3")

    assert set(published) == {archive.name}
    assert set(stalled) == {artifact.checksum.name}
    release_tool.github_assets("v1.2.3")

    assert deleted == [100]
    assert uploads == [
        (archive.name,),
        (artifact.checksum.name,),
        (artifact.checksum.name,),
    ]
    assert set(published) == artifact.names
    expected = hashlib.sha256(published[archive.name]).hexdigest()
    assert published[artifact.checksum.name].decode().split()[0] == expected


@pytest.mark.parametrize(
    ("tag", "published_tags", "latest_flag"),
    [
        ("v1.9.0", ["v1.10.0"], "--latest=false"),
        ("v2.0.0", ["v1.10.0"], "--latest"),
    ],
)
def test_only_newest_stable_release_is_latest(
    monkeypatch, tag: str, published_tags: list[str], latest_flag: str
) -> None:
    commands: list[tuple[str, ...]] = []

    def run(*args: str, capture: bool = False) -> str:
        if args[:3] == ("gh", "release", "view"):
            return "true"
        if args[:3] == ("gh", "release", "list"):
            return "\n".join(published_tags)
        assert not capture
        commands.append(args)
        return ""

    monkeypatch.setattr(release_tool, "run", run)

    release_tool.github_publish(tag)

    assert commands == [("gh", "release", "edit", tag, "--draft=false", latest_flag)]


def test_pypi_verification_retries_http_errors(monkeypatch, tmp_path: Path) -> None:
    artifact = tmp_path / "wheels-python" / "example.whl"
    artifact.parent.mkdir()
    artifact.touch()
    requests = 0

    def respond(request: httpx2.Request) -> httpx2.Response:
        nonlocal requests
        requests += 1
        if requests == 1:
            return httpx2.Response(503, request=request)
        return httpx2.Response(
            200,
            request=request,
            json={"urls": [{"filename": artifact.name}]},
        )

    client = httpx2.Client(transport=httpx2.MockTransport(respond))
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(release_tool.httpx2, "Client", lambda **_kwargs: client)

    with release_tool.stamina.set_testing(True, attempts=12):
        release_tool.verify_pypi("v1.2.3")

    assert requests == 2


def test_release_assets_includes_drafts(monkeypatch) -> None:
    commands: list[tuple[str, ...]] = []

    def run(*args: str, capture: bool = False) -> str:
        assert capture
        commands.append(args)
        return json.dumps(
            {
                "assets": [
                    {
                        "id": 42,
                        "name": "example.tar.gz",
                        "state": "uploaded",
                        "size": 123,
                    }
                ]
            }
        )

    monkeypatch.setattr(release_tool, "run", run)
    assets = release_tool.release_assets(Release(tag="v6.1.1"))

    assert [asset.name for asset in assets] == ["example.tar.gz"]
    assert commands == [
        (
            "gh",
            "api",
            "--paginate",
            "repos/{owner}/{repo}/releases",
            "--jq",
            '.[] | select(.tag_name == "v6.1.1")',
        )
    ]


def test_release_assets_reports_missing_release(monkeypatch) -> None:
    monkeypatch.setattr(release_tool, "run", lambda *args, **kwargs: "")

    with pytest.raises(ReleaseError, match="GitHub release v6.1.1 was not found"):
        release_tool.release_assets(Release(tag="v6.1.1"))


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("path", ".github/workflows/build.yml"),
        ("head_branch", "main"),
        ("event", "pull_request"),
        ("status", "in_progress"),
    ],
)
def test_recovery_rejects_wrong_source_run(monkeypatch, field: str, value: str) -> None:
    original = {
        "path": ".github/workflows/release.yml",
        "head_branch": "v6.1.1",
        "event": "push",
        "status": "completed",
        "head_sha": "release-commit",
    }
    original[field] = value
    monkeypatch.setattr(
        release_tool, "run", lambda *args, **kwargs: json.dumps(original)
    )

    with pytest.raises(ReleaseError, match="completed release run from the tag"):
        release_tool.recovery_preflight("v6.1.1", 123)


@pytest.mark.parametrize(
    "test_conclusion", ["success", "failure", "cancelled", "skipped"]
)
def test_recovery_requires_successful_checks(monkeypatch, test_conclusion: str) -> None:
    original = {
        "path": ".github/workflows/release.yml",
        "head_branch": "v6.1.1",
        "event": "push",
        "status": "completed",
        "head_sha": "release-commit",
    }
    pages = [
        {"jobs": [{"name": "test / e2e", "conclusion": test_conclusion}]},
        {
            "jobs": [
                {"name": "build / attest", "conclusion": "success"},
                {"name": "release", "conclusion": "failure"},
            ]
        },
    ]
    checked: list[tuple[str, str]] = []

    def run(*args: str, capture: bool = False) -> str:
        assert capture
        return json.dumps(pages if "--slurp" in args else original)

    monkeypatch.setattr(release_tool, "run", run)
    monkeypatch.setattr(
        release_tool, "preflight", lambda tag, sha: checked.append((tag, sha))
    )

    if test_conclusion == "success":
        release_tool.recovery_preflight("v6.1.1", 123)
        assert checked == [("v6.1.1", "release-commit")]
    else:
        with pytest.raises(
            ReleaseError, match="successful release checks and attested builds"
        ):
            release_tool.recovery_preflight("v6.1.1", 123)


def test_recovery_verifies_original_artifact_provenance(
    monkeypatch, tmp_path: Path
) -> None:
    archive = tmp_path / "binary-linux-x64" / "example.tar.gz"
    archive.parent.mkdir()
    archive.write_bytes(b"archive")
    wheel = tmp_path / "wheels-python" / "example.whl"
    wheel.parent.mkdir()
    wheel.write_bytes(b"wheel")
    commands: list[tuple[str, ...]] = []

    def run(*args: str, capture: bool = False) -> str:
        if args[:2] == ("git", "rev-parse"):
            return "release-commit"
        if args[:3] == ("gh", "repo", "view"):
            return "owner/repo"
        commands.append(args)
        return ""

    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(release_tool, "run", run)
    release_tool.verify_provenance("v6.1.1")

    assert len(commands) == 2
    for command in commands:
        assert command[:3] == ("gh", "attestation", "verify")
        assert command[4:] == (
            "--repo",
            "owner/repo",
            "--signer-workflow",
            "owner/repo/.github/workflows/build.yml",
            "--source-digest",
            "release-commit",
            "--source-ref",
            "refs/tags/v6.1.1",
        )
