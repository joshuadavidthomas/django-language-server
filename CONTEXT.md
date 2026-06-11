# Django Language Server

Django Language Server provides IDE features for Django templates by combining template source with knowledge about the surrounding Django project.

## Language

### Project and Django concepts

**Project**:
A Django codebase being analyzed by the language server.
_Avoid_: Django Project, workspace, repository

**Workspace**:
The language server's view of open documents and filesystem contents.
_Avoid_: Project, repository, workspace folder

**Project Facts**:
The static and derived information the language server has about a **Project**.
_Avoid_: Project Model, Project Context, Project Knowledge, Project State

**Django Environment**:
A path-scoped Django analysis context within a **Project**, rooted at a directory and configured by a **Django Settings Module**.
_Avoid_: Python environment, virtual environment, OS environment, workspace

**Django Settings Module**:
The Python module that configures a **Project** for Django.
_Avoid_: Settings, DJLS settings, environment

**Django Discovery**:
The process of building **Project Facts** for a **Project**.
_Avoid_: Project Model, analysis mode, environment

**Static Extraction**:
Source-based **Django Discovery** that derives **Project Facts** from source files without executing project code.
_Avoid_: introspection, runtime analysis, import-time analysis

**Project Introspection**:
Runtime-backed **Django Discovery** that asks Django or Python about the configured **Project** and is expected to shrink as **Static Extraction** matures.
_Avoid_: Static Extraction, runtime validation, rendering

**Django Model**:
A Python class in a **Project** that represents a Django ORM model.
_Avoid_: Model, Project Model, data model

**Model Graph**:
The language server's structural view of **Django Models** and their relationships.
_Avoid_: Project Facts, object graph, database schema

### Django template concepts

These terms belong to Django's template language and project behavior; the language server observes, models, and projects them.

**Template**:
A Django template source file in a **Project**.
_Avoid_: parsed template, template object, HTML file

**Template Directory**:
A filesystem directory Django searches for **Templates** by template name.
_Avoid_: template folder, template library, static directory

**Template Name**:
The path-like name Django uses to resolve a **Template** within template directories.
_Avoid_: file path, template path, filename

**Template Origin**:
A concrete **Template** matched for a **Template Name** during **Template Resolution**.
_Avoid_: resolved template, file path, template object

**Template Reference**:
A **Template Name** used in a **Template** to refer to another **Template**.
_Avoid_: file reference, import, include

**Template Resolution**:
Finding the **Template** identified by a **Template Name** using the **Project**'s template search order.
_Avoid_: path resolution, file lookup, import resolution

**Template Inheritance**:
A relationship where one **Template** extends another **Template** and overrides **Inheritance Blocks**.
_Avoid_: subclassing, inclusion, template import

**Template Tag Library**:
A Django template library loadable with `{% load %}` that provides template tags, filters, or both.
_Avoid_: Template Library, templatetag module, tag set, installed library

**Template Tag**:
A Django template source construct written inside `{% ... %}`.
_Avoid_: Tag Definition, block token, Template Tag Library

**Template Variable**:
A Django template source expression written inside `{{ ... }}`.
_Avoid_: Variable, Python variable, context variable, Variable Tag

**Template Filter**:
A source-level filter use in a **Template Variable** expression, written with `|`.
_Avoid_: Filter Definition, pipe, filter node

**Template Comment**:
A non-rendered Django template comment written with `{# ... #}`.
_Avoid_: Comment Tag, Template Comment Tag, Python comment

**Template Context**:
The runtime value mapping used by Django to resolve **Template Variables**.
_Avoid_: Project Facts, Python scope, variable environment

**Template Node**:
A parsed Django template unit before language-server tree structure is added.
_Avoid_: Template Branch, Template Leaf, syntax node

**Template Node List**:
The flat parser output made of **Template Nodes** before language-server tree structure is added.
_Avoid_: AST, Template Tree, parse tree

**Tag Bit**:
A whitespace-aware piece of a **Template Tag** after the tag name.
_Avoid_: argument, token, template argument

**Template Partial**:
A reusable template fragment that can be defined once and rendered later.
_Avoid_: Template Fragment as the canonical term, Inheritance Block, include

### Language server template model

These terms name how DJLS represents, classifies, and analyzes Django template concepts; they are not necessarily Django public terminology.

#### Definitions and availability

**Tag Definition**:
DJLS's representation of a known Django template tag, including analysis facts and a **Definition Source** when known.
_Avoid_: Template Tag, Tag Spec, Tag Role

**Filter Definition**:
DJLS's representation of a known Django template filter, including analysis facts and a **Definition Source** when known.
_Avoid_: Template Filter, filter use, pipe expression

