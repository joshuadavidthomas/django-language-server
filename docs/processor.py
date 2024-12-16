# /// script
# dependencies = [
#     "rich>=13.9.4",
# ]
# ///

"""
README.md processor using functional callbacks for processing steps.
Uses rich for beautiful logging and progress display.
"""

from __future__ import annotations

import logging
import re
from difflib import Differ
from functools import reduce
from itertools import islice
from pathlib import Path
from typing import Callable

from rich.console import Console
from rich.logging import RichHandler
from rich.panel import Panel
from rich.progress import track

console = Console()
logging.basicConfig(
    level=logging.INFO,
    format="%(message)s",
    handlers=[RichHandler(rich_tracebacks=True, show_time=False)],
)
logger = logging.getLogger(__name__)

ProcessingFunc = Callable[[str], str]


def compose(*functions: ProcessingFunc) -> ProcessingFunc:
    """Compose multiple processing functions into a single function."""
    return reduce(lambda f, g: lambda x: g(f(x)), functions)


def read_file(path: Path) -> str | None:
    """Read content from a file."""
    try:
        content = path.read_text(encoding="utf-8")
        console.print(f"[green]✓[/green] Read {len(content)} bytes from {path}")
        return content
    except FileNotFoundError:
        console.print(f"[red]✗[/red] Input file not found: {path}")
        return None
    except Exception as e:
        console.print(f"[red]✗[/red] Error reading input file: {e}")
        return None


def write_file(path: Path, content: str) -> bool:
    """Write content to a file."""
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        console.print(f"[green]✓[/green] Wrote {len(content)} bytes to {path}")
        return True
    except Exception as e:
        console.print(f"[red]✗[/red] Error writing output file: {e}")
        return False


def preview_changes(original: str, processed: str, context_lines: int = 2) -> None:
    """Show a preview of the changes made."""
    console.print("\n[yellow]Preview of changes:[/yellow]")

    # Basic statistics
    orig_lines = original.count("\n")
    proc_lines = processed.count("\n")
    diff_lines = proc_lines - orig_lines

    stats_panel = Panel(
        f"Original lines: {orig_lines}\n"
        f"Processed lines: {proc_lines}\n"
        f"Difference: {diff_lines:+d} lines",
        title="Statistics",
        border_style="blue",
    )
    console.print(stats_panel)

    # Create diff
    differ = Differ()
    diff = list(differ.compare(original.splitlines(), processed.splitlines()))

    # Find changed line groups with context
    changes = []
    current_group = []
    in_change = False
    last_change_line = -1

    for i, line in enumerate(diff):
        if line.startswith("? "):  # Skip hint lines
            continue

        is_change = line.startswith(("- ", "+ "))
        if is_change:
            if not in_change:  # Start of a new change group
                start = max(0, i - context_lines)
                # If we're close to previous group, connect them
                if start <= last_change_line + context_lines:
                    start = last_change_line + 1
                else:
                    if current_group:
                        changes.append(current_group)
                    current_group = []
                    # Add previous context
                    current_group.extend(
                        l for l in diff[start:i] if not l.startswith("? ")
                    )
            current_group.append(line)
            in_change = True
            last_change_line = i
        else:
            if in_change:
                # Add following context
                following_context = list(
                    islice(
                        (l for l in diff[i:] if not l.startswith("? ")), context_lines
                    )
                )
                if following_context:  # Only extend if we have context to add
                    current_group.extend(following_context)
                in_change = False

    if current_group:
        changes.append(current_group)

    # Format and display the changes
    formatted_output = []
    for i, group in enumerate(changes):
        if i > 0:
            formatted_output.append(
                "[bright_black]⋮ skipped unchanged content ⋮[/bright_black]"
            )

        # Track the last line to avoid duplicates
        last_line = None

        for line in group:
            # Skip if this line is the same as the last one
            if line == last_line:
                continue

            if line.startswith("  "):  # unchanged
                formatted_output.append(f"[white]{line[2:]}[/white]")
            elif line.startswith("- "):  # removed
                formatted_output.append(f"[red]━ {line[2:]}[/red]")
            elif line.startswith("+ "):  # added
                formatted_output.append(f"[green]+ {line[2:]}[/green]")

            last_line = line

    console.print(
        Panel(
            "\n".join(formatted_output),
            title="Changes with Context",
            border_style="yellow",
        )
    )


def process_readme(
    input: str = "README.md",
    output: str = "docs/index.md",
    processors: list[ProcessingFunc] | None = None,
    preview: bool = True,
) -> bool:
    """
    Process README.md with given processing functions.

    Args:
        input_path: Path to the input README.md file
        output_path: Path where the processed file will be saved
        processors: List of processing functions to apply
        preview: Whether to show a preview of changes

    Returns:
        bool: True if processing was successful, False otherwise
    """
    with console.status("[bold green]Processing README...") as status:
        input_path = Path(input)
        output_path = Path(output)

        content = read_file(input_path)
        if content is None:
            return False

        original_content = content

        try:
            for proc in track(processors, description="Applying processors"):
                status.update(f"[bold green]Running {proc.__name__}...")
                content = proc(content)

            if preview:
                preview_changes(original_content, content)

            return write_file(output_path, content)

        except Exception as e:
            console.print(f"[red]Error during processing:[/red] {e}")
            return False


