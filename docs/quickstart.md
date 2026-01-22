# Quickstart Guide

This guide will get you running your first RAIK agent in under 5 minutes.

## Prerequisites

- Rust 1.85 or later
- An API key from at least one provider:
  - Anthropic (`ANTHROPIC_API_KEY`)
  - OpenAI (`OPENAI_API_KEY`)
  - Google/Gemini (`GOOGLE_API_KEY`)

## Installation

### As a Library

Add RAIK to your project:

```bash
cargo add raik tokio --features tokio/full
```

Or manually in `Cargo.toml`:

```toml
[dependencies]
raik = "0.1"
tokio = { version = "1", features = ["full"] }
```

### As a CLI

```bash
cargo install raik-cli
```

## Your First Agent

### Step 1: Set Your API Key

```bash
export ANTHROPIC_API_KEY="your-api-key-here"
```

### Step 2: Write the Code

Create `src/main.rs`:

```rust
use raik::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")?;

    // Create and run an agent
    let result = raik::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful coding assistant.")
        .max_tokens(1024)
        .run("Write a Python function that checks if a number is prime.")
        .await?;

    // Print the response
    println!("{}", result.text);

    // Print stats
    println!("\n--- Stats ---");
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Tokens used: {}", result.usage.total_tokens());

    Ok(())
}
```

### Step 3: Run It

```bash
cargo run
```

You should see output like:

```
Here's a Python function to check if a number is prime:

```python
def is_prime(n):
    if n < 2:
        return False
    if n == 2:
        return True
    if n % 2 == 0:
        return False
    for i in range(3, int(n**0.5) + 1, 2):
        if n % i == 0:
            return False
    return True
```

--- Stats ---
Session ID: 01936f8a-7b2c-7000-8000-000000000001
Turns: 1
Tokens used: 287
```

## Using the CLI

If you installed the CLI, you can run agents directly from your terminal:

```bash
# Simple prompt
raik run "What is the capital of Japan?"

# With options
raik run --model claude-opus-4-5 --max-tokens 2048 "Explain quantum entanglement"

# JSON output for scripting
raik run --output json "What is 2+2?" | jq '.text'

# Resume a previous session
raik resume 01936f8a-7b2c-7000-8000-000000000001 "Can you explain step 3 in more detail?"
```

## Adding Tools

Agents become powerful when they can use tools. Here's a simple calculator tool:

```rust
use raik::{AgentBuilder, AgentToolDispatcher, ToolDef, AgentLlmClient, AgentSessionStore};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

// Define your tool dispatcher
struct MathTools;

#[async_trait]
impl AgentToolDispatcher for MathTools {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
            ToolDef {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        let a = args["a"].as_f64().ok_or("Missing 'a'")?;
        let b = args["b"].as_f64().ok_or("Missing 'b'")?;

        match name {
            "add" => Ok(format!("{}", a + b)),
            "multiply" => Ok(format!("{}", a * b)),
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

Then use it with the full `AgentBuilder`:

```rust
// You'll need to implement or use adapters for these traits
let llm: Arc<dyn AgentLlmClient> = /* your LLM adapter */;
let store: Arc<dyn AgentSessionStore> = /* your store adapter */;

let mut agent = AgentBuilder::new()
    .model("claude-sonnet-4")
    .system_prompt("You are a math assistant. Use tools to perform calculations.")
    .max_tokens_per_turn(1024)
    .build(llm, Arc::new(MathTools), store);

let result = agent.run("What is 25 * 4, then add 100 to the result?".to_string()).await?;
println!("{}", result.text);
println!("Tool calls made: {}", result.tool_calls);
```

## Using Different Providers

### OpenAI

```rust
let result = raik::with_openai(std::env::var("OPENAI_API_KEY")?)
    .model("gpt-4o")
    .run("Hello!")
    .await?;
```

### Gemini

```rust
let result = raik::with_gemini(std::env::var("GOOGLE_API_KEY")?)
    .model("gemini-2.0-flash-exp")
    .run("Hello!")
    .await?;
```

## Controlling Resources

### Token Limits

```rust
let result = raik::with_anthropic(api_key)
    .max_tokens(500)  // Limit response length
    .run("Write a very long story")
    .await?;
```

### Budget Limits

```rust
use raik::BudgetLimits;
use std::time::Duration;

let result = raik::with_anthropic(api_key)
    .with_budget(BudgetLimits {
        max_tokens: Some(10_000),           // Total tokens across all turns
        max_duration: Some(Duration::from_secs(30)),  // Time limit
        max_tool_calls: Some(10),           // Limit tool usage
    })
    .run("Complex multi-step task")
    .await?;
```

### Retry Policy

```rust
use raik::RetryPolicy;
use std::time::Duration;

let result = raik::with_anthropic(api_key)
    .with_retry_policy(RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        multiplier: 2.0,
    })
    .run("Task that might need retries")
    .await?;
```

## Next Steps

- Read the [Architecture Guide](./architecture.md) to understand how RAIK works
- See [Examples](./examples.md) for more complex use cases
- Check the [Configuration Guide](./configuration.md) for all options
- Browse the [API Reference](https://docs.rs/raik) for detailed documentation
