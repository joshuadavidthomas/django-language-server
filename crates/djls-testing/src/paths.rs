//! Spell Unix-style fixture paths for the host OS.
//!
//! Tests write absolute fixture paths such as `/test/project/base.html`. Without a drive,
//! Windows does not consider such a path absolute: it has no `file://` URI, and Python's
//! `__file__` or `Path(...)` of it is not an exact path. These helpers give such fixtures a
//! `C:` drive and `\` separators on Windows, the spelling djls produces when it walks or joins
//! paths there, and return them unchanged elsewhere.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use camino::Utf8Component;
use camino::Utf8Path;

/// An absolute fixture path, such as `/test/project/base.html`, as an absolute host path.
#[must_use]
pub fn host_path(path: &'static str) -> &'static str {
    if !cfg!(windows) || !path.starts_with('/') {
        return path;
    }
    intern(Spelling::Path, path, || windows_path(path))
}

/// A Python fixture source with every single-quoted absolute path literal, such as
/// `'DIRS': ['/test/project/templates']`, spelled by [`host_path`].
#[must_use]
pub fn host_source(source: &'static str) -> &'static str {
    if !cfg!(windows) {
        return source;
    }
    intern(Spelling::Source, source, || {
        let mut rewritten = String::with_capacity(source.len());
        let mut rest = source;
        while let Some(start) = rest.find("'/") {
            let Some(len) = rest[start + 1..].find('\'') else {
                break;
            };
            let path = &rest[start + 1..start + 1 + len];
            rewritten.push_str(&rest[..start]);
            rewritten.push('\'');
            rewritten.push_str(&windows_path(path).replace('\\', "\\\\"));
            rewritten.push('\'');
            rest = &rest[start + len + 2..];
        }
        rewritten.push_str(rest);
        rewritten
    })
}

/// The `file://` URI of [`host_path`], with the drive colon percent-encoded as `ls_types`
/// and VS Code spell it. It is written out rather than produced by the URI
/// conversion under test.
#[must_use]
pub fn host_uri(path: &'static str) -> &'static str {
    intern(Spelling::Uri, path, || {
        if cfg!(windows) {
            format!("file:///C%3A{path}")
        } else {
            format!("file://{path}")
        }
    })
}

/// Render a path with `/` separators, so an expectation written as `/test/project/base.html`
/// matches the path djls built with the host separator.
#[must_use]
pub fn slash_path(path: impl AsRef<Utf8Path>) -> String {
    let mut rendered = String::new();
    for component in path.as_ref().components() {
        match component {
            Utf8Component::Prefix(prefix) => rendered.push_str(prefix.as_str()),
            Utf8Component::RootDir => rendered.push('/'),
            Utf8Component::CurDir | Utf8Component::ParentDir | Utf8Component::Normal(_) => {
                if !rendered.is_empty() && !rendered.ends_with('/') {
                    rendered.push('/');
                }
                rendered.push_str(component.as_str());
            }
        }
    }
    rendered
}

fn windows_path(path: &str) -> String {
    format!("C:{}", path.replace('/', "\\"))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Spelling {
    Path,
    Source,
    Uri,
}

/// Fixture paths are `&'static str` at their call sites, so each spelling is leaked once.
fn intern(
    spelling: Spelling,
    fixture: &'static str,
    spell: impl FnOnce() -> String,
) -> &'static str {
    static SPELLINGS: OnceLock<Mutex<HashMap<(Spelling, &'static str), &'static str>>> =
        OnceLock::new();
    let mut spellings = SPELLINGS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    spellings
        .entry((spelling, fixture))
        .or_insert_with(|| String::leak(spell()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_spellings_are_absolute_and_render_back_to_the_fixture() {
        let path = Utf8Path::new(host_path("/test/project/a.html"));
        assert!(path.is_absolute());
        let (rendered, uri, source) = if cfg!(windows) {
            (
                "C:/test/project/a.html",
                "file:///C%3A/test/project/a.html",
                r"DIRS = ['C:\\test\\project', 'app']",
            )
        } else {
            (
                "/test/project/a.html",
                "file:///test/project/a.html",
                "DIRS = ['/test/project', 'app']",
            )
        };
        assert_eq!(slash_path(path), rendered);
        assert_eq!(host_uri("/test/project/a.html"), uri);
        assert_eq!(host_source("DIRS = ['/test/project', 'app']"), source);
    }
}
