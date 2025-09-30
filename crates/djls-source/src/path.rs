//! Vendored and adapted from `path-clean` crate, <https://github.com/danreeves/path-clean>
//!
//! path-clean LICENSE-MIT:
//! Copyright (c) 2018 Dan Reeves
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN

use std::path::Component;

use camino::Utf8Path;
use camino::Utf8PathBuf;

pub trait Utf8PathClean {
    fn clean(&self) -> Utf8PathBuf;
}

impl Utf8PathClean for Utf8Path {
    fn clean(&self) -> Utf8PathBuf {
        clean_utf8_path(self)
    }
}

impl Utf8PathClean for Utf8PathBuf {
    fn clean(&self) -> Utf8PathBuf {
        clean_utf8_path(self)
    }
}

pub fn clean_utf8_path(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Vec::new();

    for comp in path.as_std_path().components() {
        match comp {
            Component::CurDir => (),
            Component::ParentDir => match out.last() {
                Some(Component::RootDir) => (),
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                None | Some(Component::CurDir | Component::ParentDir | Component::Prefix(_)) => {
                    out.push(comp);
                }
            },
            comp => out.push(comp),
        }
    }

    if out.is_empty() {
        Utf8PathBuf::from(".")
    } else {
        let cleaned: std::path::PathBuf = out.iter().collect();
        Utf8PathBuf::from_path_buf(cleaned).expect("Path should still be UTF-8")
    }
}

/// Django's `safe_join` equivalent - join paths and ensure result is within base
pub fn safe_join(base: &Utf8Path, name: &str) -> Result<Utf8PathBuf, SafeJoinError> {
    let candidate = base.join(name);
    let cleaned = clean_utf8_path(&candidate);

    if cleaned.starts_with(base) {
        Ok(cleaned)
    } else {
        Err(SafeJoinError::OutsideBase {
            base: base.to_path_buf(),
            attempted: name.to_string(),
            resolved: cleaned,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SafeJoinError {
    #[error("Path '{attempted}' would resolve to '{resolved}' which is outside base '{base}'")]
    OutsideBase {
        base: Utf8PathBuf,
        attempted: String,
        resolved: Utf8PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_removes_dots() {
        assert_eq!(
            clean_utf8_path(Utf8Path::new("hello/world/..")),
            Utf8PathBuf::from("hello")
        );
    }

    #[test]
    fn test_safe_join_allows_normal_path() {
        let base = Utf8Path::new("/templates");
        assert_eq!(
            safe_join(base, "myapp/base.html").unwrap(),
            Utf8PathBuf::from("/templates/myapp/base.html")
        );
    }

    #[test]
    fn test_safe_join_blocks_parent_escape() {
        let base = Utf8Path::new("/templates");
        assert!(safe_join(base, "../../etc/passwd").is_err());
    }
}
