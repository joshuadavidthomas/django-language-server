//! Type system for Django template variables
//! Provides Python-like types with inference and interning

use rustc_hash::FxHashMap;

use crate::interned::VariablePath;

/// Python-like type representation for variables
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type<'db> {
    /// Unknown type
    Any,

    /// Python None
    None,

    /// String type
    String,

    /// Integer type
    Int,

    /// Float type
    Float,

    /// Boolean type
    Bool,

    /// List with element type
    List(Box<Type<'db>>),

    /// Dictionary type
    Dict(DictType<'db>),

    /// Object with known attributes
    Object(ObjectType<'db>),

    /// Union of multiple types
    Union(UnionType<'db>),
}

/// Dictionary type with key and value types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DictType<'db> {
    pub key_type: Box<Type<'db>>,
    pub value_type: Box<Type<'db>>,
}

/// Object type with known attributes - interned for deduplication
#[salsa::interned(debug)]
pub struct ObjectType<'db> {
    /// Class/type name
    pub name: String,

    /// Known attributes and their types (stored as sorted vec for hashing)
    #[returns(ref)]
    pub attributes: Vec<(String, Type<'db>)>,
}

/// Union of multiple types - interned for deduplication
#[salsa::interned(debug)]
pub struct UnionType<'db> {
    /// Possible types
    #[returns(ref)]
    pub types: Vec<Type<'db>>,
}

/// Interned type for caching type inference results
#[salsa::interned(debug)]
pub struct InternedType<'db> {
    pub inner: Type<'db>,
}

/// Variable scope map for tracked caching
#[salsa::tracked]
pub struct VariableScopeMap<'db> {
    /// Variables as a sorted vec for deterministic ordering
    #[returns(ref)]
    pub variables: Vec<(VariablePath<'db>, InternedType<'db>)>,
}

/// Context for variable type inference
pub struct TypeContext<'db> {
    /// Variables in the current scope
    pub variables: FxHashMap<VariablePath<'db>, Type<'db>>,

    /// Loop variables with their iterator types
    pub loop_vars: FxHashMap<String, Type<'db>>,

    /// Parent context (for template inheritance)
    pub parent: Option<Box<TypeContext<'db>>>,
}

impl<'db> TypeContext<'db> {
    /// Create a new empty type context
    pub fn new() -> Self {
        Self {
            variables: FxHashMap::default(),
            loop_vars: FxHashMap::default(),
            parent: None,
        }
    }

    /// Look up a variable's type in this context
    pub fn lookup(&self, path: &VariablePath<'db>) -> Option<&Type> {
        self.variables
            .get(path)
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup(path)))
    }

    /// Add a variable to the context
    pub fn add_variable(&mut self, path: VariablePath<'db>, ty: Type<'db>) {
        self.variables.insert(path, ty);
    }

    /// Enter a loop scope with a loop variable
    pub fn enter_loop(&mut self, var_name: String, iterator_type: Type<'db>) {
        // Infer element type from iterator
        let element_type = match iterator_type {
            Type::List(elem) => *elem,
            _ => Type::Any,
        };

        self.loop_vars.insert(var_name, element_type);
    }

    /// Exit a loop scope
    pub fn exit_loop(&mut self, var_name: &str) {
        self.loop_vars.remove(var_name);
    }
}

/// Infer the type of a variable with cycle recovery
#[salsa::tracked]
pub fn infer_variable_type<'db>(
    db: &'db dyn crate::Db,
    template: crate::inheritance::ResolvedTemplate<'db>,
    var_path: VariablePath<'db>,
) -> InternedType<'db> {
    // TODO: Implement actual type inference
    // This would:
    // 1. Check loop scopes for the variable
    // 2. Check template context
    // 3. Check parent template contexts
    // 4. Use inspector for Django model inference (future)

    // For now, return Any as placeholder
    InternedType::new(db, Type::Any)
}



/// Get all variables in scope at a given offset
#[salsa::tracked]
pub fn variables_in_scope<'db>(
    db: &'db dyn crate::Db,
    template: crate::inheritance::ResolvedTemplate<'db>,
    offset: u32,
) -> VariableScopeMap<'db> {
    // TODO: Implement scope analysis
    // This would:
    // 1. Find the element at the offset
    // 2. Determine which blocks/loops contain it
    // 3. Collect all visible variables
    // 4. Handle shadowing rules

    // For now, return empty map as placeholder
    VariableScopeMap::new(db, Vec::new())
}

impl<'db> Default for Type<'db> {
    fn default() -> Self {
        Type::Any
    }
}

impl<'db> Type<'db> {
    /// Check if this type can be iterated
    pub fn is_iterable(&self) -> bool {
        matches!(self, Type::List(_) | Type::Dict(_) | Type::String)
    }

    /// Check if this type is truthy (for if conditions)
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Type::None)
    }

    /// Get the element type for iterables
    pub fn element_type(&self) -> Type {
        match self {
            Type::List(elem) => (**elem).clone(),
            Type::Dict(dict) => dict.value_type.as_ref().clone(),
            Type::String => Type::String, // Characters
            _ => Type::Any,
        }
    }
}

