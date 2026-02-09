use std::collections::HashMap;

use serde::Serialize;

use crate::types::SplitPosition;

/// Tracks how a `token.split_contents()` result has been mutated.
///
/// Python compile functions commonly pop elements from the front (`bits.pop(0)`)
/// or back (`bits.pop()`) of the split result, or slice it (`bits[2:]`). These
/// mutations change the mapping between local indices and original positions.
///
/// `TokenSplit` encapsulates this offset arithmetic so callers use methods
/// instead of manually computing `index + base_offset + pops_from_end`.
// TODO(M15.22): Remove allow(dead_code) when SplitResult/SplitLength use TokenSplit
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TokenSplit {
    front_offset: usize,
    back_offset: usize,
}

#[allow(dead_code)]
impl TokenSplit {
    /// A fresh split result with no mutations applied.
    #[must_use]
    pub fn fresh() -> Self {
        Self {
            front_offset: 0,
            back_offset: 0,
        }
    }

    /// The split after `bits.pop(0)` — removes one element from the front.
    #[must_use]
    pub fn after_pop_front(&self) -> Self {
        Self {
            front_offset: self.front_offset + 1,
            back_offset: self.back_offset,
        }
    }

    /// The split after `bits.pop()` — removes one element from the back.
    #[must_use]
    pub fn after_pop_back(&self) -> Self {
        Self {
            front_offset: self.front_offset,
            back_offset: self.back_offset + 1,
        }
    }

    /// The split after `bits = bits[start:]` — shifts the front offset.
    #[must_use]
    pub fn after_slice_from(&self, start: usize) -> Self {
        Self {
            front_offset: self.front_offset + start,
            back_offset: self.back_offset,
        }
    }

    /// Convert a local index (into the current mutated list) to an original
    /// `SplitPosition` by adding the front offset.
    #[must_use]
    pub fn resolve_index(&self, local: usize) -> SplitPosition {
        SplitPosition::Forward(self.front_offset + local)
    }

    /// Convert a local `len()` measurement to the original argument count.
    ///
    /// If the mutated list has `local_length` elements, the original had
    /// `local_length + front_offset + back_offset`.
    #[must_use]
    pub fn resolve_length(&self, local_length: usize) -> usize {
        local_length + self.front_offset + self.back_offset
    }

    /// The number of elements removed from the front.
    #[must_use]
    pub fn front_offset(&self) -> usize {
        self.front_offset
    }

    /// The number of elements removed from the back.
    #[must_use]
    pub fn back_offset(&self) -> usize {
        self.back_offset
    }

    /// Total offset (front + back) for length adjustment.
    #[must_use]
    pub fn total_offset(&self) -> usize {
        self.front_offset + self.back_offset
    }
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
    SplitElement { index: SplitPosition },
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
    fn token_split_fresh() {
        let ts = TokenSplit::fresh();
        assert_eq!(ts.front_offset(), 0);
        assert_eq!(ts.back_offset(), 0);
        assert_eq!(ts.total_offset(), 0);
    }

    #[test]
    fn token_split_pop_front() {
        let ts = TokenSplit::fresh().after_pop_front();
        assert_eq!(ts.front_offset(), 1);
        assert_eq!(ts.back_offset(), 0);
        assert_eq!(ts.resolve_index(0), SplitPosition::Forward(1));
        assert_eq!(ts.resolve_length(3), 4);
    }

    #[test]
    fn token_split_pop_back() {
        let ts = TokenSplit::fresh().after_pop_back();
        assert_eq!(ts.front_offset(), 0);
        assert_eq!(ts.back_offset(), 1);
        assert_eq!(ts.resolve_index(0), SplitPosition::Forward(0));
        assert_eq!(ts.resolve_length(3), 4);
    }

    #[test]
    fn token_split_slice_from() {
        let ts = TokenSplit::fresh().after_slice_from(2);
        assert_eq!(ts.front_offset(), 2);
        assert_eq!(ts.resolve_index(0), SplitPosition::Forward(2));
        assert_eq!(ts.resolve_length(1), 3);
    }

    #[test]
    fn token_split_chained_mutations() {
        let ts = TokenSplit::fresh()
            .after_pop_front()
            .after_pop_back()
            .after_slice_from(1);
        assert_eq!(ts.front_offset(), 2);
        assert_eq!(ts.back_offset(), 1);
        assert_eq!(ts.total_offset(), 3);
        assert_eq!(ts.resolve_index(0), SplitPosition::Forward(2));
        assert_eq!(ts.resolve_length(2), 5);
    }
}
