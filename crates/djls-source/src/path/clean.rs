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
}