**Definition Source**:
The source or provenance DJLS associates with a **Tag Definition** or **Filter Definition**.
_Avoid_: Tag Definition, Filter Definition, source occurrence

**Tag and Filter Availability**:
Whether a **Tag Definition** or **Filter Definition** is usable at a position in a **Template** under Django's builtin and `{% load %}` rules.
_Avoid_: Template Symbol Availability, Load Scope, Load Tag Scope

#### Tag structure

Tag structure describes the branch shape a **Tag Definition** creates, not what the tag means in the project.

**Block Tag**:
A **Tag Definition** whose **Tag Spec** defines body or branch structure and an associated **Closing Tag**.
_Avoid_: Tag Block, Block Template Tag when structural category is meant

**Standalone Tag**:
A **Tag Definition** whose **Tag Spec** defines no body or branch structure and no **Closing Tag**.
_Avoid_: simple tag, leaf tag, single tag

**Opening Tag**:
A **Template Tag** whose tag name resolves to a **Block Tag** and starts a **Template Branch**.
_Avoid_: Block Opener, opener tag, start tag

**Intermediate Tag**:
A contextual **Template Tag** that starts another **Branch Segment** inside an open **Template Branch**.
_Avoid_: Block Intermediate, intermediate block, branch tag

**Closing Tag**:
A contextual **Template Tag** that ends an open **Template Branch**.
_Avoid_: Block Closer, end tag, closer tag

**Tag Spec**:
The structural, parameter, and validation model for a **Tag Definition**.
_Avoid_: Tag Role, manual tag config, tag schema

**Tag Argument**:
A semantic input supplied by a **Template Tag** occurrence.
_Avoid_: Tag Bit, Tag Parameter, token

**Tag Parameter**:
A semantic input accepted by a **Tag Definition** according to its **Tag Spec**.
_Avoid_: Tag Argument, Tag Bit, token

#### Tag roles

Tag roles are independent of tag structure: a **Tag Definition** may be both a **Block Tag** and a **Reference Tag** or **Definition Tag**.

**Tag Role**:
DJLS's semantic classification of a **Tag Definition** when the tag creates a template-domain relationship or defines a named Django template concept.
_Avoid_: Tag Spec, structural position, Control Tag

**Reference Tag**:
A **Tag Definition** whose **Tag Role** creates a template-domain relationship from the current **Template** to something outside the local template body.
_Avoid_: Loader Tag, Project-Resolved Tag, relationship tag

**Template Reference Tag**:
A **Reference Tag** that points to another **Template**.
_Avoid_: template loader tag, include tag category, file reference tag

**Library Reference Tag**:
A **Reference Tag** that points to a **Template Tag Library**.
_Avoid_: Loader Tag, load tag category, import tag

**Static Asset Reference Tag**:
A **Reference Tag** that points to a static asset.
_Avoid_: static tag category, asset loader tag

**Route Reference Tag**:
A **Reference Tag** that points to a route or view name.
_Avoid_: URL tag category, route loader tag

**Definition Tag**:
A **Tag Definition** whose **Tag Role** defines a named Django template concept, such as an **Inheritance Block** or **Template Partial**.
_Avoid_: declaration tag, structural tag, Control Tag

**Block Template Tag**:
The specific **Definition Tag** named `block` that defines an **Inheritance Block**.
_Avoid_: Template Block Tag, Block Tag when the named tag is meant, tag block

**Inheritance Block**:
A named override or fill point in **Template Inheritance**.
_Avoid_: Template Branch, Block Tag, block region

#### Template structure

**Opaque Region**:
A range of template source whose contents the language server treats as a black box and does not expand or validate.
_Avoid_: ignored content, raw text, hidden block

**Template Tree**:
The language server's tree-shaped structural view of a **Template**, made of **Template Branches** and **Template Leaves**.
_Avoid_: AST, syntax tree, parse tree

**Template Branch**:
A nested tree structure created by a **Block Tag**.
_Avoid_: Tag Block, Template Node, Opaque Region

**Branch Segment**:
One part of a **Template Branch**, started by an **Opening Tag** or **Intermediate Tag**.
_Avoid_: segment node, intermediate block, branch

**Template Leaf**:
A non-branch item in a **Template Tree**, such as text, a **Template Variable**, a **Template Comment**, a **Standalone Tag**, or an error.
_Avoid_: Template Node, Standalone Tag, leaf node

#### Parser and analysis findings

**Parse Error**:
A parser finding that a **Template** cannot be fully read as valid Django template syntax.
_Avoid_: Validation Error, diagnostic, exception

