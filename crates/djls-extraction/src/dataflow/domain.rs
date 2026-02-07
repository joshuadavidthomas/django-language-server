#![allow(dead_code)]

use std::collections::HashMap;

use serde::Serialize;

/// Positions within a `split_contents()` result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum Index {
    /// `bits[N]` — position from start, in `split_contents()` coordinates
    Forward(usize),
    /// `bits[-N]` — position from end
    Backward(usize),
}

/// Abstract representation of a Python value during dataflow analysis.
///
/// Each variant represents a class of runtime values that we can track
/// through the compile function body. `Unknown` is the safe default —
/// any value we can't track becomes Unknown, and constraints involving
/// Unknown values produce no output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AbstractValue {
    /// Untracked value — safe default, produces no constraints
    Unknown,
    /// The `token` parameter to the compile function
    Token,
    /// The `parser` parameter to the compile function
    Parser,
    /// Result of `token.split_contents()` or `token.contents.split()`.
    /// `base_offset` tracks mutations: after `bits.pop(0)`, offset becomes 1.
    /// After `bits = bits[2:]`, offset becomes 2.
    /// `pops_from_end` tracks `bits.pop()` calls (removing from the end).
    SplitResult {
        base_offset: usize,
        pops_from_end: usize,
    },
    /// Single element from a split result: `bits[N]` or `bits[-N]`
    SplitElement {
        index: Index,
    },
    /// `len(split_result)` — carries offsets for constraint adjustment.
    /// The effective original length = `measured_len + base_offset + pops_from_end`.
    SplitLength {
        base_offset: usize,
        pops_from_end: usize,
    },
    /// Integer constant
    Int(i64),
    /// String constant
    Str(String),
    /// Tuple of tracked values (for function return/destructuring)
    Tuple(Vec<AbstractValue>),
    /// List with known elements
    List(Vec<AbstractValue>),
}

/// The abstract environment: maps variable names to their abstract values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    bindings: HashMap<String, AbstractValue>,
}

impl Env {
    /// Create a new environment initialized for a compile function.
    ///
    /// Binds parameter names to `Parser` and `Token` respectively.
    #[must_use]
    pub fn for_compile_function(parser_param: &str, token_param: &str) -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(parser_param.to_string(), AbstractValue::Parser);
        bindings.insert(token_param.to_string(), AbstractValue::Token);
        Self { bindings }
    }

    /// Create a new environment with explicit bindings (for helper function inlining).
    #[must_use]
    pub fn with_bindings(bindings: HashMap<String, AbstractValue>) -> Self {
        Self { bindings }
    }

    /// Look up a variable's abstract value. Returns `Unknown` if not bound.
    #[must_use]
    pub fn get(&self, name: &str) -> &AbstractValue {
        self.bindings.get(name).unwrap_or(&AbstractValue::Unknown)
    }

    /// Bind a variable to an abstract value.
    pub fn set(&mut self, name: String, value: AbstractValue) {
        self.bindings.insert(name, value);
    }

    /// Mutate a variable's value in place (e.g., for `bits.pop(0)`).
    /// Returns `true` if the variable was found and mutated.
    pub fn mutate<F>(&mut self, name: &str, f: F) -> bool
    where
        F: FnOnce(&mut AbstractValue),
    {
        if let Some(val) = self.bindings.get_mut(name) {
            f(val);
            true
        } else {
            false
        }
    }

    /// Iterate over all bindings.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AbstractValue)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_for_compile_function() {
        let env = Env::for_compile_function("parser", "token");
        assert_eq!(env.get("parser"), &AbstractValue::Parser);
        assert_eq!(env.get("token"), &AbstractValue::Token);
        assert_eq!(env.get("nonexistent"), &AbstractValue::Unknown);
    }

    #[test]
    fn env_set_and_get() {
        let mut env = Env::default();
        env.set("x".to_string(), AbstractValue::Int(42));
        assert_eq!(env.get("x"), &AbstractValue::Int(42));
    }

    #[test]
    fn env_mutate() {
        let mut env = Env::default();
        env.set(
            "bits".to_string(),
            AbstractValue::SplitResult {
                base_offset: 0,
                pops_from_end: 0,
            },
        );
        let mutated = env.mutate("bits", |v| {
            if let AbstractValue::SplitResult { base_offset, .. } = v {
                *base_offset += 1;
            }
        });
        assert!(mutated);
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult {
                base_offset: 1,
                pops_from_end: 0
            }
        );
    }

    #[test]
    fn env_mutate_missing() {
        let mut env = Env::default();
        let mutated = env.mutate("missing", |_| {});
        assert!(!mutated);
    }

    #[test]
    fn env_with_bindings() {
        let mut bindings = HashMap::new();
        bindings.insert("a".to_string(), AbstractValue::Token);
        bindings.insert("b".to_string(), AbstractValue::Parser);
        let env = Env::with_bindings(bindings);
        assert_eq!(env.get("a"), &AbstractValue::Token);
        assert_eq!(env.get("b"), &AbstractValue::Parser);
    }
}
