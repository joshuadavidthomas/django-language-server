from __future__ import annotations

import zipapp
from pathlib import Path


def main():
    source_dir = Path(__file__).parent / "src" / "djls_inspector"
    output_file = Path(__file__).parent / "dist" / "djls_inspector.pyz"
    output_file.parent.mkdir(exist_ok=True)

    zipapp.create_archive(
        source_dir,
        target=output_file,
        interpreter=None,  # No shebang - will be invoked explicitly
        compressed=True,
    )

    print(f"Successfully created {output_file}")
    print(f"Size: {output_file.stat().st_size} bytes")


if __name__ == "__main__":
    main()
