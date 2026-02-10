# Structured Output

Meerkat supports structured output extraction -- forcing the agent to produce validated JSON conforming to a user-provided schema. The extraction happens as a separate turn after the agentic work completes, with automatic retry on validation failure.

## How It Works

When `output_schema` is configured on an agent, the execution flow is:

1. **Agentic loop runs normally** -- the agent processes the prompt, calls tools, iterates until no more tool calls.
2. **Extraction turn fires** -- `perform_extraction_turn()` (in `meerkat-core/src/agent/extraction.rs`) sends an additional LLM call with:
   - No tools (extraction is pure text generation)
   - Temperature `0.0` for deterministic output
   - A prompt asking for valid JSON matching the schema
3. **Validation** -- the response is parsed as JSON and validated against the schema using the `jsonschema` crate's `Validator`.
4. **On success** -- `RunResult.structured_output` contains the parsed `serde_json::Value`.
5. **On failure** -- retries up to `structured_output_retries` times with error feedback, then returns `AgentError::StructuredOutputValidationFailed`.

## Schema Types

### `OutputSchema`

The primary schema type (from `meerkat-core/src/types.rs`):

```rust
pub struct OutputSchema {
    pub schema: MeerkatSchema,
    pub name: Option<String>,
    pub strict: bool,
    pub compat: SchemaCompat,
    pub format: SchemaFormat,
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `schema` | `MeerkatSchema` | (required) | The JSON schema definition |
| `name` | `Option<String>` | `None` | Optional schema name (used by some providers) |
| `strict` | `bool` | `false` | Whether to enforce strict schema validation |
| `compat` | `SchemaCompat` | `Lossy` | Compatibility mode for provider lowering |
| `format` | `SchemaFormat` | `MeerkatV1` | Schema format version |

#### Construction methods

```rust
// From a raw JSON value
let schema = OutputSchema::new(json!({
    "type": "object",
    "properties": {
        "summary": { "type": "string" },
        "score": { "type": "number" }
    },
    "required": ["summary", "score"]
}))?;

// From a JSON string (raw schema or wrapper format)
let schema = OutputSchema::from_json_str(r#"{"type": "object", ...}"#)?;

// From a JSON value (raw schema or wrapper format)
let schema = OutputSchema::from_json_value(value)?;

// From a Rust type via schemars
let schema = OutputSchema::from_type::<MyStruct>()?;
```

#### Builder methods

```rust
let schema = OutputSchema::new(json!({...}))?
    .with_name("analysis-result")
    .strict()
    .with_compat(SchemaCompat::Strict)
    .with_format(SchemaFormat::MeerkatV1);
```

#### Wrapper format

`OutputSchema` supports a wrapper format for explicit configuration. If the JSON object contains a `schema` key and either a `format: "meerkat_v1"` marker or only wrapper keys (`schema`, `name`, `strict`, `compat`, `format`), it is parsed as a wrapper:

```json
{
  "schema": {
    "type": "object",
    "properties": {
      "result": { "type": "string" }
    }
  },
  "name": "my-schema",
  "strict": true,
  "compat": "strict",
  "format": "meerkat_v1"
}
```

Otherwise, the entire JSON value is treated as a raw schema.

### `MeerkatSchema`

Newtype around `serde_json::Value` with normalization (from `meerkat-core/src/schema.rs`):

```rust
pub struct MeerkatSchema(Value);
```

- Constructed via `MeerkatSchema::new(Value)` which applies `normalize_schema()`.
- **Normalization**: ensures all object-typed nodes have `properties` and `required` keys (inserting empty defaults if missing). This prevents provider-specific compilation issues.
- Returns `SchemaError::InvalidRoot` if the root is not a JSON object.

### `SchemaFormat`

Schema format versions:

| Variant | Description |
|---------|-------------|
| `MeerkatV1` | Current format version (default) |

### `SchemaCompat`

Compatibility mode for provider-specific schema lowering:

| Variant | Description |
|---------|-------------|
| `Lossy` | Best-effort lowering; unsupported features are dropped with warnings (default) |
| `Strict` | Reject schemas with unsupported features for the target provider |

### `SchemaWarning`

Warnings emitted during schema compilation:

```rust
pub struct SchemaWarning {
    pub provider: Provider,
    pub path: String,
    pub message: String,
}
```

### `CompiledSchema`

Provider-compiled schema output:

```rust
pub struct CompiledSchema {
    pub schema: Value,
    pub warnings: Vec<SchemaWarning>,
}
```

### `SchemaError`

Schema errors:

| Variant | Description |
|---------|-------------|
| `InvalidRoot` | Schema must be a JSON object at the root |
| `UnsupportedFeatures { provider, warnings }` | Schema uses features not supported by the target provider (only in `Strict` compat mode) |

## Extraction Turn Details

The extraction turn logic is in `perform_extraction_turn()` (in `meerkat-core/src/agent/extraction.rs`):

### Attempt flow

1. **Max attempts** = `structured_output_retries + 1` (default: 2 + 1 = 3 attempts).
2. **First attempt prompt**: `"Based on our conversation, provide the final output as valid JSON matching the required schema. Output ONLY the JSON, no additional text or markdown formatting."`
3. **Retry prompt** (on validation failure): `"The previous output was invalid: {error}. Please provide valid JSON matching the schema. Output ONLY the JSON, no additional text."`
4. LLM is called with **no tools** and **temperature 0.0**.
5. Response text is trimmed, then markdown code fences are stripped (handles `` ```json `` and `` ``` `` wrappers).
6. Parsed as JSON via `serde_json::from_str`.
7. Validated against the compiled schema via `jsonschema::Validator`.

### On success

Returns `RunResult` with:
- `text`: the last assistant text from the extraction turn
- `structured_output`: `Some(parsed_value)` -- the validated JSON
- `schema_warnings`: any warnings from schema compilation
- `turns`: includes extraction attempts in the count

### On failure (exhausted retries)

Returns `AgentError::StructuredOutputValidationFailed`:

```rust
AgentError::StructuredOutputValidationFailed {
    attempts: u32,        // Total attempts made
    reason: String,       // Last validation error
    last_output: String,  // Last raw LLM output
}
```

## Provider Schema Compilation

The `AgentLlmClient` trait includes a `compile_schema()` method:

```rust
fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError>;
```

The default implementation passes through the normalized schema without provider-specific lowering. Provider adapters override this to apply transformations:

- **Anthropic**: may add `additionalProperties: false` to object nodes
- **Gemini**: may strip unsupported JSON Schema keywords
- **OpenAI**: may apply strict-mode transformations

Schema warnings are collected during compilation and included in `RunResult.schema_warnings`.

## Configuration

### `AgentConfig` fields

From `meerkat-core/src/config.rs`:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `output_schema` | `Option<OutputSchema>` | `None` | JSON schema for extraction |
| `structured_output_retries` | `u32` | `2` | Max validation retries |

## CLI Usage

```bash
# Inline JSON schema
rkat run "Analyze this code" --output-schema '{"type":"object","properties":{"bugs":{"type":"array","items":{"type":"string"}}},"required":["bugs"]}'

# Schema from file
rkat run "Analyze this code" --output-schema schema.json

# With compatibility mode
rkat run "Analyze this code" --output-schema schema.json --output-schema-compat strict

# With custom retries
rkat run "Analyze this code" --output-schema schema.json --structured-output-retries 5
```

The `--output-schema` flag accepts either a file path or inline JSON. The CLI detects files by checking if the value is an existing path.

## Wire Parameters

Structured output is passed per-request via `StructuredOutputParams` (from `meerkat-contracts/src/wire/params.rs`):

```rust
pub struct StructuredOutputParams {
    pub output_schema: Option<OutputSchema>,
    pub structured_output_retries: Option<u32>,
}
```

### JSON-RPC (`session/create` and `turn/start`)

```json
{
  "jsonrpc": "2.0",
  "method": "session/create",
  "params": {
    "prompt": "Analyze this code",
    "output_schema": {
      "type": "object",
      "properties": {
        "bugs": { "type": "array", "items": { "type": "string" } }
      },
      "required": ["bugs"]
    },
    "structured_output_retries": 3
  }
}
```

### REST API

```bash
curl -X POST http://localhost:8080/v1/run \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Analyze this code",
    "output_schema": {
      "type": "object",
      "properties": {
        "bugs": { "type": "array", "items": { "type": "string" } }
      },
      "required": ["bugs"]
    }
  }'
