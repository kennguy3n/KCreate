//! Tool-call registry + GBNF grammar generator for the LLM sidecar.
//!
//! Tessera-style: instead of free-form chat completion, the host
//! declares the tools the assistant is allowed to invoke
//! (`ToolDescriptor`s — name, description, parameter schema), and
//! asks the model to emit a JSON tool call. The model is constrained
//! by a generated GBNF grammar so its output is guaranteed-valid
//! JSON in the registry's shape:
//!
//! ```json
//! {"tool": "create_artboard", "arguments": {"width": 1920, "height": 1080, "name": "Landing"}}
//! ```
//!
//! On the way back in we re-validate the JSON against the schema
//! (right tool name, all required parameters present, value types
//! match). The grammar is the *first* line of defence — the schema
//! validator is the *second*, because GBNF can constrain the literal
//! shape (object → keys → atom-types) but cannot encode richer
//! constraints like "width must be > 0" or "name is at most 64
//! chars".
//!
//! Everything in this module is **pure** (no I/O, no networking) so
//! it stays in the editing-path dependency tree even though the
//! `llm_chat` module that uses it is gated behind the
//! `llm_sidecar` feature.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::llm_chat::ChatError;

/// Type of a tool parameter. Maps to the JSON value types the GBNF
/// grammar can constrain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolParamType {
    /// JSON string.
    String,
    /// JSON number with no fractional part. We validate that the
    /// parsed value is an i64 (so the model can't sneak a float
    /// into an integer slot).
    Integer,
    /// JSON number (integer or floating-point).
    Number,
    /// JSON `true` / `false`.
    Boolean,
    /// One of a closed set of string values (enum). The set lives
    /// on the `ToolParameter::enum_values` field.
    Enum,
}

/// One parameter on a tool. Modelled after JSON-Schema's basic
/// `properties` entry but trimmed to what the GBNF generator can
/// actually constrain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolParameter {
    pub name: String,
    pub description: String,
    pub kind: ToolParamType,
    /// Whether the parameter must be present in `arguments`. If
    /// `required == false` the validator treats absence as
    /// "default — caller's choice"; we never substitute defaults
    /// silently.
    pub required: bool,
    /// For `ToolParamType::Enum`, the list of accepted string
    /// values. Must be non-empty if `kind == Enum`; ignored
    /// otherwise.
    #[serde(default)]
    pub enum_values: Vec<String>,
}

impl ToolParameter {
    /// Build a string parameter. `required` is wired through.
    #[must_use]
    pub fn string(name: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolParamType::String,
            required,
            enum_values: Vec::new(),
        }
    }

    /// Build an integer parameter.
    #[must_use]
    pub fn integer(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolParamType::Integer,
            required,
            enum_values: Vec::new(),
        }
    }

    /// Build a floating-point parameter.
    #[must_use]
    pub fn number(name: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolParamType::Number,
            required,
            enum_values: Vec::new(),
        }
    }

    /// Build a boolean parameter.
    #[must_use]
    pub fn boolean(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolParamType::Boolean,
            required,
            enum_values: Vec::new(),
        }
    }

    /// Build a closed-set string parameter (enum). Caller must
    /// supply at least one accepted value.
    pub fn enumeration(
        name: impl Into<String>,
        description: impl Into<String>,
        required: bool,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ToolRegistryError> {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        if values.is_empty() {
            return Err(ToolRegistryError::EmptyEnum {
                parameter: name.into(),
            });
        }
        Ok(Self {
            name: name.into(),
            description: description.into(),
            kind: ToolParamType::Enum,
            required,
            enum_values: values,
        })
    }
}

/// Declarative description of one tool the assistant may invoke.
///
/// Tool names follow `^[a-z][a-z0-9_]{0,62}$` so they can be
/// embedded in a GBNF grammar without escaping; parameter names
/// follow the same rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParameter>,
}

impl ToolDescriptor {
    /// Construct a descriptor, validating the name + parameter names
    /// up front.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Vec<ToolParameter>,
    ) -> Result<Self, ToolRegistryError> {
        let name = name.into();
        validate_ident(&name, ToolRegistryError::InvalidToolName)?;
        // Reject duplicate parameter names within a tool.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for p in &parameters {
            validate_ident(&p.name, ToolRegistryError::InvalidParameterName)?;
            if !seen.insert(p.name.as_str()) {
                return Err(ToolRegistryError::DuplicateParameter {
                    tool: name,
                    parameter: p.name.clone(),
                });
            }
            if p.kind == ToolParamType::Enum && p.enum_values.is_empty() {
                return Err(ToolRegistryError::EmptyEnum {
                    parameter: p.name.clone(),
                });
            }
        }
        Ok(Self {
            name,
            description: description.into(),
            parameters,
        })
    }

    /// Look up a parameter by name.
    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<&ToolParameter> {
        self.parameters.iter().find(|p| p.name == name)
    }
}

