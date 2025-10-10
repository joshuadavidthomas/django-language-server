from __future__ import annotations

import re


def on_page_markdown(markdown, page, config, files):
    markdown = _convert_admonitions(markdown)
    markdown = _convert_image_paths(markdown)
    markdown = _convert_repo_links(markdown, config.repo_url)
    return markdown


def _convert_admonitions(content):
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

    def process_match(match):
        admonition_type = ADMONITION_MAP.get(match.group(1).upper(), "note")
        content_lines = match.group(2).rstrip().split("\n")
        cleaned_lines = [line.lstrip("> ") for line in content_lines]
        indented_content = "\n".join(
            f"    {line}" if line.strip() else "" for line in cleaned_lines
        )
        trailing_newlines = len(match.group(2)) - len(match.group(2).rstrip("\n"))
        return f"!!! {admonition_type}\n\n{indented_content}" + "\n" * trailing_newlines

    pattern = r"(?m)^>\s*\[!(.*?)\]\s*\n((?:>.*(?:\n|$))+)"
    return re.sub(pattern, process_match, content)


def _convert_repo_links(content, repo_url):
    def replace_link(match):
        text, path = match.group(1), match.group(2)

        if path.startswith(("#", "http://", "https://", "./assets/", "assets/")):
            return match.group(0)

        if path.startswith("docs/"):
            return f"[{text}]({path.removeprefix('docs/')})"

        if "clients/nvim/README.md" in path:
            return f"[{text}](clients/neovim.md)"

        if path.startswith(("../", "./")) and (path.endswith(".md") or ".md#" in path):
            return match.group(0)

        clean_path = path.replace("../", "").replace("./", "").lstrip("/")
        return f"[{text}]({repo_url.rstrip('/')}/blob/main/{clean_path})"

    pattern = r"(?<!!)\[((?:[^][]|\[[^]]*\])*)\]\(([^)]+)\)"
    return re.sub(pattern, replace_link, content)


def _convert_image_paths(content):
    return re.sub(r"!\[([^\]]*)\]\(\.\/docs\/assets\/", r"![\1](./assets/", content)
