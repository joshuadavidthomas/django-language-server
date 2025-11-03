# Django Language Server Roadmap

This roadmap outlines the path toward feature parity with typical language servers and Django-specific enhancements.

## Current Status (v5.2.3)

**Implemented Features:**
- ✅ **Completions** - Context-aware template tag autocompletion with snippets
  - Tag names after `{%`
  - Tag arguments
  - Library names after `{% load`
- ✅ **Diagnostics** - Real-time semantic validation
  - Parser errors (T100 series)
  - Semantic errors (S100-S107 series)
  - Unclosed tags, unbalanced structures, invalid arguments
- ✅ **Go to Definition** - Navigate to templates in `{% extends %}` and `{% include %}`
- ✅ **Find References** - Locate all usages of templates

## Phase 1: Complete Core LSP Features

### 1.1 Enhance Existing Features

**Priority: HIGH**

- [ ] **Expand Completions**
  - [ ] Filter completions after `|` operator
  - [ ] Variable completions in `{{ }}` context
  - [ ] Context variable completions (from Django views)
  - [ ] Template tag argument completions with choices
  - [ ] Built-in filter completions
  - [ ] Custom filter completions from loaded libraries

- [ ] **Enhance Go to Definition**
  - [ ] Jump to block definitions (from `{% endblock name %}` to `{% block name %}`)
  - [ ] Jump to filter definitions
  - [ ] Jump to custom tag definitions in Python
  - [ ] Jump to context variable definitions (if view can be inferred)
  - [ ] Navigate to static files referenced in `{% static %}` tags
  - [ ] Navigate to URL patterns from `{% url %}` tags

- [ ] **Expand Find References**
  - [ ] Find all references to blocks across templates
  - [ ] Find all usages of custom tags
  - [ ] Find all usages of filters

### 1.2 New Core Features

**Priority: HIGH**

- [ ] **Hover Documentation**
  - [ ] Show documentation for template tags
  - [ ] Show documentation for filters
  - [ ] Display context variable types and values (when available)
  - [ ] Show resolved template paths for extends/include
  - [ ] Display Django version compatibility info for tags/filters
  - [ ] Show tag/filter signatures

- [ ] **Signature Help**
  - [ ] Parameter hints for template tags while typing
  - [ ] Argument hints for filters
  - [ ] Show required vs optional arguments
  - [ ] Display argument types and descriptions

- [ ] **Document Symbols** (Outline View)
  - [ ] List all blocks in current template
  - [ ] List all includes/extends
  - [ ] List all custom tags used
  - [ ] Hierarchical view of block inheritance
  - [ ] Navigate to symbols via outline

## Phase 2: Enhanced Development Experience

### 2.1 Code Intelligence

**Priority: MEDIUM-HIGH**

- [ ] **Code Actions (Quick Fixes)**
  - [ ] Auto-close unclosed tags
  - [ ] Add missing `{% load %}` for unrecognized tags
  - [ ] Convert single quotes to double quotes (and vice versa)
  - [ ] Add missing arguments to tags
  - [ ] Suggest corrections for misspelled tag names
  - [ ] Extract repeated template code to include
  - [ ] Add missing `{% csrf_token %}` in forms
  - [ ] Convert hardcoded URLs to `{% url %}` tags

- [ ] **Formatting**
  - [ ] Auto-format template files
  - [ ] Configurable indentation (spaces/tabs)
  - [ ] Respect Django coding style
  - [ ] Format template tag arguments
  - [ ] Organize `{% load %}` statements
  - [ ] Consistent quote style

- [ ] **Workspace Symbols**
  - [ ] Search for blocks across all templates
  - [ ] Search for templates by name
  - [ ] Find all custom tag usages project-wide
  - [ ] Search for specific filters usage
  - [ ] Fast fuzzy search across workspace

### 2.2 Refactoring

**Priority: MEDIUM**

