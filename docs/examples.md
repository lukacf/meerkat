# Examples Guide

This guide provides detailed examples for common RAIK use cases.

## Running Examples

All examples are in the `raik/examples/` directory:

```bash
# Set your API key
export ANTHROPIC_API_KEY="your-key-here"

# Run an example
cargo run --example simple
cargo run --example with_tools
cargo run --example multi_turn_tools
```

## Basic Usage

### Simple Chat

The simplest way to use RAIK:

```rust
use raik::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")?;

    let result = raik::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .run("What is the meaning of life?")
        .await?;

    println!("{}", result.text);
    Ok(())
}
```

### With System Prompt

Guide the model's behavior:

```rust
let result = raik::with_anthropic(api_key)
    .model("claude-sonnet-4")
    .system_prompt(r#"
        You are a pirate captain. You speak like a pirate.
        Always end your responses with "Arrr!"
    "#)
    .run("Tell me about the weather")
    .await?;
```

### JSON Output

Parse structured responses:

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct Analysis {
    sentiment: String,
    confidence: f64,
    key_topics: Vec<String>,
}

let result = raik::with_anthropic(api_key)
    .system_prompt("Respond only with valid JSON matching this schema: {sentiment: string, confidence: number, key_topics: string[]}")
    .run("Analyze: The product exceeded expectations and the team delivered on time.")
    .await?;

let analysis: Analysis = serde_json::from_str(&result.text)?;
println!("Sentiment: {} ({:.0}%)", analysis.sentiment, analysis.confidence * 100.0);
```

## Working with Tools

### Simple Calculator

```rust
use async_trait::async_trait;
use raik::{AgentToolDispatcher, ToolDef};
use serde_json::{json, Value};

struct Calculator;

#[async_trait]
impl AgentToolDispatcher for Calculator {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "calculate".to_string(),
                description: "Evaluate a mathematical expression".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "Math expression like '2 + 3 * 4'"
                        }
                    },
                    "required": ["expression"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        if name != "calculate" {
            return Err(format!("Unknown tool: {}", name));
        }

        let expr = args["expression"]
            .as_str()
            .ok_or("Missing expression")?;

        // Simple evaluation (use a proper math parser in production!)
        let result = eval_math(expr)?;
        Ok(format!("{}", result))
    }
}

fn eval_math(expr: &str) -> Result<f64, String> {
    // Implement or use a math parsing library
    // This is simplified for the example
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() == 3 {
        let a: f64 = parts[0].parse().map_err(|_| "Invalid number")?;
        let b: f64 = parts[2].parse().map_err(|_| "Invalid number")?;
        match parts[1] {
            "+" => Ok(a + b),
            "-" => Ok(a - b),
            "*" => Ok(a * b),
            "/" => Ok(a / b),
            _ => Err("Unknown operator".to_string()),
        }
    } else {
        Err("Invalid expression format".to_string())
    }
}
```

### File Operations

```rust
use std::fs;
use std::path::Path;

struct FileTools {
    allowed_dir: String,
}

