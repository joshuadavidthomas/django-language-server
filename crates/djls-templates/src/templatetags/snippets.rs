use super::specs::Arg;
use super::specs::ArgType;
use super::specs::SimpleArgType;
use super::specs::TagSpec;

/// Generate an LSP snippet pattern from an array of arguments
pub fn generate_snippet_from_args(args: &[Arg]) -> String {
    let mut parts = Vec::new();
    let mut placeholder_index = 1;
    
    for arg in args {
        // Skip optional args if we haven't seen any required args after them
        // This prevents generating snippets like: "{% for %}" when everything is optional
        if !arg.required && parts.is_empty() {
            continue;
        }
        
        let snippet_part = match &arg.arg_type {
            ArgType::Simple(simple_type) => match simple_type {
                SimpleArgType::Literal => {
                    if arg.required {
                        // Required literals are just plain text (e.g., "in", "as", "by")
                        arg.name.clone()
                    } else {
                        // Optional literals become placeholders
                        let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                        placeholder_index += 1;
                        result
                    }
                }
                SimpleArgType::Variable | SimpleArgType::Expression => {
                    // Variables and expressions become placeholders
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::String => {
                    // Strings get quotes around them
                    let result = format!("\"${{{}:{}}}\"", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::Assignment => {
                    // Assignments use the name as-is (e.g., "var=value")
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
                SimpleArgType::VarArgs => {
                    // Variable arguments, just use the name
                    let result = format!("${{{}:{}}}", placeholder_index, arg.name);
                    placeholder_index += 1;
                    result
                }
            },
            ArgType::Choice { choice } => {
                // Choice placeholders with options
                let result = format!("${{{}|{}|}}", placeholder_index, choice.join(","));
                placeholder_index += 1;
                result
            }
        };
        
        parts.push(snippet_part);
    }
    
    parts.join(" ")
}

/// Generate a complete LSP snippet for a tag including the tag name
pub fn generate_snippet_for_tag(tag_name: &str, spec: &TagSpec) -> String {
    let args_snippet = generate_snippet_from_args(&spec.args);
    
    if args_snippet.is_empty() {
        // Tag with no arguments
        tag_name.to_string()
    } else {
        // Tag with arguments
        format!("{} {}", tag_name, args_snippet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templatetags::specs::ArgType;
    use crate::templatetags::specs::SimpleArgType;

    #[test]
    fn test_snippet_for_for_tag() {
        let args = vec![
            Arg {
                name: "item".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
            Arg {
                name: "in".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
            Arg {
                name: "items".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
            Arg {
                name: "reversed".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
        ];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:item} in ${2:items} ${3:reversed}");
    }
    
    #[test]
    fn test_snippet_for_if_tag() {
        let args = vec![
            Arg {
                name: "condition".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::Expression),
            },
        ];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1:condition}");
    }
    
    #[test]
    fn test_snippet_for_autoescape_tag() {
        let args = vec![
            Arg {
                name: "mode".to_string(),
                required: true,
                arg_type: ArgType::Choice { choice: vec!["on".to_string(), "off".to_string()] },
            },
        ];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "${1|on,off|}");
    }
    
    #[test]
    fn test_snippet_for_extends_tag() {
        let args = vec![
            Arg {
                name: "template".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::String),
            },
        ];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "\"${1:template}\"");
    }
    
    #[test]
    fn test_snippet_for_csrf_token_tag() {
        let args = vec![];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "");
    }
    
    #[test]
    fn test_snippet_for_url_tag() {
        let args = vec![
            Arg {
                name: "view_name".to_string(),
                required: true,
                arg_type: ArgType::Simple(SimpleArgType::String),
            },
            Arg {
                name: "args".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::VarArgs),
            },
            Arg {
                name: "as".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Literal),
            },
            Arg {
                name: "varname".to_string(),
                required: false,
                arg_type: ArgType::Simple(SimpleArgType::Variable),
            },
        ];
        
        let snippet = generate_snippet_from_args(&args);
        assert_eq!(snippet, "\"${1:view_name}\" ${2:args} ${3:as} ${4:varname}");
    }
}