use djls_templates::parse_template;
use djls_templates::db::Db;

fn main() {
    let source = "{% block %}content{% endblock %}";
    let (ast, parse_errors) = parse_template(source).unwrap();
    println!("Parse errors: {:?}", parse_errors);
    
    // Get tag specs
    let toml_str = include_str!("crates/djls-templates/tagspecs/django.toml");
    let tag_specs = std::sync::Arc::new(
        djls_templates::tagspecs::TagSpecs::from_toml(toml_str).unwrap()
    );
    
    let (pairs, errors) = djls_templates::validation::TagMatcher::match_tags(
        ast.nodelist(), 
        tag_specs
    );
    
    println!("Validation errors ({} total):", errors.len());
    for (i, error) in errors.iter().enumerate() {
        println!("  {}. {:?}", i + 1, error);
    }
}
