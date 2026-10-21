use std::collections::HashMap;

use crate::error::AutomationError;

/// Minimal `{{variable_name}}` substitution — intentionally not a general
/// templating language. Unknown variables are a hard error rather than
/// being left verbatim or silently emptied, so a typo in an automation
/// definition surfaces immediately in the run log instead of producing a
/// confusing downstream result.
pub fn render(template: &str, vars: &HashMap<String, String>) -> Result<String, AutomationError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    loop {
        match rest.find("{{") {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                let after_open = &rest[start + 2..];
                let end = after_open.find("}}").ok_or_else(|| {
                    AutomationError::UnknownVariable(format!(
                        "unterminated template placeholder near: {}",
                        &after_open[..after_open.len().min(32)]
                    ))
                })?;
                let name = after_open[..end].trim();
                let value = vars
                    .get(name)
                    .ok_or_else(|| AutomationError::UnknownVariable(name.to_string()))?;
                out.push_str(value);
                rest = &after_open[end + 2..];
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_known_variables() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "world".to_string());
        assert_eq!(render("hello {{name}}!", &vars).unwrap(), "hello world!");
    }

    #[test]
    fn errors_on_unknown_variable() {
        let vars = HashMap::new();
        assert!(render("hello {{missing}}", &vars).is_err());
    }

    #[test]
    fn passes_through_plain_text() {
        let vars = HashMap::new();
        assert_eq!(
            render("no placeholders here", &vars).unwrap(),
            "no placeholders here"
        );
    }

    #[test]
    fn handles_multiple_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "1".to_string());
        vars.insert("b".to_string(), "2".to_string());
        assert_eq!(render("{{a}}-{{b}}", &vars).unwrap(), "1-2");
    }
}