def add_frontmatter(
    metadata: dict[str, str | int | float | bool | list | None],
) -> ProcessingFunc:
    """
    Add or update frontmatter from a dictionary of metadata.

    Args:
        metadata: Dictionary of metadata to add to frontmatter

    Returns:
        A processor function that adds/updates frontmatter

    Example:
        Input:
            # Title
            Content here

        Output:
            ---
            title: My Page
            weight: 10
            hide:
              - navigation
            ---

            # Title
            Content here
    """

    def processor(content: str) -> str:
        # Remove existing frontmatter if present
        content_without_frontmatter = re.sub(
            r"^---\n.*?\n---\n", "", content, flags=re.DOTALL
        )

        # Build the new frontmatter
        frontmatter_lines = ["---"]

        for key, value in metadata.items():
            if isinstance(value, (str, int, float, bool)) or value is None:
                frontmatter_lines.append(f"{key}: {value}")
            elif isinstance(value, list):
                frontmatter_lines.append(f"{key}:")
                for item in value:
                    frontmatter_lines.append(f"  - {item}")
            # Could add more types (dict, etc.) as needed

        frontmatter_lines.append("---\n\n")

        return "\n".join(frontmatter_lines) + content_without_frontmatter

    processor.__name__ = "add_frontmatter"
    return processor


def convert_admonitions(content: str) -> str:
    """
    Convert GitHub-style admonitions to Material for MkDocs-style admonitions.

    Args:
        content: The markdown content to process

    Returns:
        Processed content with converted admonitions

    Example:
        Input:
            > [!NOTE]
            > Content here
            > More content

        Output:
            !!! note

                Content here
                More content
    """
    # Mapping from GitHub admonition types to Material for MkDocs types
    ADMONITION_MAP = {
        "NOTE": "note",
        "TIP": "tip",
        "IMPORTANT": "important",
        "WARNING": "warning",
        "CAUTION": "warning",
        "ALERT": "danger",
        "DANGER": "danger",
        "INFO": "info",
        "TODO": "todo",
        "HINT": "tip",
    }

    def process_match(match: re.Match[str]) -> str:
        # Get admonition type and map it, defaulting to note if unknown
        admonition_type = ADMONITION_MAP.get(match.group(1).upper(), "note")
        content_lines = match.group(2).rstrip().split("\n")

        # Remove the leading '> ' from each line
        cleaned_lines = [line.lstrip("> ") for line in content_lines]

        # Indent the content (4 spaces)
        indented_content = "\n".join(
            f"    {line}" if line.strip() else "" for line in cleaned_lines
        )

        # Preserve the exact number of trailing newlines from the original match
        trailing_newlines = len(match.group(2)) - len(match.group(2).rstrip("\n"))

        return f"!!! {admonition_type}\n\n{indented_content}" + "\n" * trailing_newlines

    # Match GitHub-style admonitions
    pattern = r"(?m)^>\s*\[!(.*?)\]\s*\n((?:>.*(?:\n|$))+)"

    return re.sub(pattern, process_match, content)


def convert_repo_links(repo_url: str) -> ProcessingFunc:
    """
    Convert relative repository links to absolute URLs.

    Args:
        repo_url: The base repository URL (e.g., 'https://github.com/username/repo')

    Returns:
        A processor function that converts relative links to absolute URLs

    Example:
        Input:
            See the [`LICENSE`](LICENSE) file for more information.
            Check the [Neovim](/docs/editors/neovim.md) guide.

        Output:
            See the [`LICENSE`](https://github.com/username/repo/blob/main/LICENSE) file for more information.
            Check the [Neovim](editors/neovim.md) guide.
    """

    def processor(content: str) -> str:
        def replace_link(match: re.Match[str]) -> str:
            text = match.group(1)
            path = match.group(2)

            # Skip anchor links
            if path.startswith("#"):
                return match.group(0)

            # Skip already absolute URLs
            if path.startswith(("http://", "https://")):
                return match.group(0)

            # Handle docs directory links
            if path.startswith(("/docs/", "docs/")):
                # Remove /docs/ or docs/ prefix and .md extension
                clean_path = path.removeprefix("/docs/").removeprefix("docs/")
                return f"[{text}]({clean_path})"

            # Handle root-relative paths
            if path.startswith("/"):
                path = path.removeprefix("/")

            # Remove ./ if present
            path = path.removeprefix("./")

            # Construct the full URL for repository files
            full_url = f"{repo_url.rstrip('/')}/blob/main/{path}"
            return f"[{text}]({full_url})"

        # Match markdown links: [text](url)
        pattern = r"\[((?:[^][]|\[[^]]*\])*)\]\(([^)]+)\)"
        return re.sub(pattern, replace_link, content)

    processor.__name__ = "convert_repo_links"
    return processor


def main():
    """Example usage of the readme processor."""
    console.print("[bold blue]README Processor[/bold blue]")

    processors = [
        add_frontmatter({"title": "Home"}),
        convert_admonitions,
        convert_repo_links(
            "https://github.com/joshuadavidthomas/django-language-server"
        ),
    ]

    success = process_readme(
        input="README.md",
        output="docs/index.md",
        processors=processors,
        preview=True,
    )

    if success:
        console.print("\n[green]✨ Processing completed successfully![/green]")
    else:
        console.print("\n[red]Processing failed![/red]")


if __name__ == "__main__":
    main()
