use djls_source::File;
use ruff_python_ast as ast;

use crate::python::evaluate_path;
use crate::settings::extraction::bindings::SettingsBindings;
use crate::settings::extraction::substrate::SettingsSource;
use crate::settings::types::EvaluatedPath;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalBindings;
use crate::settings::types::LocalListBinding;

pub(super) struct EvalEnv<'a> {
    source: &'a SettingsSource,
    locals: &'a LocalBindings,
    installed_apps: Option<&'a InstalledAppsSetting>,
}

impl<'a> EvalEnv<'a> {
    pub(super) fn new(source: &'a SettingsSource, bindings: &'a SettingsBindings) -> Self {
        Self {
            source,
            locals: &bindings.locals,
            installed_apps: bindings.installed_apps.as_ref(),
        }
    }

    pub(super) fn module_file(&self) -> File {
        self.source.file()
    }

    pub(super) fn installed_apps(&self) -> Option<&'a InstalledAppsSetting> {
        self.installed_apps
    }

    pub(super) fn local_list_binding(&self, name: &str) -> Option<&'a LocalListBinding> {
        self.locals.list_binding(name)
    }

    pub(super) fn evaluate_template_dir_path(&self, expr: &ast::Expr) -> EvaluatedPath {
        evaluate_path(expr, self.source.path(), self.locals.path_bindings())
            .map_or(EvaluatedPath::Unknown, EvaluatedPath::Resolved)
    }
}
