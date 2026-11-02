// Integration tests share one binary so they link the dependency graph once.
mod code_actions;
mod completions;
mod diagnostics;
mod document_links;
mod folding;
mod formatting;
mod hover;
mod incremental;
mod navigation;
mod warmup;