**Validation Error**:
An analysis finding that a **Template**'s structure, references, or use of **Tag Definitions** or **Filter Definitions** is invalid.
_Avoid_: diagnostic, Parse Error, exception

### IDE projection

These terms name how DJLS exposes analyzed template meaning through IDE features.

**Template Symbol**:
A semantic template entity DJLS may expose to IDE features, such as an **Inheritance Block**, **Template Reference**, **Template Variable**, **Template Filter**, or **Template Partial**.
_Avoid_: raw token, Template Node, Tag Definition when library inventory is meant

**Template Outline**:
A navigational projection of editor-relevant **Template Symbols** from a **Template Tree**.
_Avoid_: Template Tree, document symbol, syntax tree

## Relationships

- **Project Facts** describe what the language server understands about a **Project**.
- A **Workspace** may contain files from one **Project**, multiple **Projects**, or no recognized **Project**.
- A **Project** contains one or more **Templates**, **Template Directories**, **Django Environments**, and **Template Tag Libraries**.
- A **Django Environment** is rooted at a directory and configured by a **Django Settings Module**.
- **Django Discovery** builds **Project Facts**.
- **Static Extraction** contributes source-derived **Project Facts** without executing project code.
- **Project Introspection** contributes runtime-derived **Project Facts** by asking Django or Python about the configured **Project**.
- A **Model Graph** describes relationships between **Django Models**.
- A **Template Directory** may contain **Templates** directly or under nested directories.
- **Template Resolution** maps a **Template Name** to at most one **Template** within the active template search order.
- A **Template Reference** is resolved through **Template Resolution**.
- **Template Inheritance** uses **Block Template Tags** to define **Inheritance Blocks**.
- A **Template Tag Library** provides Django template tags and filters.
- DJLS represents known template tags as **Tag Definitions** and known template filters as **Filter Definitions**.
- A **Tag Definition** or **Filter Definition** may have a **Definition Source**, such as a Python implementation, Django builtin, configuration entry, or unknown location.
- A **Tag Definition** has a **Tag Spec** and may have a **Tag Role**.
- A **Filter Definition** has validation facts such as arity, but no role vocabulary yet.
- **Tag and Filter Availability** may be available, unloaded, ambiguously unloaded, or unknown.
- Available definitions are usable at the position under Django's builtin and `{% load %}` rules.
- Unloaded definitions are known but require a **Template Tag Library** that is not loaded at the position.
- Ambiguously unloaded definitions are known from multiple inactive libraries, so DJLS cannot choose one `{% load %}` suggestion.
- Unknown tags or filters have no known **Tag Definition** or **Filter Definition** under current **Project Facts**.
- A **Tag Spec** classifies a **Tag Definition** as a **Block Tag** or **Standalone Tag**.
- A **Block Tag** creates a **Template Branch** when used as an **Opening Tag**.
- A **Block Tag**'s **Closing Tag** may be required or optional according to its **Tag Spec**.
- A **Standalone Tag** appears as a **Template Leaf** in a **Template Tree**.
- An **Opening Tag** is a **Template Tag** whose name resolves to a **Block Tag**.
- **Intermediate Tags** and **Closing Tags** are contextual **Template Tags**, not independent **Tag Definitions**.
- A **Block Tag**'s **Tag Spec** defines which **Intermediate Tags** and **Closing Tags** are valid in that block's **Template Branch**.
- An **Opening Tag** and each **Intermediate Tag** start a **Branch Segment**.
- A **Closing Tag** ends the open **Template Branch**.
- A **Template Branch** contains one or more **Branch Segments**.
- A **Branch Segment** may contain **Template Leaves** or nested **Template Branches**.
- A **Template Tree** gives language-server structure to a **Template Node List**.
- A **Template Outline** selects editor-relevant **Template Symbols** from a **Template Tree**.
- A **Template Tag** occurrence may become a **Template Symbol** and resolve to a **Tag Definition** when DJLS knows that tag name.
- A **Template Filter** occurrence may become a **Template Symbol** and resolve to a **Filter Definition** when DJLS knows that filter name.
- A **Template Symbol** may resolve to a **Tag Definition** or **Filter Definition**, but definitions and symbols are not the same concept.
- A **Template Variable** resolves against a **Template Context** at runtime.
- A **Template Tag** is parsed into a tag name and zero or more **Tag Bits**.
- A **Tag Bit** may correspond to a **Tag Argument**, but the terms are not interchangeable.
- A **Tag Argument** may satisfy a **Tag Parameter**, but arguments and parameters are not interchangeable.
- **Tag Role** is independent of whether a **Tag Definition** is a **Block Tag** or **Standalone Tag**.
- A **Reference Tag** creates a template-domain relationship from the current **Template** to something outside the local template body.
- A **Template Reference Tag** creates a **Template Reference**.
- A **Library Reference Tag** references a **Template Tag Library**.
- A **Static Asset Reference Tag** references a static asset.
- A **Route Reference Tag** references a route or view name.
- A **Definition Tag** defines a named Django template concept.
- A **Block Template Tag** defines an **Inheritance Block**.
- A **Definition Tag** may define a **Template Partial**.
- An **Opaque Region** suppresses expansion and validation for its contents.

