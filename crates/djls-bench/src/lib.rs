mod check;
mod db;
mod fixtures;
mod specs;

pub use check::CheckResult;
pub use check::FileCheckResult;
pub use check::check_file;
pub use check::render_validation_error;
pub use db::Db;
pub use fixtures::Fixture;
pub use fixtures::ValidationErrorFixture;
pub use fixtures::model_fixtures;
pub use fixtures::python_fixtures;
pub use fixtures::template_fixtures;
pub use fixtures::template_path;
pub use fixtures::validation_error_fixtures;
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
