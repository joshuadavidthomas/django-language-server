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
from dataclasses import dataclass
from difflib import Differ
from functools import reduce
from itertools import islice
from pathlib import Path
from typing import Callable
from typing import Dict
from typing import List
from typing import NamedTuple

from rich.console import Console
from rich.logging import RichHandler
from rich.panel import Panel
from rich.progress import track
from rich.rule import Rule

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


@dataclass
class DiffStats:
    original_lines: int
    processed_lines: int
    difference: int


class DiffLine(NamedTuple):
    orig_line_no: int
    proc_line_no: int
    change_type: str
    content: str


def calculate_stats(original: str, processed: str) -> DiffStats:
    return DiffStats(
        original_lines=original.count("\n"),
        processed_lines=processed.count("\n"),
        difference=processed.count("\n") - original.count("\n"),
    )


def create_diff_lines(diff_output: List[str]) -> List[DiffLine]:
    """Convert raw diff output into structured DiffLine objects with line numbers."""
    diff_lines = []
    orig_line_no = proc_line_no = 0

    for line in diff_output:
        if line.startswith("? "):  # Skip hint lines
            continue

        change_type = line[0:2]
        content = line[2:]

        current_orig = orig_line_no if change_type in ("  ", "- ") else 0
        current_proc = proc_line_no if change_type in ("  ", "+ ") else 0

        diff_lines.append(DiffLine(current_orig, current_proc, change_type, content))

        # Update line numbers
        if change_type == "  ":
            orig_line_no += 1
            proc_line_no += 1
        elif change_type == "- ":
            orig_line_no += 1
        elif change_type == "+ ":
            proc_line_no += 1

    return diff_lines


def group_changes(
    diff_lines: List[DiffLine], context_lines: int = 5
) -> List[List[DiffLine]]:
    """Group changes with their context lines."""
    changes = []
    current_group = []
    in_change = False
    last_change_idx = -1

    for i, line in enumerate(diff_lines):
        is_change = line.change_type in ("- ", "+ ")

        if is_change:
            if not in_change:
                # Start of a new change group
                start_idx = max(0, i - context_lines)

                # Connect nearby groups or start new group
                if start_idx <= last_change_idx + context_lines:
                    start_idx = last_change_idx + 1
                else:
                    if current_group:
                        changes.append(current_group)
                    current_group = []
                    # Add leading context
                    current_group.extend(diff_lines[start_idx:i])

            current_group.append(line)
            in_change = True
            last_change_idx = i

        elif in_change:
            # Add trailing context
            following_context = list(
                islice(
                    (l for l in diff_lines[i:] if l.change_type == "  "), context_lines
                )
            )
            current_group.extend(following_context)
            in_change = False

    if current_group:
        changes.append(current_group)

    return changes


def get_changes(
    original: str, processed: str
) -> tuple[Dict[str, int], List[List[DiffLine]]]:
    """Generate diff information and statistics."""
    # Get basic statistics
    stats = calculate_stats(original, processed)

    # Create and process diff
    differ = Differ()
    diff_output = list(differ.compare(original.splitlines(), processed.splitlines()))
    diff_lines = create_diff_lines(diff_output)
    grouped_changes = group_changes(diff_lines)

    return vars(stats), grouped_changes


@dataclass
class ChangeGroup:
    orig_no: int
    proc_no: int
    change_type: str
    content: str

    def format_line_info(self) -> str:
        """Format the line numbers and separator based on change type."""
        if self.change_type == "  ":
            return f"[bright_black]{self.orig_no:4d}│{self.proc_no:4d}│[/bright_black]"
        elif self.change_type == "- ":
            return f"[bright_black]{self.orig_no:4d}│    │[/bright_black]"
        else:  # "+" case
            return f"[bright_black]    │{self.proc_no:4d}│[/bright_black]"

    def format_content(self) -> str:
        """Format the content based on change type."""
        if self.change_type == "  ":
            return f"[white]{self.content}[/white]"
        elif self.change_type == "- ":
            return f"[red]- {self.content}[/red]"
        else:  # "+" case
            return f"[green]+ {self.content}[/green]"