- [ ] **Rename**
  - [ ] Rename blocks across inheritance chain
  - [ ] Rename template files (update all references)
  - [ ] Rename context variables (if scope can be determined)
  - [ ] Update all extends/include references

## Phase 3: Django-Specific Features

### 3.1 Template-Specific Intelligence

**Priority: MEDIUM-HIGH**

- [ ] **Template Inheritance Analysis**
  - [ ] Visualize template inheritance tree
  - [ ] Show which blocks are overridden
  - [ ] Warn about orphaned blocks (not in parent)
  - [ ] Detect circular inheritance
  - [ ] Show complete block resolution order

- [ ] **Context Variable Intelligence**
  - [ ] Infer context from URL patterns → views → templates
  - [ ] Type inference for common Django objects (User, QuerySet, etc.)
  - [ ] Warn about undefined variables in template
  - [ ] Autocomplete model fields in templates
  - [ ] Show available methods on Django model instances

- [ ] **Static Files & Media**
  - [ ] Go to definition for `{% static %}` references
  - [ ] Diagnostics for missing static files
  - [ ] Autocomplete static file paths
  - [ ] Preview images on hover
  - [ ] Validate media paths

### 3.2 Forms & URLs

**Priority: MEDIUM**

- [ ] **Form Integration**
  - [ ] Detect Django forms in context
  - [ ] Autocomplete form field names
  - [ ] Validate form field usage
  - [ ] Warn about missing CSRF tokens
  - [ ] Suggest form rendering shortcuts

- [ ] **URL Integration**
  - [ ] Validate URL pattern names in `{% url %}`
  - [ ] Autocomplete URL pattern names
  - [ ] Show URL pattern parameters
  - [ ] Go to definition for URL patterns
  - [ ] Detect reversed URL arguments mismatches

### 3.3 Internationalization (i18n)

**Priority: MEDIUM**

- [ ] **Translation Support**
  - [ ] Validate `{% trans %}` and `{% blocktrans %}` usage
  - [ ] Extract strings for translation
  - [ ] Show available translations on hover
  - [ ] Warn about untranslated strings
  - [ ] Autocomplete language codes
  - [ ] Integration with Django's makemessages

### 3.4 Security & Best Practices

**Priority: MEDIUM-HIGH**

- [ ] **Security Linting**
  - [ ] Warn about `autoescape off` usage
  - [ ] Detect potential XSS vulnerabilities
  - [ ] Warn about missing CSRF tokens in forms
  - [ ] Flag unsafe template filters (e.g., `safe`, `safeseq`)
  - [ ] Detect SQL injection risks in raw queries
  - [ ] Suggest `select_related`/`prefetch_related` for relationships

- [ ] **Performance Hints**
  - [ ] Detect potential N+1 query patterns in templates
  - [ ] Warn about expensive operations in loops
  - [ ] Suggest template fragment caching
  - [ ] Detect unused `{% load %}` statements

## Phase 4: Advanced Features

### 4.1 Testing & Debugging

**Priority: LOW-MEDIUM**

- [ ] **Template Testing**
  - [ ] Integration with Django's template testing
  - [ ] Test context variable resolution
  - [ ] Validate template rendering without server
  - [ ] Show rendered output in preview

- [ ] **Debugging Support**
  - [ ] Set breakpoints in templates (if debugger supports)
  - [ ] Inspect context variables during debugging
  - [ ] Step through template rendering
  - [ ] Show Django Debug Toolbar integration hints

### 4.2 Django ORM Support

**Priority: LOW-MEDIUM**

This could be a major expansion beyond templates:

- [ ] **Model Awareness**
  - [ ] Completions for model fields in Python
  - [ ] Validate model relationships
  - [ ] Detect N+1 queries in views
  - [ ] Suggest query optimizations
  - [ ] Show database schema on hover

- [ ] **Query Analysis**
  - [ ] Syntax highlighting for ORM queries
  - [ ] Show SQL preview on hover
  - [ ] Warn about inefficient queries
  - [ ] Suggest indexes for common queries

