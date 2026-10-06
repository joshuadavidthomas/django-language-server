# Releasing

Releases are prepared in a pull request and published from an immutable version
tag. Do not create or publish the GitHub release manually: the release workflow
keeps it as a draft until PyPI and the standalone download have been verified.

## Prepare the release

Create a `release/vX.Y.Z` branch from the latest `main`, then update the version:

```bash
git switch -c release/vX.Y.Z
just bumpver update --set-version X.Y.Z --no-commit --no-push
cargo check -q
uv lock
```

Finalize `CHANGELOG.md` in the same branch:

1. Leave an empty `[Unreleased]` section at the top.
2. Move the accumulated entries under `[X.Y.Z]`.
3. Change the `unreleased` comparison to start at `vX.Y.Z`.
4. Add the `[X.Y.Z]` release link.

Run the release metadata check before opening the pull request:

```bash
uv run tools/release.py check --release vX.Y.Z
```

The lint workflow runs the same check. The `release/*` branch name also causes
the build workflow to compile every supported artifact target before the release
is tagged.

## Publish the release

After the release pull request is merged, update `main`, create an annotated tag
on the merge commit, and push only the tag:

```bash
git switch main
git pull --ff-only
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

The tag starts the release workflow. It verifies that the tag is on `main` and
that package, lockfile, documentation, and changelog versions agree. It then:

1. runs lint and the full test suite;
2. builds and attests wheels, the source distribution, and standalone archives;
3. creates a draft GitHub release and uploads checksum-verified archives;
4. publishes the Python artifacts to PyPI;
5. verifies the complete PyPI file inventory and installs the published package;
6. downloads and runs the Linux standalone archive; and
7. publishes the GitHub release.

Treat PyPI versions, published release assets, and release tags as immutable. Do
not move a tag or replace an asset after publication.

## Recover a partial release

Use **Actions → release → Run workflow**, then select the existing version tag
from **Use workflow from** before running it. The workflow refuses recovery from
another ref so attestations identify the commit that produced the artifacts. The
equivalent CLI command is:

```bash
gh workflow run release.yml --ref vX.Y.Z
```

The workflow skips PyPI files that already exist. Completed GitHub assets are not
overwritten; empty reservations left by failed uploads are removed before retry.
Complete archive/checksum pairs are verified, a missing checksum is regenerated
from its published archive, and an archive is uploaded beside an existing
checksum only when its digest matches. Missing pairs are uploaded and the
end-to-end checks run again before a draft release is published.

If recovery reports that an existing checksum and archive disagree, stop and
investigate rather than deleting or replacing either asset. If a checksum was
published without its archive, download the original `binary-*` artifact from
the failed workflow run, verify that archive against the published checksum,
and upload the matching archive. A newly rebuilt archive may contain different
timestamps and must not be substituted merely because it came from the same
tag. Rerun the release workflow after restoring the matching archive.