def create_stats_panel(stats: dict) -> Panel:
    """Create a formatted statistics panel."""
    stats_content = (
        f"Original lines: {stats['original_lines']}\n"
        f"Processed lines: {stats['processed_lines']}\n"
        f"Difference: {stats['difference']:+d} lines"
    )
    return Panel(
        stats_content,
        title="Statistics",
        border_style="blue",
    )


def create_separator(prev_group: List[tuple], current_group: List[tuple]) -> Rule:
    """Create a separator between change groups with skip line information."""
    if not prev_group:
        return None

    last_orig = max(l[0] for l in prev_group if l[0] > 0)
    next_orig = min(l[0] for l in current_group if l[0] > 0)
    skipped_lines = next_orig - last_orig - 1

    if skipped_lines > 0:
        return Rule(
            f" {skipped_lines} lines skipped ",
            style="bright_black",
            characters="⋮",
        )
    return Rule(style="bright_black", characters="⋮")


def print_change_group(group: List[tuple]) -> None:
    """Print a group of changes with formatting."""
    for orig_no, proc_no, change_type, content in group:
        change = ChangeGroup(orig_no, proc_no, change_type, content)
        line_info = change.format_line_info()
        content_formatted = change.format_content()
        console.print(f"{line_info} {content_formatted}")


def preview_changes(original: str, processed: str) -> None:
    """Show a preview of the changes made."""
    console.print("\n[yellow]Preview of changes:[/yellow]")

    # Get diff information and show statistics
    stats, changes = get_changes(original, processed)
    console.print(create_stats_panel(stats))

    # Print changes with separators between groups
    for i, group in enumerate(changes):
        if i > 0:
            separator = create_separator(changes[i - 1], group)
            if separator:
                console.print(separator)

        print_change_group(group)


def process_file(
    input: str = "README.md",
    output: str = "docs/index.md",
    processors: list[ProcessingFunc] | None = None,
    preview: bool = True,
    description: str | None = None,
) -> bool:
    """
    Process a file with given processing functions.

    Args:
        input: Path to the input file
        output: Path where the processed file will be saved
        processors: List of processing functions to apply
        preview: Whether to show a preview of changes
        description: Optional description for status message

    Returns:
        bool: True if processing was successful, False otherwise
    """
    status_msg = f"[bold green]Processing {description or input}..."
    with console.status(status_msg) as status:
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
            Open an [issue](../../issues/new) to report bugs.

        Output:
            See the [`LICENSE`](https://github.com/username/repo/blob/main/LICENSE) file for more information.
            Check the [Neovim](editors/neovim.md) guide.
            Open an [issue](https://github.com/username/repo/issues/new) to report bugs.
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

            # Handle relative paths with ../ or ./
            if "../" in path or "./" in path:
                # Special handling for GitHub-specific paths
                if "issues/" in path or "pulls/" in path:
                    clean_path = path.replace("../", "").replace("./", "")
                    return f"[{text}]({repo_url}/{clean_path})"

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
    """Process documentation files."""
    console.print("[bold blue]Documentation Processor[/bold blue]")

    common_processors = [
        convert_admonitions,
        convert_repo_links(
            "https://github.com/joshuadavidthomas/django-language-server"
        ),
    ]

    readme_success = process_file(
        input="README.md",
        output="docs/index.md",
        processors=[
            add_frontmatter({"title": "Home"}),
            *common_processors,
        ],
        preview=True,
        description="README.md → docs/index.md",
    )

    nvim_success = process_file(
        input="editors/nvim/README.md",
        output="docs/editors/neovim.md",
        processors=[
            add_frontmatter(
                {
                    "title": "Neovim",
                    "weight": 10,
                }
            ),
            *common_processors,
        ],
        preview=True,
        description="Neovim docs → docs/editors/neovim.md",
    )

    if readme_success and nvim_success:
        console.print("\n[green]✨ All files processed successfully![/green]")
    else:
        console.print("\n[red]Some files failed to process:[/red]")
        for name, success in [
            ("README.md → docs/index.md", readme_success),
            ("Neovim docs → docs/editors/neovim.md", nvim_success),
        ]:
            status = "[green]✓[/green]" if success else "[red]✗[/red]"
            console.print(f"{status} {name}")


if __name__ == "__main__":
    main()