#[async_trait]
impl AgentToolDispatcher for FileTools {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "read_file".to_string(),
                description: "Read contents of a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to read"}
                    },
                    "required": ["path"]
                }),
            },
            ToolDef {
                name: "write_file".to_string(),
                description: "Write contents to a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to write"},
                        "content": {"type": "string", "description": "Content to write"}
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDef {
                name: "list_files".to_string(),
                description: "List files in a directory".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Directory path"}
                    },
                    "required": ["path"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        let path = args["path"]
            .as_str()
            .ok_or("Missing path")?;

        // Security: ensure path is within allowed directory
        let full_path = Path::new(&self.allowed_dir).join(path);
        if !full_path.starts_with(&self.allowed_dir) {
            return Err("Path outside allowed directory".to_string());
        }

        match name {
            "read_file" => {
                fs::read_to_string(&full_path)
                    .map_err(|e| format!("Failed to read: {}", e))
            }
            "write_file" => {
                let content = args["content"]
                    .as_str()
                    .ok_or("Missing content")?;
                fs::write(&full_path, content)
                    .map_err(|e| format!("Failed to write: {}", e))?;
                Ok("File written successfully".to_string())
            }
            "list_files" => {
                let entries: Vec<String> = fs::read_dir(&full_path)
                    .map_err(|e| format!("Failed to list: {}", e))?
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                Ok(entries.join("\n"))
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

### HTTP Client Tool

```rust
struct HttpTools;

#[async_trait]
impl AgentToolDispatcher for HttpTools {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "http_get".to_string(),
                description: "Make an HTTP GET request".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL to fetch"}
                    },
                    "required": ["url"]
                }),
            },
            ToolDef {
                name: "http_post".to_string(),
                description: "Make an HTTP POST request".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL to post to"},
                        "body": {"type": "string", "description": "Request body"}
                    },
                    "required": ["url", "body"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        let url = args["url"].as_str().ok_or("Missing url")?;

        let client = reqwest::Client::new();

        match name {
            "http_get" => {
                client.get(url)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .text()
                    .await
                    .map_err(|e| e.to_string())
            }
            "http_post" => {
                let body = args["body"].as_str().ok_or("Missing body")?;
                client.post(url)
                    .body(body.to_string())
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .text()
                    .await
                    .map_err(|e| e.to_string())
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

## Multi-Turn Conversations

### Maintaining Context

```rust
// First turn
let mut agent = AgentBuilder::new()
    .model("claude-sonnet-4")
    .system_prompt("You are a helpful math tutor.")
    .build(llm, tools, store);

let result1 = agent.run("What is a quadratic equation?".to_string()).await?;
println!("Turn 1: {}", result1.text);

// Second turn - context is maintained
let result2 = agent.run("Can you give me an example?".to_string()).await?;
println!("Turn 2: {}", result2.text);

// Third turn - still has full context
let result3 = agent.run("How do I solve it?".to_string()).await?;
println!("Turn 3: {}", result3.text);

println!("Total turns: {}", result3.turns);
println!("Total tokens: {}", result3.usage.total_tokens());
```

### Session Resume

```rust
use raik::{JsonlStore, SessionId};

// Run initial conversation
let store = Arc::new(JsonlStore::new("./sessions"));
store.init().await?;

let mut agent = AgentBuilder::new()
    .model("claude-sonnet-4")
    .build(llm.clone(), tools.clone(), store.clone());

let result = agent.run("Remember: the secret code is ALPHA-7".to_string()).await?;
let session_id = result.session_id.clone();
println!("Session: {}", session_id);

// Later: resume the session
let session = store.load(&session_id).await?.expect("Session not found");

let mut resumed = AgentBuilder::new()
    .model("claude-sonnet-4")
    .resume_session(session)
    .build(llm, tools, store);

let result = resumed.run("What was the secret code?".to_string()).await?;
println!("Response: {}", result.text);  // Should mention ALPHA-7
```

## MCP Integration

### Using MCP Servers

```rust
use raik::{McpRouter, McpServerConfig};
use std::collections::HashMap;

// Configure MCP servers
let filesystem_config = McpServerConfig {
    name: "filesystem".to_string(),
    command: "npx".to_string(),
    args: vec!["-y".to_string(), "@anthropic/mcp-server-filesystem".to_string(), "/tmp".to_string()],
    env: HashMap::new(),
};

let memory_config = McpServerConfig {
    name: "memory".to_string(),
    command: "npx".to_string(),
    args: vec!["-y".to_string(), "@anthropic/mcp-server-memory".to_string()],
    env: HashMap::new(),
};

// Create router with multiple servers
let router = McpRouter::new();
router.add_server(filesystem_config).await?;
router.add_server(memory_config).await?;

// List available tools
let tools = router.list_tools().await?;
println!("Available tools:");
for tool in &tools {
    println!("  - {}: {}", tool.name, tool.description);
}

// Use with agent
let agent = AgentBuilder::new()
    .model("claude-sonnet-4")
    .build(llm, Arc::new(router), store);
```

## Error Handling

### Graceful Degradation

```rust
let result = raik::with_anthropic(api_key)
    .with_budget(BudgetLimits {
        max_tokens: Some(1000),
        max_duration: Some(Duration::from_secs(30)),
        ..Default::default()
    })
    .run("Write a very long essay about the history of computing")
    .await;

match result {
    Ok(r) => {
        println!("Response: {}", r.text);
        if r.usage.total_tokens() >= 1000 {
            println!("Note: Response may be truncated due to token limit");
        }
    }
    Err(AgentError::BudgetExhausted(budget_type)) => {
        println!("Budget exceeded: {:?}", budget_type);
    }
    Err(e) => {
        eprintln!("Error: {}", e);
    }
}
```

### Retry Logic

```rust
use raik::RetryPolicy;

let result = raik::with_anthropic(api_key)
    .with_retry_policy(RetryPolicy {
        max_retries: 5,                              // More retries
        initial_delay: Duration::from_secs(1),       // Start slower
        max_delay: Duration::from_secs(60),          // Wait up to 1 minute
        multiplier: 2.0,                             // Double each time
    })
    .run("Important task that must complete")
    .await?;
```

## Advanced Patterns

### Parallel Processing

Process multiple prompts concurrently:

```rust
use futures::future::join_all;

let prompts = vec![
    "Summarize document A",
    "Summarize document B",
    "Summarize document C",
];

let futures: Vec<_> = prompts.iter().map(|prompt| {
    raik::with_anthropic(api_key.clone())
        .model("claude-sonnet-4")
        .run(*prompt)
}).collect();

let results = join_all(futures).await;

for (i, result) in results.into_iter().enumerate() {
    match result {
        Ok(r) => println!("Result {}: {}", i, r.text),
        Err(e) => eprintln!("Error {}: {}", i, e),
    }
}
```

### Streaming Responses

Get responses as they arrive:

```rust
use raik::{AgentBuilder, AgentEvent};
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel(100);

// Run agent with event channel
let handle = tokio::spawn(async move {
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .build(llm, tools, store);

    agent.run_with_events("Write a poem".to_string(), tx).await
});

// Process events as they arrive
while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::TextDelta { delta } => {
            print!("{}", delta);  // Print without newline
            std::io::stdout().flush()?;
        }
        AgentEvent::TurnCompleted { turn, usage } => {
            println!("\n[Turn {} complete, {} tokens]", turn, usage.total_tokens());
        }
        AgentEvent::RunCompleted { result } => {
            println!("\n[Done! Session: {}]", result.session_id);
        }
        _ => {}
    }
}

let result = handle.await??;
```

### Chain of Thought

Force step-by-step reasoning:

```rust
let result = raik::with_anthropic(api_key)
    .system_prompt(r#"
        You are a careful reasoner. For every question:
        1. First, break down the problem into steps
        2. Work through each step explicitly
        3. Show your reasoning at each point
        4. Only then give your final answer

        Format your response as:
        ANALYSIS:
        [Your step-by-step reasoning]

        ANSWER:
        [Your final answer]
    "#)
    .run("If a train leaves Chicago at 9am going 60mph, and another leaves New York at 10am going 80mph, when do they meet?")
    .await?;
```

### Tool Selection Strategy

Control which tools the model can use:

```rust
struct SelectiveTools {
    all_tools: Vec<ToolDef>,
    allowed: HashSet<String>,
}

impl SelectiveTools {
    fn allow_only(&mut self, names: &[&str]) {
        self.allowed = names.iter().map(|s| s.to_string()).collect();
    }
}

#[async_trait]
impl AgentToolDispatcher for SelectiveTools {
    fn tools(&self) -> Vec<ToolDef> {
        self.all_tools.iter()
            .filter(|t| self.allowed.contains(&t.name))
            .cloned()
            .collect()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        if !self.allowed.contains(name) {
            return Err(format!("Tool '{}' is not allowed", name));
        }
        // Dispatch to underlying implementation...
        Ok("result".to_string())
    }
}
```