/// Registry of tools the assistant is allowed to invoke. Tool names
/// must be unique across the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRegistry {
    tools: Vec<ToolDescriptor>,
}

impl ToolCallRegistry {
    /// Build an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Build a registry from a list of descriptors. Returns
    /// `DuplicateTool` if two descriptors share a name.
    pub fn from_tools(tools: Vec<ToolDescriptor>) -> Result<Self, ToolRegistryError> {
        let mut registry = Self::new();
        for tool in tools {
            registry.register(tool)?;
        }
        Ok(registry)
    }

    /// Register a tool. Rejects duplicate names.
    pub fn register(&mut self, tool: ToolDescriptor) -> Result<(), ToolRegistryError> {
        if self.tools.iter().any(|t| t.name == tool.name) {
            return Err(ToolRegistryError::DuplicateTool { tool: tool.name });
        }
        self.tools.push(tool);
        Ok(())
    }

    /// List the registered tools.
    #[must_use]
    pub fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Return `true` when the registry has no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

/// A parsed-and-validated tool invocation produced by the assistant.
/// `arguments` is a `BTreeMap<String, serde_json::Value>` so the
/// keys serialise in a deterministic order (handy for fixtures).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub arguments: BTreeMap<String, serde_json::Value>,
}

impl ToolCall {
    /// Pull a `&str` from `arguments` by name. Returns `None` when
    /// absent or when the value isn't a JSON string.
    #[must_use]
    pub fn arg_str(&self, name: &str) -> Option<&str> {
        self.arguments.get(name).and_then(|v| v.as_str())
    }

    /// Pull an `i64` from `arguments` by name.
    #[must_use]
    pub fn arg_i64(&self, name: &str) -> Option<i64> {
        self.arguments.get(name).and_then(serde_json::Value::as_i64)
    }

    /// Pull an `f64` from `arguments` by name. Accepts integer
    /// values too (they're losslessly representable as f64 up to
    /// 2^53).
    #[must_use]
    pub fn arg_f64(&self, name: &str) -> Option<f64> {
        self.arguments.get(name).and_then(serde_json::Value::as_f64)
    }

    /// Pull a `bool` from `arguments` by name.
    #[must_use]
    pub fn arg_bool(&self, name: &str) -> Option<bool> {
        self.arguments
            .get(name)
            .and_then(serde_json::Value::as_bool)
    }
}

/// Errors that can arise when constructing a tool registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolRegistryError {
    #[error("invalid tool name {0:?}: must match ^[a-z][a-z0-9_]{{0,62}}$")]
    InvalidToolName(String),
    #[error("invalid parameter name {0:?}: must match ^[a-z][a-z0-9_]{{0,62}}$")]
    InvalidParameterName(String),
    #[error("tool {tool:?} has duplicate parameter {parameter:?}")]
    DuplicateParameter { tool: String, parameter: String },
    #[error("duplicate tool {tool:?}")]
    DuplicateTool { tool: String },
    #[error("enum parameter {parameter:?} must have at least one value")]
    EmptyEnum { parameter: String },
}

