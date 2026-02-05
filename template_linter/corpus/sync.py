from __future__ import annotations

import argparse
import ast
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path
from typing import Any

from .utils import infer_sdist_version
from .utils import to_pip_requirement


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Download and extract a third-party templatetags corpus "
            "(sdists and git repos)."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path(__file__).with_name("manifest.toml"),
        help="Path to manifest.toml",
    )
    parser.add_argument(
        "--root-dir",
        type=Path,
        default=None,
        help="Override corpus root directory (defaults to manifest's root_dir)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Re-download and re-extract even if a package already exists.",
    )
    parser.add_argument(
        "--keep-sdists",
        action="store_true",
        help="Keep downloaded sdists under .corpus/_sdists.",
    )
    parser.add_argument(
        "--write-runtime-registry",
        action="store_true",
        default=True,
        help=(
            "Write `.runtime_registry.json` files into corpus entries. "
            "This is used by strict-unknown corpus validation."
        ),
    )
    parser.add_argument(
        "--no-write-runtime-registry",
        action="store_false",
        dest="write_runtime_registry",
        help="Disable writing `.runtime_registry.json` files into corpus entries.",
    )
    args = parser.parse_args()

    manifest = _load_manifest(args.manifest)
    # The corpus is a project-level artifact stored under template_linter/.corpus/.
    project_root = args.manifest.resolve().parents[1]
    corpus_root = (
        args.root_dir
        if args.root_dir is not None
        else Path(manifest["corpus"]["root_dir"])
    )
    corpus_root = (project_root / corpus_root).resolve()
    sdists_dir = corpus_root / "_sdists"
    packages_dir = corpus_root / "packages"
    repos_dir = corpus_root / "repos"
    sdists_dir.mkdir(parents=True, exist_ok=True)
    packages_dir.mkdir(parents=True, exist_ok=True)
    repos_dir.mkdir(parents=True, exist_ok=True)

    uv_cache_dir = project_root / ".uv_cache"
    pip_cache_dir = project_root / ".pip_cache"
    uv_cache_dir.mkdir(parents=True, exist_ok=True)
    pip_cache_dir.mkdir(parents=True, exist_ok=True)

    packages = manifest.get("package", [])
    repos = manifest.get("repo", [])
    if not packages and not repos:
        raise SystemExit("manifest.toml has no [[package]] or [[repo]] entries")

    entries_for_runtime_registry: list[Path] = []

    for pkg in packages:
        name = pkg["name"]
        version = pkg["version"]
        out_dir = packages_dir / name / version
        marker = out_dir / ".complete.json"

        if out_dir.exists() and marker.exists() and not args.force:
            entries_for_runtime_registry.append(out_dir)
            continue

        if out_dir.exists() and args.force:
            shutil.rmtree(out_dir)

        out_dir.mkdir(parents=True, exist_ok=True)

        # Download deterministically (no deps).
        #
        # Prefer wheels when available to avoid PEP517 build isolation for sdists
        # that use build backends requiring a newer Rust toolchain.
        req = to_pip_requirement(name, version)
        _download_distribution_archive(
            sdists_dir,
            req,
            uv_cache_dir=uv_cache_dir,
            pip_cache_dir=pip_cache_dir,
        )

        archive_path = _find_distribution_archive(sdists_dir, name)
        if archive_path is None:
            raise SystemExit(
                f"Could not find downloaded distribution archive for {req} in {sdists_dir}"
            )
        resolved_version = infer_sdist_version(name, archive_path) or version

        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            extracted = tmpdir / "extracted"
            extracted.mkdir(parents=True, exist_ok=True)
            _extract_archive(archive_path, extracted)

            root = _single_top_level_dir(extracted) or extracted

            # Copy relevant files into the corpus entry.
            _copy_license_and_metadata(root, out_dir)
            _copy_glob(root, out_dir, "**/templatetags/**/*.py")
            _copy_glob(root, out_dir, "**/templates/**/*")
            _copy_python_for_templatetags_roots(root, out_dir)
            # For version-aware Django corpus entries we need the core template
            # tag/filter source files (not just `templatetags/` modules).
            _copy_glob(root, out_dir, "**/template/defaulttags.py")
            _copy_glob(root, out_dir, "**/template/loader_tags.py")
            _copy_glob(root, out_dir, "**/template/defaultfilters.py")
            _copy_glob(root, out_dir, "**/template/base.py")

        marker.write_text(
            json.dumps(
                {
                    "name": name,
                    "version": version,
                    "resolved_version": resolved_version,
                    "archive": archive_path.name,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n"
        )
        entries_for_runtime_registry.append(out_dir)

    for repo in repos:
        name = repo["name"]
        url = repo["url"]
        ref = _sanitize_ref(repo.get("ref", "HEAD"))
        out_dir = repos_dir / name / ref
        marker = out_dir / ".complete.json"

        if out_dir.exists() and marker.exists() and not args.force:
            entries_for_runtime_registry.append(out_dir)
            continue

        if out_dir.exists() and args.force:
            shutil.rmtree(out_dir)

        out_dir.mkdir(parents=True, exist_ok=True)

        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            clone_dir = tmpdir / "repo"
            _clone_repo(url, ref, clone_dir)
            commit = _git_head_commit(clone_dir)

            # Copy relevant files into the corpus entry.
            _copy_license_and_metadata(clone_dir, out_dir)
            _copy_glob(clone_dir, out_dir, "**/templatetags/**/*.py")
            _copy_glob(clone_dir, out_dir, "**/templates/**/*")
            _copy_python_for_templatetags_roots(clone_dir, out_dir)

        marker.write_text(
            json.dumps(
                {
                    "name": name,
                    "url": url,
                    "ref": ref,
                    "commit": commit,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n"
        )
        entries_for_runtime_registry.append(out_dir)

    if args.write_runtime_registry:
        # (Re)generate runtime registries only after the full corpus is present
        # so cross-entry `{% load %}` dependency mapping can see all providers.
        _GLOBAL_LIBRARY_PROVIDERS_CACHE.pop(corpus_root, None)
        for entry in sorted(set(entries_for_runtime_registry)):
            _write_runtime_registry(entry)

    if not args.keep_sdists:
        shutil.rmtree(sdists_dir, ignore_errors=True)

    return 0


def _load_manifest(path: Path) -> dict[str, Any]:
    import tomllib

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if "corpus" not in data or "root_dir" not in data["corpus"]:
        raise SystemExit("manifest.toml must contain [corpus] root_dir = ...")
    return data


def _run(cmd: list[str], *, env: dict[str, str] | None = None) -> None:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    subprocess.run(cmd, check=True, env=merged)


def _sanitize_ref(ref: str) -> str:
    # Keep refs filesystem-friendly (tags/branches can contain slashes).
    return ref.replace("/", "__")


def _clone_repo(url: str, ref: str, dest: Path) -> None:
    """
    Clone a git repo at a specific ref into dest.

    We prefer a shallow fetch for speed, and avoid keeping a working clone
    in the corpus (we only copy out relevant files).
    """
    # Use git plumbing so `ref` can be a branch, tag, or commit.
    _run(["git", "init", str(dest)])
    _run(["git", "-C", str(dest), "remote", "add", "origin", url])
    _run(["git", "-C", str(dest), "fetch", "--depth", "1", "origin", ref])
    _run(["git", "-C", str(dest), "checkout", "--detach", "FETCH_HEAD"])


def _git_head_commit(repo_dir: Path) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repo_dir), "rev-parse", "HEAD"], text=True
    ).strip()


def _find_distribution_archive(sdists_dir: Path, name: str) -> Path | None:
    """
    Find the most recently downloaded distribution archive for a package name.

    We download one requirement at a time, so "newest matching name" is stable
    even when the version spec is floating (e.g. "6.0.*").
    """
    lowered = name.lower().replace("-", "_")
    candidates: list[Path] = []
    for p in sdists_dir.iterdir():
        if not p.is_file():
            continue
        if not (
            p.name.endswith(".tar.gz")
            or p.suffix in {".zip", ".gz", ".bz2", ".xz"}
            or p.name.endswith(".tar.bz2")
            or p.name.endswith(".tar.xz")
            or p.name.endswith(".tgz")
            or p.suffix == ".whl"
        ):
            continue
        if p.name.lower().replace("-", "_").startswith(lowered):
            candidates.append(p)
    if not candidates:
        return None
    return max(candidates, key=lambda p: p.stat().st_mtime_ns)


def _download_distribution_archive(
    sdists_dir: Path,
    requirement: str,
    *,
    uv_cache_dir: Path,
    pip_cache_dir: Path,
) -> None:
    """
    Download one package distribution archive into sdists_dir.

    Prefer wheels when available to avoid PEP517 build requirements for sdists.
    """
    env = {
        "UV_CACHE_DIR": str(uv_cache_dir),
        "PIP_CACHE_DIR": str(pip_cache_dir),
    }
    try:
        _run(
            [
                "uv",
                "run",
                "pip",
                "download",
                "--no-deps",
                "--ignore-requires-python",
                "--only-binary",
                ":all:",
                "-d",
                str(sdists_dir),
                requirement,
            ],
            env=env,
        )
    except subprocess.CalledProcessError:
        _run(
            [
                "uv",
                "run",
                "pip",
                "download",
                "--no-deps",
                "--ignore-requires-python",
                "--no-binary",
                ":all:",
                "-d",
                str(sdists_dir),
                requirement,
            ],
            env=env,
        )


def _extract_archive(archive: Path, dest: Path) -> None:
    if archive.suffix in {".zip", ".whl"}:
        with zipfile.ZipFile(archive) as zf:
            zf.extractall(dest)
        return
    # tar.* (including .tar.gz)
    with tarfile.open(archive) as tf:
        tf.extractall(dest)


def _single_top_level_dir(dest: Path) -> Path | None:
    children = [p for p in dest.iterdir() if p.is_dir()]
    if len(children) == 1:
        return children[0]
    return None


def _copy_glob(src_root: Path, dest_root: Path, pattern: str) -> None:
    for path in src_root.glob(pattern):
        if path.is_dir():
            continue
        rel = path.relative_to(src_root)
        out = dest_root / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, out)


