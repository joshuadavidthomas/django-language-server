// Integration tests share one binary so they link the dependency graph once.
mod bundled_django;
mod corpus_settings_redaction;
mod django_projects;
mod django_version;
mod extraction;
mod model_relations;
mod project_performance;
mod resolve;
mod settings;
mod settings_extraction;
mod template_libraries;
mod templates;