/// Errors that can arise when parsing a tool-call response.
#[derive(Debug, thiserror::Error)]
pub enum ToolCallParseError {
    #[error("response is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("response is not a JSON object")]
    NotAnObject,
    #[error("response missing required field {0:?}")]
    MissingField(&'static str),
    #[error("response field {0:?} has the wrong JSON type")]
    WrongType(&'static str),
    #[error("tool {0:?} is not registered")]
    UnknownTool(String),
    #[error("tool {tool:?} missing required parameter {parameter:?}")]
    MissingParameter { tool: String, parameter: String },
    #[error("tool {tool:?} parameter {parameter:?} has wrong type (expected {expected})")]
    WrongParameterType {
        tool: String,
        parameter: String,
        expected: &'static str,
    },
    #[error("tool {tool:?} parameter {parameter:?} value {value:?} is not in the allowed set")]
    EnumValueNotAllowed {
        tool: String,
        parameter: String,
        value: String,
    },
    #[error("tool {tool:?} got unknown parameter {parameter:?}")]
    UnknownParameter { tool: String, parameter: String },
}

impl From<ToolCallParseError> for ChatError {
    fn from(err: ToolCallParseError) -> Self {
        Self::Decode(err.to_string())
    }
}

/// Validate a `^[a-z][a-z0-9_]{0,62}$` identifier and call
/// `f(input.to_owned())` to construct the error if it fails.
fn validate_ident<F>(s: &str, f: F) -> Result<(), ToolRegistryError>
where
    F: FnOnce(String) -> ToolRegistryError,
{
    let mut chars = s.chars();
    let first = chars.next();
    let first_ok = matches!(first, Some(c) if c.is_ascii_lowercase());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !(first_ok && rest_ok && s.len() <= 63) {
        return Err(f(s.to_owned()));
    }
    Ok(())
}

/// Generate a GBNF grammar that constrains the LLM's output to a
/// valid tool call against this registry.
///
/// The grammar emits a top-level object with two keys (`tool`,
/// `arguments`). The `tool` key is constrained to the union of
/// registered tool names. The `arguments` key is constrained to a
/// JSON object whose keys are the union of all parameter names
/// across all tools, and whose values are the matching atom types
/// (string / integer / number / boolean / enum literal).
///
/// **Important**: GBNF cannot encode the per-tool argument schema
/// at the grammar level — it would require a context-sensitive
/// grammar. So the grammar is a *necessary but not sufficient*
/// constraint; the schema validator in `parse_tool_call_response`
/// is what enforces "the right arguments for the chosen tool".
#[must_use]
pub fn gbnf_for_registry(registry: &ToolCallRegistry) -> String {
    if registry.is_empty() {
        // Degenerate case: no tools registered. Force the model to
        // emit a single empty `{}` so the parse step fails with a
        // typed `MissingField` rather than a grammar-stuck infinite
        // loop on the llama-server side.
        return "root ::= \"{}\"\n".to_string();
    }

    let mut out = String::new();
    out.push_str("# Auto-generated by `gbnf_for_registry`.\n");
    out.push_str(
        "root ::= \"{\" ws \"\\\"tool\\\"\" ws \":\" ws tool ws \",\" ws \"\\\"arguments\\\"\" ws \":\" ws arguments ws \"}\"\n",
    );

    // `tool ::= "name1" | "name2" | …`
    out.push_str("tool ::= ");
    for (i, t) in registry.tools().iter().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        out.push('"');
        out.push('\\');
        out.push('"');
        out.push_str(&t.name);
        out.push('\\');
        out.push('"');
        out.push('"');
    }
    out.push('\n');

    // `arguments ::= "{" ws (kv (ws "," ws kv)*)? ws "}"`
    out.push_str("arguments ::= \"{\" ws ( kv ( ws \",\" ws kv )* )? ws \"}\"\n");

    // `kv ::= key ws ":" ws value`
    out.push_str("kv ::= key ws \":\" ws value\n");

    // Build the union of parameter names across all tools, and
    // collect every enum-value literal so each one becomes a
    // grammar alternative under `value`.
    let mut param_names: BTreeSet<String> = BTreeSet::new();
    let mut enum_values: BTreeSet<String> = BTreeSet::new();
    for t in registry.tools() {
        for p in &t.parameters {
            param_names.insert(p.name.clone());
            if p.kind == ToolParamType::Enum {
                for v in &p.enum_values {
                    enum_values.insert(v.clone());
                }
            }
        }
    }
    out.push_str("key ::= ");
    for (i, name) in param_names.iter().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        out.push('"');
        out.push('\\');
        out.push('"');
        out.push_str(name);
        out.push('\\');
        out.push('"');
        out.push('"');
    }
    out.push('\n');

    // `value ::= string | integer | number | boolean | enum_literal`
    out.push_str("value ::= string | integer | number | boolean");
    if !enum_values.is_empty() {
        out.push_str(" | enum_literal");
    }
    out.push('\n');

    // JSON atoms. The string rule is the GBNF distillation of the

