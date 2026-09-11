//! Test case model.
//!
//! A case is a prompt, the tools the model may call, and an assertion about the
//! shape of what should come back. Cases load from JSON so a suite can be
//! extended without rebuilding.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    SingleCall,
    MultiTurn,
    NestedArgs,
    NumericArg,
    Refusal,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::SingleCall => "single-call",
            Category::MultiTurn => "multi-turn",
            Category::NestedArgs => "nested-args",
            Category::NumericArg => "numeric-arg",
            Category::Refusal => "refusal",
        }
    }

    pub fn all() -> [Category; 5] {
        [
            Category::SingleCall,
            Category::MultiTurn,
            Category::NestedArgs,
            Category::NumericArg,
            Category::Refusal,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgKind {
    String,
    Number,
    Integer,
    Boolean,
    Object,
    Array,
}

/// An assertion about one argument the model supplied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgExpect {
    /// Dotted path into the argument object, e.g. `mode.width`.
    pub path: String,
    pub kind: ArgKind,
    /// Case-insensitive substring the string argument must contain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

impl ArgExpect {
    pub fn text(path: &str, contains: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::String,
            contains: Some(contains.to_string()),
            min: None,
            max: None,
        }
    }

    pub fn any_text(path: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::String,
            contains: None,
            min: None,
            max: None,
        }
    }

    pub fn number(path: &str, value: f64) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::Number,
            contains: None,
            min: Some(value),
            max: Some(value),
        }
    }

    pub fn integer(path: &str, value: i64) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::Integer,
            contains: None,
            min: Some(value as f64),
            max: Some(value as f64),
        }
    }

    pub fn object(path: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::Object,
            contains: None,
            min: None,
            max: None,
        }
    }

    pub fn array(path: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: ArgKind::Array,
            contains: None,
            min: None,
            max: None,
        }
    }
}

/// One expected tool call, plus the result fed back for the next turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    #[serde(default)]
    pub args: Vec<ArgExpect>,
    /// Canned tool output returned to the model before the following step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

impl Step {
    pub fn new(tool: &str, args: Vec<ArgExpect>) -> Self {
        Self {
            tool: tool.to_string(),
            args,
            result: None,
        }
    }

    pub fn returning(tool: &str, args: Vec<ArgExpect>, result: &str) -> Self {
        Self {
            tool: tool.to_string(),
            args,
            result: Some(result.to_string()),
        }
    }
}

/// What a correct answer looks like.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Expect {
    /// One or more tool calls, in order.
    Sequence { steps: Vec<Step> },
    /// No tool applies. Calling one is the failure.
    NoCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub category: Category,
    pub prompt: String,
    /// Tool definitions exactly as they go on the wire.
    pub tools: Vec<Value>,
    pub expect: Expect,
}

impl Case {
    pub fn steps(&self) -> &[Step] {
        match &self.expect {
            Expect::Sequence { steps } => steps,
            Expect::NoCall => &[],
        }
    }

}

/// Load a suite from a JSON file holding an array of cases.
pub fn load(path: &Path) -> Result<Vec<Case>, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Schema construction helpers, used by the built-in suite.
// ---------------------------------------------------------------------------

/// A function tool in OpenAI wire format.
pub fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

pub fn params(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, schema) in properties {
        map.insert(key.to_string(), schema);
    }
    json!({
        "type": "object",
        "properties": Value::Object(map),
        "required": required.iter().map(|r| Value::String(r.to_string())).collect::<Vec<_>>()
    })
}

pub fn string_of(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

pub fn number_of(description: &str) -> Value {
    json!({"type": "number", "description": description})
}

pub fn integer_of(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

pub fn boolean_of(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

pub fn array_of(items: Value, description: &str) -> Value {
    json!({"type": "array", "items": items, "description": description})
}

pub fn object_of(properties: Vec<(&str, Value)>, required: &[&str], description: &str) -> Value {
    let mut schema = params(properties, required);
    schema["description"] = Value::String(description.to_string());
    schema
}
