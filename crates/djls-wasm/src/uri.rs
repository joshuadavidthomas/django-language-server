use camino::Utf8Path;
use camino::Utf8PathBuf;
use tower_lsp_server::ls_types;

/// Maps document URIs to database paths and back. Browser workspaces use their folder's own
/// scheme (`vscode-vfs://github/...`, `vscode-test-web://mount/...`), so each other scheme and
/// authority gets a directory, `/{scheme}/{authority}`, and paths under it map back to that
/// URI prefix. `file:` URIs map to their path, which also covers bundled Django sources
/// under `/djls-cache`.
#[derive(Default)]
pub(crate) struct UriMap {
    /// Directory and URI prefix (`scheme://authority` or `scheme:`) of each scheme seen.
    mounts: Vec<(Utf8PathBuf, String)>,
}

impl UriMap {
    pub(crate) fn path(uri: &ls_types::Uri) -> Option<Utf8PathBuf> {
        Self::mount(uri).map(|(path, _, _)| path)
    }

    /// Like `path`, remembering the URI's scheme so paths can map back to it.
    pub(crate) fn learn(&mut self, uri: &ls_types::Uri) -> Option<Utf8PathBuf> {
        let (path, mount, prefix) = Self::mount(uri)?;
        if let Some(mount) = mount
            && !self.mounts.iter().any(|(known, _)| *known == mount)
        {
            self.mounts.push((mount, prefix));
        }
        Some(path)
    }

    fn mount(uri: &ls_types::Uri) -> Option<(Utf8PathBuf, Option<Utf8PathBuf>, String)> {
        let path = percent_encoding::percent_decode_str(uri.path().as_str())
            .decode_utf8()
            .ok()?;
        let scheme = uri.scheme().as_str();
        if scheme == "file" {
            return path
                .starts_with('/')
                .then(|| (Utf8PathBuf::from(path.as_ref()), None, String::new()));
        }
        if !(path.is_empty() || path.starts_with('/')) {
            return None;
        }
        let (mount, prefix) = match uri.authority() {
            Some(authority) => (
                Utf8PathBuf::from(format!("/{scheme}/{}", authority.as_str())),
                format!("{scheme}://{}", authority.as_str()),
            ),
            None => (
                Utf8PathBuf::from(format!("/{scheme}")),
                format!("{scheme}:"),
            ),
        };
        let path = mount.join(path.trim_start_matches('/'));
        Some((path, Some(mount), prefix))
    }

    pub(crate) fn uri(&self, path: &Utf8Path) -> Option<ls_types::Uri> {
        let uri = self
            .mounts
            .iter()
            .filter_map(|(mount, prefix)| Some((prefix, path.strip_prefix(mount).ok()?)))
            .min_by_key(|(_, rest)| rest.as_str().len())
            .map_or_else(
                || {
                    format!(
                        "file://{}",
                        percent_encoding::utf8_percent_encode(path.as_str(), PATH)
                    )
                },
                |(prefix, rest)| {
                    format!(
                        "{prefix}/{}",
                        percent_encoding::utf8_percent_encode(rest.as_str(), PATH)
                    )
                },
            );
        uri.parse().ok()
    }
}

const PATH: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');
