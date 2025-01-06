# Django Template AST Specification

## Overview

This document specifies the Abstract Syntax Tree (AST) design for parsing Django templates. The AST represents the structure and semantics of Django templates, enabling accurate parsing, analysis, and tooling support.

## Types

### `Ast`

The root of the AST, representing the entire parsed template.

```rust
pub struct Ast {
    pub nodes: Vec<Node>,       // Top-level nodes in the template
    pub line_offsets: Vec<u32>, // Positions of line breaks for mapping offsets to line/column
}
```

### `Span`

Represents the position of a node within the source template.

```rust
pub struct Span {
    pub start: u32,  // Byte offset from the start of the template
    pub length: u32, // Length in bytes
}
```

### `Node`

Enumeration of all possible node types in the AST.

```rust
pub enum Node {
    Text {
        content: String, // The raw text content
        span: Span,      // The position of the text in the template
    },
    Comment {
        content: String, // The comment content
        span: Span,      // The position of the comment in the template
    },
    Variable {
        bits: Vec<String>,          // Components of the variable path
        filters: Vec<DjangoFilter>, // Filters applied to the variable
        span: Span,                 // The position of the variable in the template
    },
    Block(Block),
}
```

#### `Node::Text`

Represents raw text and HTML content outside of Django template tags.

```rust
Node::Text {
    content: String, // The raw text content
    span: Span,      // The position of the text in the template
}
```

#### `Node::Comment`

Represents Django template comments (`{# ... #}`).

```rust
Node::Comment {
    content: String, // The comment content
    span: Span,      // The position of the comment in the template
}
```

#### `Node::Variable`

Represents variable interpolation (`{{ variable|filter }}`).

```rust
Node::Variable {
    bits: Vec<String>,          // Components of the variable path
    filters: Vec<DjangoFilter>, // Filters applied to the variable
    span: Span,                 // The position of the variable in the template
}
```

##### `DjangoFilter`

Represents a filter applied to a variable.

```rust
pub struct DjangoFilter {
    pub name: String,      // Name of the filter
    pub args: Vec<String>, // Arguments passed to the filter
}
```

#### `Node::Block`

Represents Django template tags that may have nested content, assignments, and control flow structures.

```rust
Node::Block(Block)
```

### `Block`

Represents Django template tags that may have nested content, assignments, and control flow structures.

```rust
pub enum Block {
    Block {
        tag: Tag,
        nodes: Vec<Node>,
        closing: Option<Box<Block>>,   // Contains Block::Closing if present
        assignments: Option<Vec<Assignment>>, // Assignments declared within the tag (e.g., `{% with var=value %}`)
    },
    Branch {
        tag: Tag,
        nodes: Vec<Node>,
    },
    Tag {
        tag: Tag,
    },
    Inclusion {
        tag: Tag,
        template_name: String,
    },
    Variable {
        tag: Tag,
    },
    Closing {
        tag: Tag,
    },
}
```

#### `Tag`

Shared structure for all tag-related nodes in `Block`.

```rust
pub struct Tag {
    pub name: String,               // Name of the tag (e.g., "if", "for", "include")
    pub bits: Vec<String>,          // Arguments or components of the tag
    pub span: Span,                 // Span covering the entire tag
    pub tag_span: Span,             // Span covering just the tag declaration (`{% tag ... %}`)
    pub assignment: Option<String>, // Optional assignment target variable name
}
```

#### `Assignment`

Represents an assignment within a tag (e.g., `{% with var=value %}` or `{% url 'some-view' as assigned_url %}`).

```rust
pub struct Assignment {
    pub target: String, // Variable name to assign to
    pub value: String,  // Value assigned to the variable
}
```

#### Variants

##### `Block::Block`

Represents standard block tags that may contain child nodes and require a closing tag.

```rust
Block::Block {
    tag: Tag,                             // The opening Tag of the block
    nodes: Vec<Node>,                     // Nodes contained within the block
    closing: Option<Box<Block>>,          // Contains Block::Closing if present
    assignments: Option<Vec<Assignment>>, // Assignments declared within the tag
}
```

Examples:

- `{% if %}...{% endif %}`
- `{% for %}...{% endfor %}`
- `{% with %}...{% endwith %}`

##### `Block::Branch`

Represents branch tags that are part of control flow structures and contain child nodes.

```rust
Block::Branch {
    tag: Tag,         // The Tag of the branch
    nodes: Vec<Node>, // Nodes contained within the branch
}
```

Examples:

- `{% elif %}`
- `{% else %}`
- `{% empty %}`

##### `Block::Tag`

Represents standalone tags that do not contain child nodes or require a closing tag.

```rust
Block::Tag {
    tag: Tag, // The Tag of the standalone tag
}
```

Examples:

- `{% csrf_token %}`
- `{% load %}`
- `{% now "Y-m-d" %}`

##### `Block::Inclusion`

Represents tags that include or extend templates.

```rust
Block::Inclusion {
    tag: Tag,              // The Tag of the inclusion tag
    template_name: String, // Name of the template being included/extended
}
```

Examples:

- `{% include "template.html" %}`
- `{% extends "base.html" %}`

##### `Block::Variable`

Represents tags that output a value directly.

```rust
Block::Variable {
    tag: Tag, // The Tag of the variable tag
}
```

Examples:

- `{% cycle %}`
- `{% firstof %}`

##### `Block::Closing`

Represents closing tags corresponding to opening block tags.

```rust
Block::Closing {
    tag: Tag, // The Tag of the closing tag
}
```