## Example dialogue

> **Dev:** "Is `{% block content %}` a **Block Tag** or a **Block Template Tag**?"
> **Domain expert:** "Both, depending on context: DJLS represents `block` as a **Tag Definition** whose **Tag Spec** makes it a **Block Tag**, and whose **Tag Role** makes it the **Block Template Tag** that defines an **Inheritance Block**."

## Flagged ambiguities

- "Project" could mean the editor workspace, Cargo workspace, repository, or analyzed Django application; resolved: **Project** means the Django codebase being analyzed by the language server.
- "Project model", "project context", "project knowledge", and "project state" are not canonical terms; resolved: use **Project Facts**.
- "Django Environment" is a path-scoped Django analysis context, not a Python environment, virtual environment, or OS environment.
- "Template Library" can mean a collection of templates; resolved: use **Template Tag Library** for Django tag/filter libraries.
- "Installed Template Tag Library" is ambiguous between pip-installed packages and Django `INSTALLED_APPS`; resolved: describe a **Template Tag Library** as loadable (requires `{% load %}`) or builtin (preloaded). "Active" and "discovered" are not library availability states.
- "Tag" can mean source syntax, a known definition, structural behavior, or semantic role; resolved: use **Template Tag**, **Tag Definition**, **Tag Spec**, and **Tag Role** for those different contexts.
- "Definition" can mean the Python source that implements a tag or DJLS's model of a known tag; resolved: use **Definition Source** for provenance or navigation targets and **Tag Definition** or **Filter Definition** for the language server model.
- "Block" is overloaded by Django syntax, Django inheritance, parser internals, and tree structure; resolved: use **Block Tag** for the structural definition category, **Block Template Tag** for the named `block` tag, **Inheritance Block** for the thing it defines, and **Template Branch** for the tree structure.
- "Tag Block" looks like the inverse of **Block Tag** but does not clarify the model; resolved: avoid **Tag Block**.
- "Intermediate Tag" is DJLS glossary language for tags such as `elif`, `else`, `empty`, and `plural`; Django often describes these as clauses or parser stop tokens instead.
- "Loader Tag" is source-derived and overloaded by Django's `loader_tags.py`; resolved: use **Reference Tag** roles such as **Template Reference Tag** and **Library Reference Tag**.
- "Standalone Tag" is DJLS glossary language rather than Django's public terminology; resolved: use it for **Tag Definitions** with no body or closing tag.
- "Control Tag" exists in current implementation language but is not canonical domain vocabulary until DJLS has behavior that depends on that semantic distinction.
- **Tag and Filter Availability** follows Django's builtin and `{% load %}` rules; DJLS classifies availability only to explain those rules at a template position.
- **Template Context** is a Django runtime concept; DJLS has limited knowledge of it because the language server does not render templates and does not yet infer template variable types.
- Specific named tags use the `<Name> Template Tag` pattern when the name matters; resolved: define only names with domain significance or ambiguity, such as **Block Template Tag**.
- "Partial Tag" is too ambiguous between tags that define, render, or include partials; resolved: use **Template Partial** for the reusable fragment and existing tag-role terms for tags that interact with it.
- **Template Partial** is the canonical term for partials, while "template fragment" may be used in definitions because partials are reusable fragments.

## Known terminology drift

- Some code and docs use "block tag" for any paired tag with contents; this glossary uses **Block Tag** for the structural category and **Block Template Tag** for the specific `{% block %}` tag.
- Current code uses symbol language for tag/filter inventory; this glossary calls those known library and builtin facts **Tag Definitions** and **Filter Definitions**.
- Current code and docs may describe intermediate and closing tags as tag names in a **Tag Spec**; this glossary treats them as contextual **Template Tags** rather than independent **Tag Definitions**.
- Current code and outline snapshots may use `ControlTag`; this glossary does not make **Control Tag** canonical yet.
- Django groups `block`, `extends`, and `include` in `django.template.loader_tags`, but this glossary uses role terms instead of **Loader Tag** for user-facing domain language.
