mod check;
mod db;
mod fixtures;
mod specs;

pub use check::MANY_ERRORS_SOURCE;
pub use check::synthetic_render_diagnostics;
pub use db::Db;
pub use fixtures::CorpusLoadError;
pub use fixtures::CorpusTemplates;
pub use fixtures::Fixture;
pub use fixtures::ValidationErrorFixture;
pub use fixtures::django_corpus_templates;
pub use fixtures::full_corpus_templates;
pub use fixtures::model_fixtures;
pub use fixtures::python_fixtures;
pub use fixtures::template_fixtures;
pub use fixtures::validation_error_fixtures;
pub use specs::primed_realistic_db;
pub use specs::realistic_db;
pub use specs::structure_db;

pub const BATCH_INNER_ITERS: usize = 8;
pub const REPEATED_INNER_ITERS: usize = 100;
pub const DIAGNOSTICS_INNER_ITERS: usize = BATCH_INNER_ITERS;
pub const DIAGNOSTICS_WARMUP_ITERS: usize = 3;

pub fn prime<F: FnMut()>(times: usize, mut f: F) {
    for _ in 0..times {
        f();
    }
}
