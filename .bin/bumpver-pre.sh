#!/usr/bin/env bash

set -euo pipefail

create_release_branch() {
        local release_branch="release-v${BUMPVER_NEW_VERSION}"

        if git show-ref --quiet "refs/heads/$release_branch" ||
                git show-ref --quiet "refs/remotes/origin/$release_branch"; then
                echo "Error: Branch $release_branch already exists locally or remotely" >&2
                exit 1
        fi

        git checkout -b "$release_branch"
        echo "Created and switched to branch: $release_branch"
}

main() {
        create_release_branch
}

main "$@"
