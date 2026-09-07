use std::collections::HashMap;
use std::sync::Arc;

use djls_source::Span;

use crate::templates::tags::types::AssignmentMode;
use crate::templates::tags::types::SplitPosition;

/// Tracks how a `token.split_contents()` result has been mutated.
///
/// Python compile functions commonly pop elements from the front (`bits.pop(0)`)
/// or back (`bits.pop()`) of the split result, or slice it (`bits[2:]`). These
/// mutations change the mapping between local indices and original positions.
///
/// `TokenSplit` encapsulates this offset arithmetic so callers use methods
/// instead of manually computing `index + base_offset + pops_from_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TokenSplit {
    front_offset: usize,
    back_offset: usize,
}

impl TokenSplit {
    /// A fresh split result with no mutations applied.
    #[must_use]
    pub(crate) fn fresh() -> Self {
        Self {
            front_offset: 0,
            back_offset: 0,
        }
    }

    /// The split after `bits.pop(0)` — removes one element from the front.
    #[must_use]
    pub(crate) fn after_pop_front(&self) -> Self {
        Self {
            front_offset: self.front_offset + 1,
            back_offset: self.back_offset,
        }
    }

    /// The split after `bits.pop()` — removes one element from the back.
    #[must_use]
    pub(crate) fn after_pop_back(&self) -> Self {
        Self {
            front_offset: self.front_offset,
            back_offset: self.back_offset + 1,
        }
    }

    /// The split after `bits = bits[start:]` — shifts the front offset.
    #[must_use]
    pub(crate) fn after_slice_from(&self, start: usize) -> Self {
        Self {
            front_offset: self.front_offset + start,
            back_offset: self.back_offset,
        }
    }

    /// Convert a local index (into the current mutated list) to an original
    /// `SplitPosition` by adding the front offset.
    #[must_use]
    pub(crate) fn resolve_index(&self, local: usize) -> SplitPosition {
        SplitPosition::Forward(self.front_offset + local)
    }

    /// Convert a local `len()` measurement to the original argument count.
    ///
    /// If the mutated list has `local_length` elements, the original had
    /// `local_length + front_offset + back_offset`.
    #[must_use]
    pub(crate) fn resolve_length(&self, local_length: usize) -> usize {
        local_length + self.front_offset + self.back_offset
    }

    /// The number of elements removed from the front.
    #[must_use]
    pub(crate) fn front_offset(&self) -> usize {
        self.front_offset
    }

    /// The number of elements removed from the back.
    #[must_use]
    pub(crate) fn back_offset(&self) -> usize {
        self.back_offset
    }

