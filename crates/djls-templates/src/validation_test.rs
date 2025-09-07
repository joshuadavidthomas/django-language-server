#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_template;
    use crate::validation::TagMatcher;
    use crate::tagspecs::TagSpecs;
    use std::sync::Arc;
    
    fn load_test_tagspecs() -> Arc<TagSpecs> {
        let toml_str = include_str!("../tagspecs/django.toml");
        Arc::new(TagSpecs::from_toml(toml_str).unwrap())
    }

    #[test]
    fn test_block_missing_arguments() {
        let source = "{% block %}content{% endblock %}";
        let (ast, _) = parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();
        
        let (_, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);
        
        // Should have error for missing block name
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0], 
            crate::ast::AstError::MissingRequiredArguments { tag, min, .. } 
            if tag == "block" && *min == 1
        ));
    }

    #[test]
    fn test_block_too_many_arguments() {
        let source = "{% block content extra %}content{% endblock %}";
        let (ast, _) = parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();
        
        let (_, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);
        
        // Should have error for too many arguments
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0], 
            crate::ast::AstError::TooManyArguments { tag, max, .. } 
            if tag == "block" && *max == 1
        ));
    }

    #[test]
    fn test_extends_missing_arguments() {
        let source = "{% extends %}";
        let (ast, _) = parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();
        
        let (_, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);
        
        // Should have error for missing template name
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0], 
            crate::ast::AstError::MissingRequiredArguments { tag, min, .. } 
            if tag == "extends" && *min == 1
        ));
    }

    #[test]
    fn test_load_missing_arguments() {
        let source = "{% load %}";
        let (ast, _) = parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();
        
        let (_, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);
        
        // Should have error for missing library name
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0], 
            crate::ast::AstError::MissingRequiredArguments { tag, min, .. } 
            if tag == "load" && *min == 1
        ));
    }

    #[test]
    fn test_csrf_token_with_arguments() {
        let source = "{% csrf_token some_arg %}";
        let (ast, _) = parse_template(source).unwrap();
        let tag_specs = load_test_tagspecs();
        
        let (_, errors) = TagMatcher::match_tags(ast.nodelist(), tag_specs);
        
        // Should have error for too many arguments (csrf_token takes none)
        assert_eq!(errors.len(), 1);
        assert!(matches!(
            &errors[0], 
            crate::ast::AstError::TooManyArguments { tag, max, .. } 
            if tag == "csrf_token" && *max == 0
        ));
    }
}