set unstable := true

justfile := justfile_directory() + "/.just/docs.just"
mkdoc_config := justfile_directory() + "/.mkdocs.yml"

[private]
default:
    @just --list --justfile {{ justfile }}

[private]
[no-cd]
copy:
    #!/usr/bin/env sh

    {
        head -n 1 CHANGELOG.md
        sed -n '/^## \[Unreleased\]/,$p' CHANGELOG.md \
            | sed 's](docs/](]g'
    } > docs/changelog.md

    cat CONTRIBUTING.md \
        | awk '
            /^> \[!NOTE\]/ {
                print "!!! note\n"
                in_admonition = 1
                next
            }
            in_admonition && /^> / {
                sub(/^> /, "    ")
                print
                next
            }
            in_admonition && !/^> / {
                in_admonition = 0
            }
            /^<!-- \[\[\[cog/,/^\]\]\] -->$/ { next }
            /^<!-- \[\[\[end\]\]\] -->$/ { next }
            { print }
        ' \
        | sed 's](\./]\(https://github.com/joshuadavidthomas/django-language-server/tree/main/]g' \
        | sed 's](\([^h#:/][^)]*\))]\(https://github.com/joshuadavidthomas/django-language-server/tree/main/\1)]g' \
        > docs/contributing.md

    {
        echo "---"
        echo "title: Home"
        echo "---"
        echo ""
        cat README.md \
            | awk '
            BEGIN { in_cog = 0; in_admonition = 0; past_header = 0 }
            /^# / && !past_header { past_header = 1; print; next }
            /^<!-- \[\[\[cog/ { in_cog = 1; next }
            in_cog && /^\]\]\] -->$/ { in_cog = 0; next }
            in_cog { next }
            /^<!-- \[\[\[end\]\]\] -->$/ { next }
            /img\.shields\.io/ { next }
            /^> \[!CAUTION\]/ {
                print "!!! warning\n"
                in_admonition = 1
                next
            }
            in_admonition && /^> / {
                sub(/^> /, "    ")
                print
                next
            }
            in_admonition && !/^> / {
                in_admonition = 0
            }
            /^## Documentation$/ { exit }
            { print }
            ' \
            | sed 's](docs/](]g' \
            | sed 's](LICENSE)]\(https://github.com/joshuadavidthomas/django-language-server/tree/main/LICENSE)]g'
    } > docs/index.md

# Generate docs/rules.md from rules.toml and djls check output
[no-cd]
generate-rules:
    python3 docs/rules/generate.py

[private]
fmt:
    @just --fmt --justfile {{ justfile }}

# Build documentation
[no-cd]
build LOCATION="site": copy
    uv run --group docs --frozen zensical build --clean --config-file {{ mkdoc_config }}

# Serve documentation locally
[no-cd]
serve PORT="8000": copy
    #!/usr/bin/env sh
    HOST="localhost"
    if [ -f "/.dockerenv" ]; then
        HOST="0.0.0.0"
    fi
    uv run --group docs --frozen zensical serve --config-file {{ mkdoc_config }} --dev-addr localhost:{{ PORT }}