```

### MCP Server (`meerkat_run`)

The MCP server tool `meerkat_run` accepts `output_schema` as part of its input parameters.

## `RunResult` Fields

When structured output extraction succeeds, the `RunResult` (from `meerkat-core/src/types.rs`) contains:

```rust
pub struct RunResult {
    pub text: String,                              // Raw extraction text
    pub session_id: SessionId,
    pub usage: Usage,
    pub turns: u32,                                // Includes extraction attempts
    pub tool_calls: u32,
    pub structured_output: Option<Value>,           // Validated JSON value
    pub schema_warnings: Option<Vec<SchemaWarning>>, // Compilation warnings
}
```

The `structured_output` field is `Some(value)` when extraction succeeds and `None` when no schema was configured. The `text` field contains the raw JSON string from the last extraction response.

## SDK Usage

### Basic structured output

```rust
use meerkat_core::OutputSchema;
use serde_json::json;

let schema = OutputSchema::new(json!({
    "type": "object",
    "properties": {
        "summary": { "type": "string" },
        "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
    },
    "required": ["summary", "confidence"]
}))?;

// Configure via AgentBuildConfig
let mut build_config = AgentBuildConfig::default();
build_config.output_schema = Some(schema);
build_config.structured_output_retries = 3;
```

### Using `from_type` with schemars

```rust
use meerkat_core::OutputSchema;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(JsonSchema, Deserialize)]
struct AnalysisResult {
    summary: String,
    confidence: f64,
    issues: Vec<String>,
}

let schema = OutputSchema::from_type::<AnalysisResult>()?;
```

### Handling the result

```rust
let result: RunResult = agent.run("Analyze this code").await?;

if let Some(structured) = &result.structured_output {
    let analysis: AnalysisResult = serde_json::from_value(structured.clone())?;
    println!("Confidence: {}", analysis.confidence);
}

if let Some(warnings) = &result.schema_warnings {
    for w in warnings {
        eprintln!("Schema warning for {:?} at {}: {}", w.provider, w.path, w.message);
    }
}
```
