#!/usr/bin/env bash

set -euo pipefail

update_changelog() {
        local repo_url
        repo_url=$(git remote get-url origin | tr -d '\n' | sed 's/\.git$//')

        sed -i "0,/## \[Unreleased\]/s/## \[Unreleased\]/## [$BUMPVER_NEW_VERSION]/" CHANGELOG.md
        sed -i "/## \[$BUMPVER_NEW_VERSION\]/i ## [Unreleased]\n" CHANGELOG.md
        echo "[$BUMPVER_NEW_VERSION]: $repo_url/releases/tag/v$BUMPVER_NEW_VERSION" >>CHANGELOG.md
        sed -i "s|\[unreleased\]: .*|[unreleased]: $repo_url/compare/v$BUMPVER_NEW_VERSION...HEAD|" CHANGELOG.md

        git add CHANGELOG.md
        git commit -m "update CHANGELOG for version $BUMPVER_NEW_VERSION"
}

update_uvlock() {
        uv lock

        if ! git status --porcelain | grep -q "uv.lock"; then
                echo "No changes to uv.lock, skipping commit"
                return 0
        fi

        git add uv.lock
        git commit -m "update uv.lock for version $BUMPVER_NEW_VERSION"
}

main() {
        update_changelog
        update_uvlock
}

main "$@"