Examples:

- `{% endif %}`
- `{% endfor %}`
- `{% endwith %}`

## TagSpecs

### Schema

Tag Specifications (TagSpecs) define how tags are parsed and understood. They allow the parser to handle custom tags without hard-coding them.

```toml
[package.module.path.tag_name]  # Path where tag is registered, e.g., django.template.defaulttags
type = "block" | "tag" | "inclusion" | "variable"
closing = "closing_tag_name"        # For block tags that require a closing tag
supports_assignment = true | false  # Whether the tag supports 'as' assignment
branches = ["branch_tag_name", ...] # For block tags that support branches

[[package.module.path.tag_name.args]]
name = "argument_name"
required = true | false
```

### Configuration

- **Built-in TagSpecs**: The parser includes TagSpecs for Django's built-in tags and popular third-party tags.
- **User-defined TagSpecs**: Users can expand or override TagSpecs via `pyproject.toml` or `djls.toml` files in their project, allowing custom tags and configurations to be seamlessly integrated.

### Examples

#### If Tag

```toml
[django.template.defaulttags.if]
type = "block"
closing = "endif"
supports_assignment = false
branches = ["elif", "else"]

[[django.template.defaulttags.if.args]]
name = "condition"
required = true
```

#### Include Tag

```toml
[django.template.defaulttags.includes]
type = "inclusion"
supports_assignment = true

[[django.template.defaulttags.includes.args]]
name = "template_name"
required = true
```

#### Custom Tag Example

```toml
[my_module.templatetags.my_tags.my_custom_tag]
type = "tag"
supports_assignment = true

{[my_module.templatetags.my_tags.my_custom_tag.args]]
name = "arg1"
required = false
```

### AST Examples

#### Standard Block with Branches

Template:

```django
{% if user.is_authenticated %}
    Hello, {{ user.name }}!
{% elif user.is_guest %}
    Welcome, guest!
{% else %}
    Please log in.
{% endif %}
```

AST Representation:

```rust
Node::Block(Block::Block {
    tag: Tag {
        name: "if".to_string(),
        bits: vec!["user.is_authenticated".to_string()],
        span: Span { start: 0, length: 35 },
        tag_span: Span { start: 0, length: 28 },
        assignment: None,
    },
    nodes: vec![
        Node::Text {
            content: "    Hello, ".to_string(),
            span: Span { start: 35, length: 12 },
        },
        Node::Variable {
            bits: vec!["user".to_string(), "name".to_string()],
            filters: vec![],
            span: Span { start: 47, length: 13 },
        },
        Node::Text {
            content: "!\n".to_string(),
            span: Span { start: 60, length: 2 },
        },
        Node::Block(Block::Branch {
            tag: Tag {
                name: "elif".to_string(),
                bits: vec!["user.is_guest".to_string()],
                span: Span { start: 62, length: 32 },
                tag_span: Span { start: 62, length: 26 },
                assignment: None,
            },
            nodes: vec![
                Node::Text {
                    content: "    Welcome, guest!\n".to_string(),
                    span: Span { start: 94, length: 22 },
                },
            ],
        }),
        Node::Block(Block::Branch {
            tag: Tag {
                name: "else".to_string(),
                bits: vec![],
                span: Span { start: 116, length: 22 },
                tag_span: Span { start: 116, length: 16 },
                assignment: None,
            },
            nodes: vec![
                Node::Text {
                    content: "    Please log in.\n".to_string(),
                    span: Span { start: 138, length: 21 },
                },
            ],
        }),
    ],
    closing: Some(Box::new(Block::Closing {
        tag: Tag {
            name: "endif".to_string(),
            bits: vec![],
            span: Span { start: 159, length: 9 },
            tag_span: Span { start: 159, length: 9 },
            assignment: None,
        },
    })),
    assignments: None,
})
```

#### Inclusion Tag with Assignment

Template:

```django
{% include "header.html" as header_content %}
```

AST Representation:

```rust
Node::Block(Block::Inclusion {
    tag: Tag {
        name: "include".to_string(),
        bits: vec!["\"header.html\"".to_string()],
        span: Span { start: 0, length: 45 },
        tag_span: Span { start: 0, length: 45 },
        assignment: Some("header_content".to_string()),
    },
    template_name: "header.html".to_string(),
})
```

#### Variable Tag

Template:

```django
{% cycle 'odd' 'even' %}
```

AST Representation:

```rust
Node::Block(Block::Variable {
    tag: Tag {
        name: "cycle".to_string(),
        bits: vec!["'odd'".to_string(), "'even'".to_string()],
        span: Span { start: 0, length: 24 },
        tag_span: Span { start: 0, length: 24 },
        assignment: None,
    },
})
```

## LSP Support

The AST design supports integration with Language Server Protocol (LSP) features:

- **Diagnostics**:
    - Detect unclosed or mismatched tags.
    - Identify invalid arguments or unknown tags/filters.
    - Highlight syntax errors with precise location information.
- **Code Navigation**:
    - Go to definitions of variables, tags, and included templates.
    - Find references and usages of variables and blocks.
    - Provide an outline of the template structure.
- **Code Completion**:
    - Suggest tags, filters, and variables in context.
    - Auto-complete tag names and attributes based on TagSpecs.
- **Hover Information**:
    - Display documentation and usage information for tags and filters.
    - Show variable types and values in context.
- **Refactoring Tools**:
    - Support renaming of variables and blocks.
    - Assist in extracting or inlining templates.
    - Provide code actions for common refactoring tasks.
