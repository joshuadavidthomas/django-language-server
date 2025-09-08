#!/usr/bin/env python3
import re

with open("crates/djls-templates/src/validation.rs", "r") as f:
    content = f.read()

# Fix UnclosedTag errors
content = re.sub(
    r"(\s+)self\.errors\.push\(AstError::UnclosedTag \{\n(\s+)tag: (.*?),\n(\s+)span: (.*?),\n(\s+)\}\);",
    r"\1self.errors.push(AstError::UnclosedTag {\n\2tag: \3,\n\4span_start: \5.start(self.db),\n\4span_length: \5.length(self.db),\n\6});",
    content,
    flags=re.MULTILINE,
)

# Fix UnbalancedStructure errors with opening_span: span
content = re.sub(
    r"(\s+)opening_span: span,\n(\s+)closing_span: (.*?),",
    r"\1opening_span_start: span.start(self.db),\n\1opening_span_length: span.length(self.db),\n\2closing_span_start: \3,\n\2closing_span_length: \3,",
    content,
)

# Fix UnbalancedStructure errors with opening_span: opening_tag.span
content = re.sub(
    r"(\s+)opening_span: opening_tag\.span,\n(\s+)closing_span: Some\(span\),",
    r"\1opening_span_start: opening_tag.span.start(self.db),\n\1opening_span_length: opening_tag.span.length(self.db),\n\2closing_span_start: Some(span.start(self.db)),\n\2closing_span_length: Some(span.length(self.db)),",
    content,
)

# Fix UnmatchedBlockName errors
content = re.sub(
    r"(\s+)self\.errors\.push\(AstError::UnmatchedBlockName \{\n(\s+)name: (.*?),\n(\s+)span,\n(\s+)\}\);",
    r"\1self.errors.push(AstError::UnmatchedBlockName {\n\2name: \3,\n\4span_start: span.start(self.db),\n\4span_length: span.length(self.db),\n\5});",
    content,
    flags=re.MULTILINE,
)

# OrphanedTag already fixed

with open("crates/djls-templates/src/validation.rs", "w") as f:
    f.write(content)

print("Fixed validation.rs")