### 4.3 Django Settings & Configuration

**Priority: LOW**

- [ ] **Settings Intelligence**
  - [ ] Autocomplete Django settings
  - [ ] Validate settings values
  - [ ] Show settings documentation on hover
  - [ ] Detect deprecated settings
  - [ ] Environment-specific settings validation

### 4.4 Third-Party Integration

**Priority: LOW**

- [ ] **Popular Package Support**
  - [ ] Django REST Framework serializers
  - [ ] django-crispy-forms templates
  - [ ] django-tables2 templates
  - [ ] django-filters integration
  - [ ] Celery task definitions
  - [ ] django-allauth templates

## Infrastructure & Quality

### Ongoing Improvements

- [ ] **Performance**
  - [ ] Optimize large file parsing
  - [ ] Incremental parsing improvements
  - [ ] Reduce memory footprint
  - [ ] Faster workspace indexing
  - [ ] Cache Django introspection results

- [ ] **Testing**
  - [ ] Comprehensive test suite for all LSP features
  - [ ] Integration tests with real Django projects
  - [ ] Performance benchmarks
  - [ ] Cross-editor compatibility tests

- [ ] **Documentation**
  - [ ] User guide for all features
  - [ ] Configuration documentation
  - [ ] Architecture documentation for contributors
  - [ ] Video tutorials and demos
  - [ ] Migration guides for breaking changes

- [ ] **Editor Support**
  - [ ] VS Code extension (dedicated)
  - [ ] Sublime Text package
  - [ ] IntelliJ/PyCharm plugin
  - [ ] Emacs package
  - [ ] Helix configuration
  - [ ] Improved Neovim integration

## Decision Points

### Open Questions

1. **Scope of ORM Support**: Should the language server expand beyond templates to provide comprehensive Django development support?

2. **Formatter Strategy**: Should formatting follow djLint, custom rules, or be configurable?

3. **View-Template Connection**: How aggressive should we be in inferring template context from views? Some views have complex context building.

4. **Static Analysis Depth**: How far should we go with security and performance analysis? False positives vs. false negatives tradeoff.

5. **Multi-file Analysis**: Should we analyze entire template inheritance chains at once, or focus on single-file performance?

## Contribution Priorities

If you're looking to contribute, these areas would have high impact:

1. **Hover Documentation** - Relatively straightforward, huge UX improvement
2. **Code Actions** - Great for new contributors, clear scope
3. **Signature Help** - Builds on existing tag spec system
4. **Document Symbols** - Useful feature, moderate complexity
5. **Django Context Inference** - Complex but very valuable
6. **Formatting** - Could integrate or build on djLint

## Version Planning

### Near Term (v5.1.x - v5.2.x)

Focus on completing Phase 1 core LSP features:
- Hover documentation
- Signature help
- Enhanced completions (filters, variables)
- Document symbols

### Medium Term (v6.0.x)

Focus on Phase 2 enhanced experience:
- Code actions
- Formatting
- Workspace symbols
- Basic refactoring

### Long Term (v6.1.x+)

Focus on Phase 3 Django-specific intelligence:
- Template inheritance analysis
- Context variable inference
- Security linting
- Performance hints

## Success Metrics

- Feature parity with Python language servers (Pyright, Pylance) for template files
- Adoption by Django community (downloads, stars, editor integrations)
- Reduction in common template errors caught early
- Positive developer experience feedback
- Performance: Sub-second response times for all operations

## Related Resources

- [LSP Specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
- [djLint](https://www.djlint.com/) - Django template linter/formatter
- [django-stubs](https://github.com/typeddjango/django-stubs) - Django type hints
- [Pyright](https://github.com/microsoft/pyright) - Reference Python language server
- [rust-analyzer](https://github.com/rust-lang/rust-analyzer) - Reference Rust language server architecture