    // JSON spec's `string` production — \" and \\ are mandatory
    // escapes; other control chars must be escaped too but we
    // accept the model emitting them raw because llama-server
    // already escapes its output. We don't need full \uXXXX
    // support because the tool-call use case never carries arbitrary
    // unicode payloads.
    out.push_str("string ::= \"\\\"\" string_body \"\\\"\"\n");
    out.push_str("string_body ::= ( [^\"\\\\] | \"\\\\\" [\"\\\\bfnrt/] )*\n");
    out.push_str("integer ::= \"-\"? [0-9]+\n");
    out.push_str("number ::= \"-\"? [0-9]+ ( \".\" [0-9]+ )? ( [eE] [+-]? [0-9]+ )?\n");
    out.push_str("boolean ::= \"true\" | \"false\"\n");
    if !enum_values.is_empty() {
        out.push_str("enum_literal ::= ");
        for (i, v) in enum_values.iter().enumerate() {
            if i > 0 {
                out.push_str(" | ");
            }
            out.push('"');
            out.push('\\');
            out.push('"');
            out.push_str(v);
            out.push('\\');
            out.push('"');
            out.push('"');
        }
        out.push('\n');
    }
    out.push_str("ws ::= ( \" \" | \"\\t\" | \"\\n\" )*\n");
    out
}

/// Parse a tool-call JSON response and validate it against
/// `registry`. Returns a typed `ToolCall` ready for the host
/// dispatcher.
///
/// The function is strict: it rejects unknown tools, missing
/// required parameters, unknown parameters, and parameters whose
/// JSON type doesn't match the descriptor. This is the *second
/// line of defence* after the GBNF grammar — anything that the
/// grammar misses (e.g. the LLM emitted a valid-shape but
/// semantically wrong call) is caught here.
pub fn parse_tool_call_response(
    response: &str,
    registry: &ToolCallRegistry,
) -> Result<ToolCall, ToolCallParseError> {
    let value: serde_json::Value = serde_json::from_str(response)
        .map_err(|e| ToolCallParseError::InvalidJson(e.to_string()))?;
    let obj = value.as_object().ok_or(ToolCallParseError::NotAnObject)?;
    let tool_name = obj
        .get("tool")
        .ok_or(ToolCallParseError::MissingField("tool"))?
        .as_str()
        .ok_or(ToolCallParseError::WrongType("tool"))?
        .to_string();
    let descriptor = registry
        .get(&tool_name)
        .ok_or_else(|| ToolCallParseError::UnknownTool(tool_name.clone()))?;
    let arguments_value = obj
        .get("arguments")
        .ok_or(ToolCallParseError::MissingField("arguments"))?;
    let arguments_obj = arguments_value
        .as_object()
        .ok_or(ToolCallParseError::WrongType("arguments"))?;

    // Check every required parameter is present.
    for p in &descriptor.parameters {
        if p.required && !arguments_obj.contains_key(&p.name) {
            return Err(ToolCallParseError::MissingParameter {
                tool: tool_name.clone(),
                parameter: p.name.clone(),
            });
        }
    }

    // Check every supplied argument is (a) a known parameter and
    // (b) the right type.
    let mut arguments = BTreeMap::new();
    for (k, v) in arguments_obj {
        let p = descriptor
            .parameter(k)
            .ok_or_else(|| ToolCallParseError::UnknownParameter {
                tool: tool_name.clone(),
                parameter: k.clone(),
            })?;
        let type_ok = match p.kind {
            ToolParamType::String | ToolParamType::Enum => v.is_string(),
            ToolParamType::Integer => v.is_i64(),
            // `is_number()` accepts integers too, which is what we
            // want for the "number" type — i.e. integer inputs are
            // valid where a number is expected.
            ToolParamType::Number => v.is_number(),
            ToolParamType::Boolean => v.is_boolean(),
        };
        if !type_ok {
            return Err(ToolCallParseError::WrongParameterType {
                tool: tool_name.clone(),
                parameter: k.clone(),
                expected: match p.kind {
                    ToolParamType::String => "string",
                    ToolParamType::Integer => "integer",
                    ToolParamType::Number => "number",
                    ToolParamType::Boolean => "boolean",
                    ToolParamType::Enum => "string (enum)",
                },
            });
        }
        if p.kind == ToolParamType::Enum {
            let actual = v.as_str().expect("type_ok established");
            if !p.enum_values.iter().any(|x| x == actual) {
                return Err(ToolCallParseError::EnumValueNotAllowed {
                    tool: tool_name.clone(),
                    parameter: k.clone(),
                    value: actual.to_string(),
                });
            }
        }
        arguments.insert(k.clone(), v.clone());
    }
    Ok(ToolCall {
        tool: tool_name,
        arguments,
    })
}