    /// Total offset (front + back) for length adjustment.
    #[cfg(test)]
    #[must_use]
    fn total_offset(&self) -> usize {
        self.front_offset + self.back_offset
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AssignmentCall {
    pub file: djls_source::File,
    pub function: Span,
    pub call: Span,
    pub split: TokenSplit,
    pub mode: AssignmentMode,
}

/// Abstract representation of a Python value during analysis.
///
/// Each variant represents a class of runtime values that we can track
/// through the compile function body. `Unknown` is the safe default —
/// any value we can't track becomes Unknown, and constraints involving
/// Unknown values produce no output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AbstractValue {
    /// Untracked value — safe default, produces no constraints
    Unknown,
    /// The `token` parameter to the compile function
    Token,
    /// The `parser` parameter to the compile function
    Parser,
    /// Result of `token.split_contents()` or `token.contents.split()`.
    /// The `TokenSplit` tracks mutations (pop from front/back, slicing).
    SplitResult(TokenSplit),
    /// Single element from a split result: `bits[N]` or `bits[-N]`
    SplitElement {
        index: SplitPosition,
    },
    /// `len(split_result)` — carries offsets for constraint adjustment.
    /// The effective original length = `measured_len + split.total_offset()`.
    SplitLength(TokenSplit),
    /// Integer constant
    Int(i64),
    /// String constant
    Str(String),
    /// A comparison over the original `split_contents()` result.
    SplitPredicate(SplitPredicate),
    AssignmentMap(AssignmentCall),
    AssignmentRemainder(AssignmentCall),
    /// Tuple of tracked values (for function return/destructuring)
    Tuple(Vec<AbstractValue>),
}

impl AbstractValue {
    pub(crate) fn forget_mutable(&mut self) {
        match self {
            Self::SplitResult(_) | Self::AssignmentMap(_) | Self::AssignmentRemainder(_) => {
                *self = Self::Unknown;
            }
            Self::Tuple(values) => {
                for value in values {
                    value.forget_mutable();
                }
            }
            Self::Unknown
            | Self::Token
            | Self::Parser
            | Self::SplitElement { .. }
            | Self::SplitLength(_)
            | Self::Int(_)
            | Self::Str(_)
            | Self::SplitPredicate(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SplitPredicate {
    LengthEquals(usize),
    LengthAtLeast(usize),
    ElementEquals {
        position: SplitPosition,
        value: String,
    },
}

/// The abstract environment: maps variable names to their abstract values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Env {
    bindings: Arc<HashMap<String, AbstractValue>>,
    static_bindings: Arc<HashMap<String, AbstractValue>>,
    /// `None` means module name resolution is open. A closed set records names
    /// that shadow their Python builtins in this function.
    shadowed_builtin_names: Option<Arc<std::collections::HashSet<String>>>,
}

impl Env {
    /// Create a new environment initialized for a compile function.
    ///
    /// Binds parameter names to `Parser` and `Token` respectively.
    #[must_use]
    pub(crate) fn for_compile_function(parser_param: &str, token_param: &str) -> Self {
        let mut bindings = HashMap::new();
        bindings.insert(parser_param.to_string(), AbstractValue::Parser);
        bindings.insert(token_param.to_string(), AbstractValue::Token);
        Self {
            bindings: Arc::new(bindings),
            static_bindings: Arc::new(HashMap::new()),
            shadowed_builtin_names: None,
        }
    }

    /// Look up a variable's abstract value. A local binding, including
    /// `Unknown`, shadows a module constant.
    #[must_use]
    pub(crate) fn get(&self, name: &str) -> &AbstractValue {
        self.bindings
            .get(name)
            .or_else(|| self.static_bindings.get(name))
            .unwrap_or(&AbstractValue::Unknown)
    }

    /// Look up a statically resolved dotted path unless its root is shadowed
    /// by a function-local binding.
    #[must_use]
    pub(crate) fn get_static_path(&self, path: &[String]) -> Option<&AbstractValue> {
        let [root, ..] = path else {
            return None;
        };
        if self.bindings.contains_key(root) {
            return None;
        }
        self.static_bindings.get(&path.join("."))
    }

    /// Bind a variable to an abstract value.
    pub(crate) fn set(&mut self, name: String, value: AbstractValue) {
        Arc::make_mut(&mut self.bindings).insert(name, value);
    }

    /// Add a closed module or class constant. Local writes remain separate so
    /// they cannot accidentally reveal the static value after control flow.
    pub(crate) fn set_static(&mut self, name: String, value: AbstractValue) {
        Arc::make_mut(&mut self.static_bindings).insert(name, value);
    }

    /// Block static fallback for a local name without replacing a value already
    /// known at function entry, such as the parser or token parameter.
    pub(crate) fn shadow_static(&mut self, name: String) {
        Arc::make_mut(&mut self.bindings)
            .entry(name)
            .or_insert(AbstractValue::Unknown);
    }

    pub(crate) fn set_builtin_name_scope(
        &mut self,
        shadowed_names: std::collections::HashSet<String>,
    ) {
        self.shadowed_builtin_names = Some(Arc::new(shadowed_names));
    }

    #[must_use]
    pub(crate) fn builtin_name_visible(&self, name: &str) -> bool {
        self.shadowed_builtin_names
            .as_ref()
            .is_some_and(|shadowed| !shadowed.contains(name))
    }

    /// Mutate a variable's value in place (e.g., for `bits.pop(0)`).
    /// Returns `true` if the variable was found and mutated.
    pub(crate) fn mutate<F>(&mut self, name: &str, f: F) -> bool
    where
        F: FnOnce(&mut AbstractValue),
    {
        let Some(previous) = self.bindings.get(name).cloned() else {
            return false;
        };
        let mut value = previous.clone();
        f(&mut value);
        if value != previous {
            self.forget_aliases(&previous);
            self.set(name.to_string(), value);
        }
        true
    }

    /// Keep only bindings that have the same exact value on every branch.
    #[must_use]
    pub(crate) fn join_exact<'a>(branches: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut branches = branches.into_iter();
        let Some(first) = branches.next() else {
            return Self::default();
        };
        let mut joined = first.clone();
        for branch in branches {
            if !Arc::ptr_eq(&joined.bindings, &branch.bindings)
                && joined
                    .bindings
                    .iter()
                    .any(|(name, value)| branch.bindings.get(name) != Some(value))
            {
                Arc::make_mut(&mut joined.bindings)
                    .retain(|name, value| branch.bindings.get(name) == Some(value));
            }
            if !joined.static_bindings.is_empty()
                && branch.static_bindings != joined.static_bindings
            {
                joined.static_bindings = Arc::new(HashMap::new());
            }
        }
        joined
    }

    /// Forget a binding that may have changed before an implicit exception.
    pub(crate) fn forget(&mut self, name: &str) {
        if !self.bindings.contains_key(name) {
            return;
        }
        if let Some(value) = Arc::make_mut(&mut self.bindings).get_mut(name) {
            *value = AbstractValue::Unknown;
        }
    }

    /// Forget every binding that identifies the source token.
    pub(crate) fn forget_tokens(&mut self) {
        if !self
            .bindings
            .values()
            .any(|value| matches!(value, AbstractValue::Token))
        {
            return;
        }
        for value in Arc::make_mut(&mut self.bindings).values_mut() {
            if matches!(value, AbstractValue::Token) {
                *value = AbstractValue::Unknown;
            }
        }
    }

    /// Equal mutable values may share a Python object. Invalidate those aliases
    /// before changing one binding; different slices can keep their evidence.
    pub(crate) fn forget_aliases(&mut self, target: &AbstractValue) {
        fn contains(value: &AbstractValue, target: &AbstractValue) -> bool {
            value == target
                || matches!(value, AbstractValue::Tuple(values)
                    if values.iter().any(|value| contains(value, target)))
        }
        fn forget(value: &mut AbstractValue, target: &AbstractValue) {
            if value == target {
                *value = AbstractValue::Unknown;
            } else if let AbstractValue::Tuple(values) = value {
                for value in values {
                    forget(value, target);
                }
            }
        }

        match target {
            AbstractValue::Tuple(values) => {
                for value in values {
                    self.forget_aliases(value);
                }
                return;
            }
            AbstractValue::SplitResult(_)
            | AbstractValue::AssignmentMap(_)
            | AbstractValue::AssignmentRemainder(_) => {}
            AbstractValue::Unknown
            | AbstractValue::Token
            | AbstractValue::Parser
            | AbstractValue::SplitElement { .. }
            | AbstractValue::SplitLength(_)
            | AbstractValue::Int(_)
            | AbstractValue::Str(_)
            | AbstractValue::SplitPredicate(_) => return,
        }

        if self.bindings.values().any(|value| contains(value, target)) {
            for value in Arc::make_mut(&mut self.bindings).values_mut() {
                forget(value, target);
            }
        }
    }

    /// Forget every mutable token-derived sequence, including aliases nested
    /// in a tuple or mutable literal container.
    pub(crate) fn forget_split_results(&mut self) {
        fn contains_split_result(value: &AbstractValue) -> bool {
            match value {
                AbstractValue::SplitResult(_)
                | AbstractValue::AssignmentMap(_)
                | AbstractValue::AssignmentRemainder(_) => true,
                AbstractValue::Tuple(values) => values.iter().any(contains_split_result),
                AbstractValue::Unknown
                | AbstractValue::Token
                | AbstractValue::Parser
                | AbstractValue::SplitElement { .. }
                | AbstractValue::SplitLength(_)
                | AbstractValue::Int(_)
                | AbstractValue::Str(_)
                | AbstractValue::SplitPredicate(_) => false,
            }
        }

        if !self.bindings.values().any(contains_split_result) {
            return;
        }
        for value in Arc::make_mut(&mut self.bindings).values_mut() {
            value.forget_mutable();
        }
    }

    /// Iterate over all bindings.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &AbstractValue)> {
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
    fn env_clone_isolated_on_write() {
        let mut original = Env::default();
        original.set("x".to_string(), AbstractValue::Int(1));
        let mut branch = original.clone();

        branch.set("x".to_string(), AbstractValue::Int(2));

        assert_eq!(original.get("x"), &AbstractValue::Int(1));
        assert_eq!(branch.get("x"), &AbstractValue::Int(2));
    }

    #[test]
    fn env_mutate() {
        let mut env = Env::default();
        env.set(
            "bits".to_string(),
            AbstractValue::SplitResult(TokenSplit::fresh()),
        );
        let mutated = env.mutate("bits", |v| {
            if let AbstractValue::SplitResult(split) = v {
                *split = split.after_pop_front();
            }
        });
        assert!(mutated);
        assert_eq!(
            env.get("bits"),
            &AbstractValue::SplitResult(TokenSplit::fresh().after_pop_front())
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
