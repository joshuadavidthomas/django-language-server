use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;

/// Exact absolute path data produced by the supported `pathlib` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PythonPath(Utf8PathBuf);

impl PythonPath {
    pub(crate) fn from_absolute_string(value: &str) -> Option<Self> {
        let path = Utf8Path::new(value);
        path.is_absolute().then(|| Self(path.to_path_buf()))
    }

    pub(crate) fn as_path(&self) -> &Utf8Path {
        &self.0
    }

    pub(crate) fn into_path_buf(self) -> Utf8PathBuf {
        self.0
    }

    pub(crate) fn parent(&self) -> Self {
        let parent = self.0.parent().unwrap_or_else(|| Utf8Path::new("/"));
        Self(parent.to_path_buf())
    }

    pub(crate) fn join(&self, segment: &str) -> Self {
        Self(self.0.join(segment))
    }

    pub(crate) fn resolve(&self) -> Self {
        let mut resolved = Utf8PathBuf::new();
        for component in self.0.components() {
            match component {
                Utf8Component::Prefix(prefix) => resolved.push(prefix.as_str()),
                Utf8Component::RootDir => resolved.push(Utf8Path::new("/")),
                Utf8Component::CurDir => {}
                Utf8Component::ParentDir => {
                    resolved.pop();
                }
                Utf8Component::Normal(component) => resolved.push(component),
            }
        }
        Self(resolved)
    }
}
