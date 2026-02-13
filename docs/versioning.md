# Versioning

## Supported versions

Django Language Server supports the intersection of all actively maintained Python and Django versions:

- All [actively maintained Django versions](https://www.djangoproject.com/download/#supported-versions), including LTS releases
- All [actively maintained Python versions](https://devguide.python.org/versions/)

End-of-life Python versions are not supported, even if a maintained Django version still officially supports them. For example, Django 4.2 LTS supports Python 3.8, but Python 3.8 is EOL so the language server does not.

Currently supported:

<!-- [[[cog
import subprocess
import cog

from noxfile import DJ_VERSIONS
from noxfile import PY_VERSIONS
from noxfile import display_version

django_versions = [
    display_version(version) for version in DJ_VERSIONS if version != "main"
]

cog.outl(f"- Python {', '.join(PY_VERSIONS)}")
cog.outl(f"- Django {', '.join(django_versions)}")
]]] -->
- Python 3.10, 3.11, 3.12, 3.13, 3.14
- Django 4.2, 5.1, 5.2, 6.0
<!-- [[[end]]] -->

## Version numbering

This project adheres to DjangoVer. For a quick overview of what DjangoVer is, here's an excerpt from Django core developer James Bennett's [Introducing DjangoVer](https://www.b-list.org/weblog/2024/nov/18/djangover/) blog post:

> In DjangoVer, a Django-related package has a version number of the form `DJANGO_MAJOR.DJANGO_FEATURE.PACKAGE_VERSION`, where `DJANGO_MAJOR` and `DJANGO_FEATURE` indicate the most recent feature release series of Django supported by the package, and `PACKAGE_VERSION` begins at zero and increments by one with each release of the package supporting that feature release of Django.

In short, `v5.1.x` means the latest version of Django the server would support is 5.1 — so, e.g., versions `v5.1.0`, `v5.1.1`, `v5.1.2`, etc. should all work with Django 5.1.

## Breaking changes

While DjangoVer doesn't encode API stability in the version number, this project strives to follow Django's standard practice of "deprecate for two releases, then remove" policy for breaking changes. Given this is a language server, breaking changes should primarily affect:

- Configuration options (settings in editor config files)
- CLI commands and arguments
- LSP protocol extensions (custom commands/notifications)

The project will provide deprecation warnings where possible and document breaking changes clearly in release notes.

!!! note "Deprecation Policy Across Django Version Updates"

    The "two releases" policy refers to two **release cycles**, not specific version number components. When the language server bumps its major version to track a new Django release (per DjangoVer), ongoing deprecation timelines continue uninterrupted across this version boundary.

For example, if a configuration option is deprecated:

- **`v6.0.0`**: Old option works but logs deprecation warning
- **`v6.0.1`**: Old option still works, continues to show warning
- **`v6.0.2`**: Old option removed

Or spanning a Django version update:

- **`v5.2.4`**: Feature deprecated with warning
- **`v6.0.0`**: Still supported with warning (despite major version bump for Django 6.0)
- **`v6.1.0`**: Feature removed after two release cycles
