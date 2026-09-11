//! Scoring a reply against a case.
//!
//! Two questions are kept apart on purpose. Schema-valid asks whether the model
//! emitted something the runtime could actually dispatch. Arguments-plausible
//! asks whether it picked the right tool and filled it in sensibly. A model can
//! pass the first and fail the second, and the difference decides whether the
//! router can trust it.

use serde_json::Value;

use crate::case::{ArgExpect, ArgKind, Step};
use crate::client::ToolCall;

/// Find a tool definition by name in the case's declared tools.
pub fn find_tool<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
    tools.iter().find(|tool| {
        tool.pointer("/function/name")
            .and_then(Value::as_str)
            .map(|declared| declared == name)
            .unwrap_or(false)
    })
}

/// Does this call conform to the schema the tool declared?
///
/// Checks that the tool exists, the arguments parse as a JSON object, and the
/// object satisfies the declared parameter schema.
pub fn schema_valid(call: &ToolCall, tools: &[Value]) -> Result<Value, String> {
    let tool = find_tool(tools, &call.name)
        .ok_or_else(|| format!("called undeclared tool `{}`", call.name))?;

    let text = call.arguments.trim();
    let arguments: Value = if text.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(text).map_err(|e| format!("arguments are not JSON: {}", e))?
    };
    if !arguments.is_object() {
        return Err("arguments are not a JSON object".to_string());
    }

    let schema = tool
        .pointer("/function/parameters")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    validate(&arguments, &schema, "arguments")?;
    Ok(arguments)
}

/// Validate a value against the subset of JSON Schema the suite uses:
/// type, required, properties and array items.
fn validate(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    if let Some(declared) = schema.get("type").and_then(Value::as_str) {
        if !type_matches(value, declared) {
            return Err(format!(
                "{} should be {}, found {}",
                path,
                declared,
                type_name(value)
            ));
        }
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if value.get(name).is_none() {
                return Err(format!("{} is missing required `{}`", path, name));
            }
        }
    }

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        if let Some(object) = value.as_object() {
            for (name, child) in object {
                if let Some(child_schema) = properties.get(name) {
                    validate(child, child_schema, &format!("{}.{}", path, name))?;
                }
            }
        }
    }

    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, child) in array.iter().enumerate() {
            validate(child, items, &format!("{}[{}]", path, index))?;
        }
    }

    Ok(())
}

fn type_matches(value: &Value, declared: &str) -> bool {
    match declared {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => is_integer(value),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn is_integer(value: &Value) -> bool {
    match value {
        Value::Number(number) => {
            number.is_i64() || number.is_u64() || number.as_f64().map(|n| n.fract() == 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::Null => "null",
    }
}

/// Did the model pick the expected tool and fill it in sensibly?
pub fn args_plausible(call: &ToolCall, arguments: &Value, step: &Step) -> Result<(), String> {
    if call.name != step.tool {
        return Err(format!(
            "called `{}`, the case expects `{}`",
            call.name, step.tool
        ));
    }
    for expect in &step.args {
        check_arg(arguments, expect)?;
    }
    Ok(())
}

fn check_arg(arguments: &Value, expect: &ArgExpect) -> Result<(), String> {
    let value = lookup(arguments, &expect.path)
        .ok_or_else(|| format!("`{}` is absent", expect.path))?;

    let kind_ok = match expect.kind {
        ArgKind::String => value.is_string(),
        ArgKind::Number => value.is_number(),
        ArgKind::Integer => is_integer(value),
        ArgKind::Boolean => value.is_boolean(),
        ArgKind::Object => value.is_object(),
        ArgKind::Array => value.is_array(),
    };
    if !kind_ok {
        return Err(format!(
            "`{}` should be {:?}, found {}",
            expect.path,
            expect.kind,
            type_name(value)
        ));
    }

    if let Some(needle) = &expect.contains {
        let haystack = value.as_str().unwrap_or_default().to_lowercase();
        if !haystack.contains(&needle.to_lowercase()) {
            return Err(format!(
                "`{}` should mention `{}`, found `{}`",
                expect.path,
                needle,
                value.as_str().unwrap_or_default()
            ));
        }
    }

    if expect.min.is_some() || expect.max.is_some() {
        let number = value
            .as_f64()
            .ok_or_else(|| format!("`{}` is not numeric", expect.path))?;
        if let Some(min) = expect.min {
            if number < min {
                return Err(format!("`{}` is {}, below {}", expect.path, number, min));
            }
        }
        if let Some(max) = expect.max {
            if number > max {
                return Err(format!("`{}` is {}, above {}", expect.path, number, max));
            }
        }
    }

    Ok(())
}

/// Walk a dotted path such as `mode.width` into the argument object.
fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{params, string_of, tool, ArgExpect, Step};
    use serde_json::json;

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "c1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn tools() -> Vec<Value> {
        vec![tool(
            "read_file",
            "Read a file",
            params(vec![("path", string_of("Absolute path"))], &["path"]),
        )]
    }

    #[test]
    fn accepts_a_well_formed_call() {
        let parsed = schema_valid(&call("read_file", r#"{"path":"/etc/hostname"}"#), &tools());
        assert!(parsed.is_ok(), "{:?}", parsed);
    }

    #[test]
    fn rejects_an_undeclared_tool() {
        assert!(schema_valid(&call("delete_everything", "{}"), &tools()).is_err());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(schema_valid(&call("read_file", "{path: /etc}"), &tools()).is_err());
    }

    #[test]
    fn rejects_a_missing_required_argument() {
        assert!(schema_valid(&call("read_file", "{}"), &tools()).is_err());
    }

    #[test]
    fn rejects_a_wrongly_typed_argument() {
        assert!(schema_valid(&call("read_file", r#"{"path":7}"#), &tools()).is_err());
    }

    #[test]
    fn checks_nested_paths_and_numbers() {
        let arguments = json!({"mode": {"width": 1920}, "name": "HDMI-1"});
        let step = Step::new(
            "configure",
            vec![
                ArgExpect::integer("mode.width", 1920),
                ArgExpect::text("name", "hdmi"),
            ],
        );
        let outcome = args_plausible(&call("configure", "{}"), &arguments, &step);
        assert!(outcome.is_ok(), "{:?}", outcome);
    }

    #[test]
    fn catches_the_wrong_number() {
        let arguments = json!({"level": 11});
        let step = Step::new("set_volume", vec![ArgExpect::integer("level", 40)]);
        assert!(args_plausible(&call("set_volume", "{}"), &arguments, &step).is_err());
    }

    #[test]
    fn catches_the_wrong_tool() {
        let step = Step::new("set_volume", vec![]);
        assert!(args_plausible(&call("set_brightness", "{}"), &json!({}), &step).is_err());
    }
}
