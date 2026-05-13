mod db;
mod fixtures;
mod specs;

pub use db::Db;
pub use fixtures::model_fixtures;
pub use fixtures::python_fixtures;
pub use fixtures::template_fixtures;
pub use fixtures::validation_error_fixtures;
pub use fixtures::Fixture;
pub use fixtures::ModelFixture;
pub use fixtures::PythonFixture;
pub use fixtures::TemplateFixture;
pub use fixtures::ValidationErrorFixture;
pub use specs::realistic_db;

pub const BATCH_INNER_ITERS: usize = 8;
pub const REPEATED_INNER_ITERS: usize = 100;
pub const DIAGNOSTICS_INNER_ITERS: usize = BATCH_INNER_ITERS;
pub const DIAGNOSTICS_WARMUP_ITERS: usize = 3;

pub fn prime<F: FnMut()>(times: usize, mut f: F) {
    for _ in 0..times {
        f();
    }
}