def _copy_license_and_metadata(src_root: Path, dest_root: Path) -> None:
    # Common license patterns at repo/package root.
    for name in ("LICENSE", "LICENSE.txt", "LICENSE.md", "COPYING", "NOTICE"):
        p = src_root / name
        if p.exists() and p.is_file():
            shutil.copy2(p, dest_root / p.name)

    # Also include any similarly named top-level license files.
    for p in src_root.iterdir():
        if not p.is_file():
            continue
        upper = p.name.upper()
        if (
            upper.startswith("LICENSE")
            or upper.startswith("COPYING")
            or upper.startswith("NOTICE")
        ):
            shutil.copy2(p, dest_root / p.name)

    for name in ("pyproject.toml", "setup.cfg", "setup.py"):
        p = src_root / name
        if p.exists() and p.is_file():
            shutil.copy2(p, dest_root / p.name)


def _copy_python_for_templatetags_roots(src_root: Path, dest_root: Path) -> None:
    """
    Copy Python sources for any app/package that contains `templatetags/`.

    Corpus entries originally only copied `templatetags/**/*.py`, but projects
    sometimes register builtin filters/tags from non-templatetags modules (e.g.
    importing Django's builtin `register`). Strict unknown validation needs the
    sources for those modules to be present so extraction can find them.

    To keep corpus size reasonable, this copies `*.py` only for each root that
    directly contains a `templatetags/` directory (and its subpackages).
    """
    app_roots: set[Path] = set()
    for p in src_root.rglob("templatetags"):
        if p.is_dir():
            app_roots.add(p.parent)
    for app_root in sorted(app_roots):
        for path in app_root.rglob("*.py"):
            if not path.is_file():
                continue
            rel = path.relative_to(src_root)
            out = dest_root / rel
            out.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, out)


