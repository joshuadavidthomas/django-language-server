use camino::Utf8Path;
use ruff_python_ast as ast;

use crate::python::evaluate_path;
use crate::settings::extraction::bindings::SettingsBindings;
use crate::settings::types::InstalledAppsSetting;
use crate::settings::types::LocalBindings;
use crate::settings::types::LocalListBinding;
use crate::settings::types::TemplateDirPath;

pub(super) struct EvalEnv<'a> {
    module_path: &'a Utf8Path,
    locals: &'a LocalBindings,
    installed_apps: Option<&'a InstalledAppsSetting>,
}

impl<'a> EvalEnv<'a> {
    pub(super) fn new(module_path: &'a Utf8Path, bindings: &'a SettingsBindings) -> Self {
        Self {
            module_path,
            locals: &bindings.locals,
            installed_apps: bindings.installed_apps.as_ref(),
        }
    }

    pub(super) fn installed_apps(&self) -> Option<&'a InstalledAppsSetting> {
        self.installed_apps
    }

    pub(super) fn local_list_binding(&self, name: &str) -> Option<&'a LocalListBinding> {
        self.locals.list_binding(name)
    }

    pub(super) fn evaluate_template_dir_path(&self, expr: &ast::Expr) -> TemplateDirPath {
        evaluate_path(expr, self.module_path, self.locals.path_bindings())
            .map_or(TemplateDirPath::Unknown, TemplateDirPath::Resolved)
    }
}
