#!/usr/bin/env bash

set -euo pipefail

get_version() {
        uv run --with bumpver bumpver show --no-fetch | grep -Po '^PEP440         : \K.*'
}

update_changelog() {
        local repo_url
        repo_url=$(git remote get-url origin | tr -d '\n' | sed 's/\.git$//')

        local version
        version=$(get_version)

        sed -i "0,/## \[Unreleased\]/s/## \[Unreleased\]/## [$version]/" CHANGELOG.md
        sed -i "/## \[$version\]/i ## [Unreleased]\n" CHANGELOG.md
        echo "[$version]: $repo_url/releases/tag/v$version" >>CHANGELOG.md
        sed -i "s|\[unreleased\]: .*|[unreleased]: $repo_url/compare/v$version...HEAD|" CHANGELOG.md

        git add CHANGELOG.md
        git commit -m "update CHANGELOG for version $version"
}

update_uvlock() {
        local version
        version=$(get_version)

        uv lock

        if ! git status --porcelain | grep -q "uv.lock"; then
                echo "No changes to uv.lock, skipping commit"
                return 0
        fi

        git add uv.lock
        git commit -m "update uv.lock for version $version"
}

main() {
        update_changelog
        update_uvlock
}

main "$@"