_DEFAULT_ENGINE_BUILTINS: list[str] = [
    "django.template.defaulttags",
    "django.template.defaultfilters",
    "django.template.loader_tags",
]


_BUILTIN_REGISTER_IMPORT_RE = re.compile(
    r"^\s*from\s+django\.template\.(defaultfilters|defaulttags|loader_tags)\s+import\s+register\s*$",
    flags=re.MULTILINE,
)

_REGISTER_USE_RE = re.compile(r"(^\s*@\s*register\.)|(\bregister\.)", flags=re.MULTILINE)


def _corpus_root_from_entry(entry_root: Path) -> Path | None:
    for p in entry_root.parents:
        if p.name == ".corpus":
            return p
    return None


_GLOBAL_LIBRARY_PROVIDERS_CACHE: dict[Path, dict[str, set[str]]] = {}


def _global_library_providers_for_corpus(corpus_root: Path) -> dict[str, set[str]]:
    cached = _GLOBAL_LIBRARY_PROVIDERS_CACHE.get(corpus_root)
    if cached is not None:
        return cached

    providers: dict[str, set[str]] = {}

    def _add_from_entry(entry: Path) -> None:
        for path in entry.rglob("templatetags/**/*.py"):
            if path.name == "__init__.py":
                continue
            rel = path.relative_to(entry).with_suffix("")
            module = ".".join(rel.parts)
            providers.setdefault(path.stem, set()).add(module)

    for entry in sorted((corpus_root / "packages").glob("*/*")):
        if entry.is_dir():
            _add_from_entry(entry)
    for entry in sorted((corpus_root / "repos").glob("*/*")):
        if entry.is_dir():
            _add_from_entry(entry)

    _GLOBAL_LIBRARY_PROVIDERS_CACHE[corpus_root] = providers
    return providers


_LOAD_TAG_RE = re.compile(r"\{\%\s*load\s+([^%]+?)\%\}")