/// Default registry of design tools the assistant may invoke.
///
/// The dispatcher (bridge layer, Block 3) will wire each of these
/// to a concrete `kcreate_bridge::document` entry point. The names
/// follow the project's existing IPC verb style (`create_artboard`,
/// not `createArtboard`) so the bridge dispatcher can route by
/// snake-case match.
#[must_use]
pub fn default_design_registry() -> ToolCallRegistry {
    let tools = vec![
        ToolDescriptor::new(
            "list_artboards",
            "List the artboards in the current document.",
            vec![],
        )
        .expect("static descriptor"),
        ToolDescriptor::new(
            "create_artboard",
            "Create a new artboard with the given dimensions and name.",
            vec![
                ToolParameter::string("name", "Display name for the artboard.", true),
                ToolParameter::integer("width", "Width in document units (px).", true),
                ToolParameter::integer("height", "Height in document units (px).", true),
            ],
        )
        .expect("static descriptor"),
        ToolDescriptor::new(
            "set_fill",
            "Set the fill of the named node to a hex colour (e.g. \"#ff0080\").",
            vec![
                ToolParameter::string(
                    "node_id",
                    "Document graph node id (uuid-v4 hyphenated).",
                    true,
                ),
                ToolParameter::string(
                    "color_hex",
                    "Hex colour string, with or without leading #.",
                    true,
                ),
            ],
        )
        .expect("static descriptor"),
        ToolDescriptor::new(
            "create_text",
            "Create a text node on the given artboard with the supplied content.",
            vec![
                ToolParameter::string("artboard_id", "Parent artboard's node id.", true),
                ToolParameter::string("text", "Text content (single line).", true),
                ToolParameter::number("x", "X position relative to the artboard, in px.", true),
                ToolParameter::number("y", "Y position relative to the artboard, in px.", true),
                ToolParameter::number(
                    "font_size",
                    "Font size in px. Defaults to 16 if omitted.",
                    false,
                ),
            ],
        )
        .expect("static descriptor"),
        ToolDescriptor::new(
            "set_alignment",
            "Set the horizontal alignment of the named text node.",
            vec![
                ToolParameter::string("node_id", "Document graph node id.", true),
                ToolParameter::enumeration(
                    "alignment",
                    "Horizontal alignment.",
                    true,
                    ["left", "center", "right", "justify"],
                )
                .expect("static descriptor"),
            ],
        )
        .expect("static descriptor"),
    ];
    ToolCallRegistry::from_tools(tools).expect("static registry")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ident_validator_accepts_valid_names() {
        assert!(validate_ident("create_artboard", ToolRegistryError::InvalidToolName).is_ok());
        assert!(validate_ident("a", ToolRegistryError::InvalidToolName).is_ok());
        assert!(validate_ident("x1_y2_z3", ToolRegistryError::InvalidToolName).is_ok());
    }

    #[test]
    fn ident_validator_rejects_bad_names() {
        for bad in [
            "",
            "Capital",
            "1leading_digit",
            "with-dash",
            "with space",
            "with.dot",
            // 64 chars — limit is 63.
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(
                validate_ident(bad, ToolRegistryError::InvalidToolName).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn descriptor_rejects_duplicate_parameters() {
        let err = ToolDescriptor::new(
            "t",
            "",
            vec![
                ToolParameter::string("a", "", true),
                ToolParameter::string("a", "", false),
            ],
        )
        .expect_err("dup");
        assert!(matches!(err, ToolRegistryError::DuplicateParameter { .. }));
    }

    #[test]
    fn registry_rejects_duplicate_tools() {
        let mut r = ToolCallRegistry::new();
        r.register(ToolDescriptor::new("a", "", vec![]).unwrap())
            .unwrap();
        let err = r
            .register(ToolDescriptor::new("a", "", vec![]).unwrap())
            .expect_err("dup");
        assert!(matches!(err, ToolRegistryError::DuplicateTool { .. }));
    }

    #[test]
    fn gbnf_for_empty_registry_is_constant_object() {
        let r = ToolCallRegistry::new();
        assert_eq!(gbnf_for_registry(&r), "root ::= \"{}\"\n");
    }

    #[test]
    fn gbnf_for_default_registry_mentions_every_tool() {
        let r = default_design_registry();
        let g = gbnf_for_registry(&r);
        for t in r.tools() {
            assert!(
                g.contains(&format!("\\\"{}\\\"", t.name)),
                "grammar should mention tool {:?}; got:\n{g}",
                t.name
            );
        }
        // Sanity: the rule heads are all present.
        for head in ["root", "tool", "arguments", "kv", "key", "value", "ws"] {
            assert!(
                g.contains(&format!("{head} ::=")),
                "missing rule head {head}: \n{g}"
            );
        }
    }

    #[test]
    fn parse_round_trips_a_simple_call() {
        let r = default_design_registry();
        let raw = r#"{"tool":"create_artboard","arguments":{"name":"Landing","width":1920,"height":1080}}"#;
        let call = parse_tool_call_response(raw, &r).expect("parse");
        assert_eq!(call.tool, "create_artboard");
        assert_eq!(call.arg_str("name"), Some("Landing"));
        assert_eq!(call.arg_i64("width"), Some(1920));
        assert_eq!(call.arg_i64("height"), Some(1080));
    }

    #[test]
    fn parse_rejects_unknown_tool() {
        let r = default_design_registry();
        let raw = r#"{"tool":"explode","arguments":{}}"#;
        let err = parse_tool_call_response(raw, &r).expect_err("unknown");
        assert!(matches!(err, ToolCallParseError::UnknownTool(ref t) if t == "explode"));
    }

    #[test]
    fn parse_rejects_missing_required_param() {
        let r = default_design_registry();
        let raw = r#"{"tool":"create_artboard","arguments":{"name":"A","width":100}}"#;
        let err = parse_tool_call_response(raw, &r).expect_err("missing");
        match err {
            ToolCallParseError::MissingParameter { parameter, .. } => {
                assert_eq!(parameter, "height");
            }
            other => panic!("wrong variant {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_unknown_param() {
        let r = default_design_registry();
        let raw = r#"{"tool":"list_artboards","arguments":{"unexpected":1}}"#;
        let err = parse_tool_call_response(raw, &r).expect_err("unknown param");
        assert!(matches!(err, ToolCallParseError::UnknownParameter { .. }));
    }

    #[test]
    fn parse_rejects_wrong_type() {
        let r = default_design_registry();
        let raw =
            r#"{"tool":"create_artboard","arguments":{"name":"A","width":"big","height":1080}}"#;
        let err = parse_tool_call_response(raw, &r).expect_err("wrong type");
        match err {
            ToolCallParseError::WrongParameterType {
                parameter,
                expected,
                ..
            } => {
                assert_eq!(parameter, "width");
                assert_eq!(expected, "integer");
            }
            other => panic!("wrong variant {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_enum_value_outside_set() {
        let r = default_design_registry();
        let raw = r#"{"tool":"set_alignment","arguments":{"node_id":"n","alignment":"diagonal"}}"#;
        let err = parse_tool_call_response(raw, &r).expect_err("enum");
        match err {
            ToolCallParseError::EnumValueNotAllowed {
                parameter, value, ..
            } => {
                assert_eq!(parameter, "alignment");
                assert_eq!(value, "diagonal");
            }
            other => panic!("wrong variant {other:?}"),
        }
    }

    #[test]
    fn parse_accepts_enum_value_in_set() {
        let r = default_design_registry();
        let raw = r#"{"tool":"set_alignment","arguments":{"node_id":"n","alignment":"center"}}"#;
        let call = parse_tool_call_response(raw, &r).expect("parse");
        assert_eq!(call.arg_str("alignment"), Some("center"));
    }

    #[test]
    fn parse_optional_param_may_be_omitted() {
        let r = default_design_registry();
        // `font_size` is optional on `create_text`.
        let raw =
            r#"{"tool":"create_text","arguments":{"artboard_id":"a","text":"hi","x":0,"y":0}}"#;
        let call = parse_tool_call_response(raw, &r).expect("parse");
        assert!(!call.arguments.contains_key("font_size"));
    }

    #[test]
    fn parse_rejects_invalid_json() {
        let r = default_design_registry();
        let err = parse_tool_call_response("not json", &r).expect_err("json");
        assert!(matches!(err, ToolCallParseError::InvalidJson(_)));
    }

    #[test]
    fn arg_accessors_return_none_on_type_mismatch() {
        let mut arguments = BTreeMap::new();
        arguments.insert("text".to_string(), json!("hi"));
        let call = ToolCall {
            tool: "create_text".into(),
            arguments,
        };
        // text is a string, but caller asked for i64
        assert_eq!(call.arg_i64("text"), None);
    }
}
