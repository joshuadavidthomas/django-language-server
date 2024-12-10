#[macro_export]
macro_rules! include_script {
    ($name:expr) => {
        include_str!(concat!(
            env!("CARGO_WORKSPACE_DIR"),
            "python/djls/scripts/",
            $name,
            ".py"
        ))
    };
}

pub const HAS_IMPORT: &str = include_script!("has_import");
pub const PYTHON_SETUP: &str = include_script!["python_setup"];
