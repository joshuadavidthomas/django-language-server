use camino::Utf8PathBuf;
use djls_source::File;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginSet {
    origins: Vec<Origin>,
}

impl OriginSet {
    #[must_use]
    pub fn single(origin: Origin) -> Self {
        Self {
            origins: vec![origin],
        }
    }

    #[must_use]
    pub fn origins(&self) -> &[Origin] {
        &self.origins
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Origin {
    Config {
        root: Utf8PathBuf,
    },
    ConfiguredEnvironment {
        root: Option<Utf8PathBuf>,
        name: Option<String>,
    },
    Environment {
        root: Utf8PathBuf,
        name: String,
    },
    PythonSource {
        file: File,
    },
    Convention {
        file: File,
    },
}