def _required_load_libraries(entry_root: Path) -> set[str]:
    libs: set[str] = set()
    for tpl in entry_root.rglob("templates/**/*"):
        if not tpl.is_file():
            continue
        if tpl.suffix.lower() not in {".html", ".txt", ".xml"}:
            continue
        text = tpl.read_text(encoding="utf-8", errors="replace")
        for m in _LOAD_TAG_RE.finditer(text):
            payload = m.group(1).strip()
            if not payload:
                continue
            parts = payload.split()
            if "from" in parts:
                idx = parts.index("from")
                if idx + 1 < len(parts):
                    libs.add(parts[idx + 1])
                continue
            libs.update(parts)
    return libs


def _discover_template_builtins(entry_root: Path) -> list[str]:
    """
    Best-effort discovery of `TEMPLATES[...]['OPTIONS']['builtins']` modules.

    Many projects configure template libraries as "builtins" in settings so they
    can be used without `{% load %}`. Strict-unknown corpus validation needs a
    registry approximation for this.

    This intentionally does *not* execute any code. It scans Python ASTs for
    dict literals containing a `"builtins": [...]` (or tuple) entry and collects
    string literals from the sequence value.
    """
    discovered: list[str] = []
    seen: set[str] = set()

    def _add(value: str) -> None:
        if value in seen:
            return
        seen.add(value)
        discovered.append(value)

    for path in sorted(entry_root.rglob("*.py")):
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if "builtins" not in text:
            continue
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue

        for node in ast.walk(tree):
            if not isinstance(node, ast.Dict):
                continue
            for k, v in zip(node.keys, node.values):
                if not isinstance(k, ast.Constant) or k.value != "builtins":
                    continue
                if isinstance(v, (ast.List, ast.Tuple, ast.Set)):
                    for elt in v.elts:
                        if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                            _add(elt.value)

    return discovered


def _write_runtime_registry(entry_root: Path) -> None:
    """
    Write `.runtime_registry.json` for a corpus entry.

    This file mirrors the runtime-registry shape consumed by template_linter:
    - `libraries`: `{% load %}` name -> module path (derived from templatetags)
    - `builtins`: ordered module paths that contribute builtins
      (Engine.builtins plus additional "builtin contributor" modules discovered
      statically within the entry)

    Notes:
    - This is a static approximation. Its primary goal is to make strict unknown
      validation meaningful in the corpus harness without requiring a full
      runnable Django environment for each entry.
    """
    libs: dict[str, str] = {}
    for path in sorted(entry_root.rglob("templatetags/**/*.py")):
        if path.name == "__init__.py":
            continue
        rel = path.relative_to(entry_root).with_suffix("")
        module = ".".join(rel.parts)
        libs.setdefault(path.stem, module)

    # Add loadable libraries that this entry's templates reference but which are
    # provided by other corpus entries (dependencies in the pinned set).
    corpus_root = _corpus_root_from_entry(entry_root)
    if corpus_root is not None:
        providers = _global_library_providers_for_corpus(corpus_root)
        for name in sorted(_required_load_libraries(entry_root)):
            if name in libs:
                continue
            candidates = providers.get(name)
            if not candidates:
                continue
            if len(candidates) == 1:
                libs[name] = next(iter(candidates))

    builtins: list[str] = list(_DEFAULT_ENGINE_BUILTINS)
    seen = set(builtins)

    # Project-configured builtins (from settings) come before contributor
    # modules so local overrides can still win.
    for module in _discover_template_builtins(entry_root):
        if module in seen:
            continue
        seen.add(module)
        builtins.append(module)

    # Heuristic: treat `templatetags/builtins/**/*.py` modules as builtins.
    #
    # Several projects (e.g. NetBox) provide template tags/filters that are
    # intended to be globally available without `{% load %}` by organizing them
    # under a `builtins/` templatetags package. Corpus sync does not copy full
    # settings modules for repos, so we can't always discover the builtins list
    # from `TEMPLATES[...]["OPTIONS"]["builtins"]`.
    for path in sorted(entry_root.rglob("templatetags/builtins/**/*.py")):
        if path.name == "__init__.py":
            continue
        rel = path.relative_to(entry_root).with_suffix("")
        module = ".".join(rel.parts)
        if module in seen:
            continue
        seen.add(module)
        builtins.append(module)

    for path in sorted(entry_root.rglob("*.py")):
        if "templatetags" in path.parts:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if not _BUILTIN_REGISTER_IMPORT_RE.search(text):
            continue
        if not _REGISTER_USE_RE.search(text):
            continue

        rel = path.relative_to(entry_root).with_suffix("")
        module = ".".join(rel.parts)
        if module in seen:
            continue
        seen.add(module)
        builtins.append(module)

    out_path = entry_root / ".runtime_registry.json"
    out_path.write_text(
        json.dumps({"builtins": builtins, "libraries": libs}, indent=2, sort_keys=True)
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    raise SystemExit(main())
