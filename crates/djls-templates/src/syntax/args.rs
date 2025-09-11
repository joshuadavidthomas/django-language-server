use crate::db::Db as TemplateDb;
use crate::syntax::tree::VariableName;

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct ParsedArgs<'db> {
    pub positional: Vec<ParsedArg<'db>>,
    pub named: Vec<(String, ParsedArg<'db>)>, // Use Vec instead of HashMap for Hash impl
}

impl<'db> ParsedArgs<'db> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            positional: Vec::new(),
            named: Vec::new(),
        }
    }

    pub fn add_positional(&mut self, arg: ParsedArg<'db>) {
        self.positional.push(arg);
    }

    pub fn add_named(&mut self, name: String, arg: ParsedArg<'db>) {
        self.named.push((name, arg));
    }

    /// Get positional argument by index
    #[must_use]
    pub fn get_positional(&self, index: usize) -> Option<&ParsedArg<'db>> {
        self.positional.get(index)
    }

    /// Get named argument by key
    #[must_use]
    pub fn get_named(&self, key: &str) -> Option<&ParsedArg<'db>> {
        self.named
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, arg)| arg)
    }

    /// Get total number of arguments (positional + named)
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.positional.len() + self.named.len()
    }

    /// Get count of positional arguments
    #[must_use]
    pub fn positional_count(&self) -> usize {
        self.positional.len()
    }

    /// Get count of named arguments
    #[must_use]
    pub fn named_count(&self) -> usize {
        self.named.len()
    }

    /// Check if arguments are empty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.positional.is_empty() && self.named.is_empty()
    }

    /// Get all named argument keys
    #[must_use]
    pub fn named_keys(&self) -> Vec<&String> {
        self.named.iter().map(|(name, _)| name).collect()
    }

    /// Iterate over positional arguments
    pub fn iter_positional(&self) -> impl Iterator<Item = &ParsedArg<'db>> {
        self.positional.iter()
    }

    /// Iterate over named arguments
    pub fn iter_named(&self) -> impl Iterator<Item = (&String, &ParsedArg<'db>)> {
        self.named.iter().map(|(name, arg)| (name, arg))
    }
}

impl Default for ParsedArgs<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Individual parsed argument with type information
#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum ParsedArg<'db> {
    Variable(VariableName<'db>),
    String(String),
    Expression(String), // TODO: Parse into expression AST
    Literal(String),
    Assignment { name: String, value: String },
}

impl<'db> ParsedArg<'db> {
    /// Get the string representation of this argument
    pub fn as_string(&self, db: &'db dyn TemplateDb) -> String {
        match self {
            ParsedArg::Variable(var) => format!("${{{}}}", var.text(db)),
            ParsedArg::String(s) => format!("\"{s}\""),
            ParsedArg::Expression(expr) => expr.clone(),
            ParsedArg::Literal(lit) => lit.clone(),
            ParsedArg::Assignment { name, value } => format!("{name}={value}"),
        }
    }

    /// Check if this argument is a variable
    #[must_use]
    pub fn is_variable(&self) -> bool {
        matches!(self, ParsedArg::Variable(_))
    }

    /// Check if this argument is a string literal
    #[must_use]
    pub fn is_string(&self) -> bool {
        matches!(self, ParsedArg::String(_))
    }

    /// Check if this argument is an expression
    #[must_use]
    pub fn is_expression(&self) -> bool {
        matches!(self, ParsedArg::Expression(_))
    }

    /// Check if this argument is a literal value
    #[must_use]
    pub fn is_literal(&self) -> bool {
        matches!(self, ParsedArg::Literal(_))
    }

    /// Check if this argument is an assignment
    #[must_use]
    pub fn is_assignment(&self) -> bool {
        matches!(self, ParsedArg::Assignment { .. })
    }

    /// Get the variable name if this is a variable argument
    #[must_use]
    pub fn as_variable(&self) -> Option<&VariableName<'db>> {
        match self {
            ParsedArg::Variable(var) => Some(var),
            _ => None,
        }
    }

    /// Get the assignment parts if this is an assignment argument
    #[must_use]
    pub fn as_assignment(&self) -> Option<(&str, &str)> {
        match self {
            ParsedArg::Assignment { name, value } => Some((name, value)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_args_operations() {
        let mut args = ParsedArgs::new();

        // Test adding positional args
        args.add_positional(ParsedArg::Literal("test".to_string()));
        assert_eq!(args.positional.len(), 1);
        assert_eq!(args.positional_count(), 1);

        // Test adding named args
        args.add_named("key".to_string(), ParsedArg::String("value".to_string()));
        assert_eq!(args.named.len(), 1);
        assert_eq!(args.named_count(), 1);
        assert!(args.get_named("key").is_some());

        // Test total count
        assert_eq!(args.total_count(), 2);
        assert!(!args.is_empty());

        // Test getters
        assert!(args.get_positional(0).is_some());
        assert!(args.get_positional(1).is_none());
        assert!(args.get_named("key").is_some());
        assert!(args.get_named("nonexistent").is_none());

        // Test named keys
        let keys = args.named_keys();
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&&"key".to_string()));
    }

    #[test]
    fn test_parsed_args_empty() {
        let args = ParsedArgs::new();
        assert!(args.is_empty());
        assert_eq!(args.total_count(), 0);
        assert_eq!(args.positional_count(), 0);
        assert_eq!(args.named_count(), 0);
    }

    #[test]
    fn test_parsed_arg_type_checking() {
        let literal_arg = ParsedArg::Literal("test".to_string());
        assert!(literal_arg.is_literal());
        assert!(!literal_arg.is_variable());
        assert!(!literal_arg.is_string());
        assert!(!literal_arg.is_expression());
        assert!(!literal_arg.is_assignment());

        let string_arg = ParsedArg::String("hello".to_string());
        assert!(string_arg.is_string());
        assert!(!string_arg.is_literal());

        let expr_arg = ParsedArg::Expression("user.name".to_string());
        assert!(expr_arg.is_expression());
        assert!(!expr_arg.is_string());

        let assign_arg = ParsedArg::Assignment {
            name: "key".to_string(),
            value: "value".to_string(),
        };
        assert!(assign_arg.is_assignment());
        assert!(!assign_arg.is_literal());

        // Test assignment getter
        if let Some((name, value)) = assign_arg.as_assignment() {
            assert_eq!(name, "key");
            assert_eq!(value, "value");
        } else {
            panic!("Assignment should return Some");
        }
    }
}
