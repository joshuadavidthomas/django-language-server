use djls_extraction::extract_rules;

#[test]
fn test_extract_defaulttags_subset() {
    let source = include_str!("fixtures/defaulttags_subset.py");
    let result = extract_rules(source).unwrap();

    insta::assert_yaml_snapshot!(result);
}

#[test]
fn test_autoescape_with_args_variable() {
    let source = include_str!("fixtures/defaulttags_subset.py");
    let result = extract_rules(source).unwrap();

    let autoescape = result.tags.iter().find(|t| t.name == "autoescape").unwrap();

    // Should extract rules despite using 'args' (not 'bits')
    assert!(!autoescape.rules.is_empty());
    assert!(autoescape.rules.iter().any(|r| {
        matches!(
            r.condition,
            djls_extraction::RuleCondition::ExactArgCount { count: 2, .. }
        )
    }));
}

#[test]
fn test_for_tag_with_parts_variable() {
    let source = include_str!("fixtures/defaulttags_subset.py");
    let result = extract_rules(source).unwrap();

    let for_tag = result.tags.iter().find(|t| t.name == "for").unwrap();

    // Should extract rules using 'parts' (not 'bits')
    assert!(for_tag.rules.iter().any(|r| {
        matches!(
            r.condition,
            djls_extraction::RuleCondition::MaxArgCount { .. }
        )
    }));
    assert!(for_tag.rules.iter().any(|r| {
        matches!(
            r.condition,
            djls_extraction::RuleCondition::LiteralAt {
                index: 2,
                ref value,
                negated: true
            } if value == "in"
        )
    }));
}

#[test]
fn test_filter_extraction() {
    let source = include_str!("fixtures/defaulttags_subset.py");
    let result = extract_rules(source).unwrap();

    assert_eq!(result.filters.len(), 3);

    let title = result.filters.iter().find(|f| f.name == "title").unwrap();
    assert_eq!(title.arity, djls_extraction::FilterArity::None);

    let default = result.filters.iter().find(|f| f.name == "default").unwrap();
    assert_eq!(default.arity, djls_extraction::FilterArity::Optional);

    let truncatewords = result
        .filters
        .iter()
        .find(|f| f.name == "truncatewords")
        .unwrap();
    assert_eq!(truncatewords.arity, djls_extraction::FilterArity::Required);
}

#[test]
fn test_block_spec_extraction() {
    let source = include_str!("fixtures/defaulttags_subset.py");
    let result = extract_rules(source).unwrap();

    // autoescape: block tag with endautoescape
    let autoescape = result.tags.iter().find(|t| t.name == "autoescape").unwrap();
    let block = autoescape.block_spec.as_ref().unwrap();
    assert_eq!(block.end_tag.as_deref(), Some("endautoescape"));
    assert!(block.intermediate_tags.is_empty());

    // if: block tag with endif, intermediates elif/else
    let if_tag = result.tags.iter().find(|t| t.name == "if").unwrap();
    let block = if_tag.block_spec.as_ref().unwrap();
    assert_eq!(block.end_tag.as_deref(), Some("endif"));
    let intermediate_names: Vec<&str> = block
        .intermediate_tags
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(intermediate_names.contains(&"elif"));
    assert!(intermediate_names.contains(&"else"));

    // for: block tag with endfor, intermediate empty
    let for_tag = result.tags.iter().find(|t| t.name == "for").unwrap();
    let block = for_tag.block_spec.as_ref().unwrap();
    assert_eq!(block.end_tag.as_deref(), Some("endfor"));
    assert_eq!(block.intermediate_tags.len(), 1);
    assert_eq!(block.intermediate_tags[0].name, "empty");

    // now: simple_tag, no block spec
    let now = result.tags.iter().find(|t| t.name == "now").unwrap();
    assert!(now.block_spec.is_none());
}
