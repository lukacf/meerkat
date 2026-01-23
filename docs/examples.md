# Meerkat Examples Walkthrough

This guide provides detailed walkthroughs of the runnable examples in the Meerkat repository. Each section explains not just *what* the code does, but *why* it's structured that way.

## Overview

| Example | File | What It Demonstrates |
|---------|------|---------------------|
| Simple Chat | `simple.rs` | Minimal SDK usage, fluent API |
| Using Tools | `with_tools.rs` | Custom tool implementation, AgentBuilder |
| Multi-turn Tools | `multi_turn_tools.rs` | Stateful tools, conversation continuity |
| Inter-Agent Comms | `comms_verbose.rs` | Two agents communicating via TCP |

## Prerequisites

### API Keys

All examples require an Anthropic API key:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

For the comms example, you can optionally specify a different model:

```bash
export ANTHROPIC_MODEL="claude-sonnet-4-20250514"
```

### Running Examples

```bash
# From the repository root
cargo run --example simple
cargo run --example with_tools
cargo run --example multi_turn_tools
cargo run --example comms_verbose
```

---

## 1. Simple Chat (`simple.rs`)

**Location:** `meerkat/examples/simple.rs`

### What It Demonstrates

This is the "hello world" of Meerkat. It shows the simplest possible way to interact with an LLM using the fluent SDK API.

### Code Walkthrough

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    // Step 2: Build and run in one fluent chain
    let result = meerkat::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant. Be concise in your responses.")
        .max_tokens(1024)
        .run("What is the capital of France? Answer in one sentence.")
        .await?;

    // Step 3: Use the result
    println!("Response: {}", result.text);
    println!("\n--- Stats ---");
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Total tokens: {}", result.usage.total_tokens());

    Ok(())
}
```

### Why This Pattern?

**Fluent builder API:** The `meerkat::with_anthropic(api_key)` pattern creates a builder that lets you chain configuration. This is idiomatic Rust and makes the code self-documenting:

```rust
meerkat::with_anthropic(api_key)  // Start with provider + credentials
    .model("...")                  // Required: which model
    .system_prompt("...")          // Optional: guide behavior
    .max_tokens(1024)              // Optional: limit response length
    .run("...")                    // Execute with user prompt
```

**The result struct:** `AgentResult` gives you everything you need:
- `result.text` - The model's response
- `result.session_id` - For resuming conversations later
- `result.turns` - How many LLM calls were made
- `result.usage` - Token counts for cost tracking

### Expected Output

```
Response: The capital of France is Paris.

--- Stats ---
Session ID: sess_abc123...
Turns: 1
Total tokens: 47
```

### Key Learnings

1. **Minimal boilerplate:** Meerkat handles session management, message formatting, and streaming internally.
2. **Everything is async:** The `run()` method is async because it makes network calls.
3. **Sensible defaults:** You only need to specify what matters for your use case.

---

## 2. Using Tools (`with_tools.rs`)

**Location:** `meerkat/examples/with_tools.rs`

### What It Demonstrates

How to give an agent access to custom tools. This example shows the full `AgentBuilder` API with explicit components rather than the simplified SDK.

### Architecture Overview

The example creates three components that work together:

```
+---------------------+     +----------------------+     +-------------+
| MathToolDispatcher  |     | AnthropicLlmAdapter  |     | MemoryStore |
| (your tools)        |     | (LLM provider)       |     | (sessions)  |
+---------+-----------+     +-----------+----------+     +------+------+
          |                             |                       |
          +-----------------------------+-----------------------+
                                        |
                                +-------v-------+
                                |  AgentBuilder |
                                +-------+-------+
                                        |
                                +-------v-------+
                                |     Agent     |
                                +---------------+
```

### Code Walkthrough

#### Step 1: Implement `AgentToolDispatcher`

```rust
struct MathToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for MathToolDispatcher {
    // Define what tools exist
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
            // ... multiply tool ...
        ]
    }

    // Handle tool calls from the LLM
    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "add" => {
                let a = args["a"].as_f64().ok_or("Missing 'a' argument")?;
                let b = args["b"].as_f64().ok_or("Missing 'b' argument")?;
                Ok(format!("{}", a + b))
            }
            // ... other tools ...
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

**Why this design?**

- **`tools()`** returns JSON Schema definitions. The LLM uses these to understand what tools are available and how to call them.
- **`dispatch()`** executes the tool when called. It receives the tool name and arguments as JSON.
- **Return `String`:** Tool results are always strings because they go back into the conversation.

#### Step 2: Implement `AgentLlmClient`

This wraps `AnthropicClient` to implement the trait:

```rust
#[async_trait]
impl AgentLlmClient for AnthropicLlmAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, meerkat::AgentError> {
        // Build request
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        // Stream and collect response
        let mut stream = self.client.stream(&request);
        // ... process stream events ...

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }
}
```

**Why implement this trait?**

The `AgentLlmClient` trait abstracts over different LLM providers. Meerkat-core doesn't care if you're using Anthropic, OpenAI, or a local model - it just needs something that implements this trait.

#### Step 3: Build and Run the Agent

```rust
let llm = Arc::new(AnthropicLlmAdapter::new(api_key, "claude-sonnet-4".to_string()));
let tools = Arc::new(MathToolDispatcher);
let store = Arc::new(MemoryStore::new());

let mut agent = AgentBuilder::new()
    .model("claude-sonnet-4")
    .system_prompt("You are a math assistant. Use the provided tools to perform calculations.")
    .max_tokens_per_turn(1024)
    .build(llm, tools, store);

let result = agent
    .run("What is 25 + 17, and then multiply the result by 3?".to_string())
    .await?;
```

### Expected Output

```
Response: Let me calculate that for you.

First, 25 + 17 = 42

Then, 42 * 3 = 126

So the final answer is 126.

--- Stats ---
Session ID: sess_xyz...
Turns: 3
Tool calls: 2
Total tokens: 234
```

Notice that `turns: 3` even though you only called `run()` once. This is because:
1. Turn 1: LLM decides to call `add(25, 17)`
2. Turn 2: LLM receives "42", decides to call `multiply(42, 3)`
3. Turn 3: LLM receives "126", generates final response

### Key Learnings

1. **The agent loops automatically:** When an LLM wants to use a tool, Meerkat handles the back-and-forth.
2. **Tools are JSON-defined:** Use JSON Schema to describe inputs. The LLM reads this.
3. **Arc wrapping:** Components are wrapped in `Arc` because they're shared across async tasks.
4. **Explicit is better:** The full API gives you control over every component.

---

## 3. Multi-turn with Tools (`multi_turn_tools.rs`)

**Location:** `meerkat/examples/multi_turn_tools.rs`

### What It Demonstrates

- Stateful tools that persist data across conversation turns
- Multiple conversation turns with a single agent instance
- How the agent maintains context between `run()` calls

### Architecture: Stateful Tools

```rust
struct MultiToolDispatcher {
    // Tools can have internal state
    state: std::sync::Mutex<AppState>,
}

struct AppState {
    notes: Vec<String>,        // Persists across turns
    calculations: Vec<f64>,    // History of calculations
}
```

**Why use `Mutex`?**

The dispatcher might be called from multiple async tasks. `Mutex` ensures thread-safe access to the state. For a single-threaded runtime, you could use `RefCell`, but `Mutex` is safer.

### Tools Defined

| Tool | Purpose |
|------|---------|
| `calculate` | Evaluate arithmetic expressions |
| `save_note` | Store a note for later |
| `get_notes` | Retrieve all saved notes |
| `get_calculation_history` | Show past calculations |

### Conversation Flow

```rust
// Turn 1: Calculate and save
let result = agent
    .run("Calculate 15 * 8, then save a note about the result.".to_string())
    .await?;
// LLM calls calculate("15 * 8") -> "120"
// LLM calls save_note("The calculation 15 * 8 equals 120")

// Turn 2: More calculations
let result = agent
    .run("Now calculate 100 / 4 and 25 + 37.".to_string())
    .await?;
// LLM calls calculate("100 / 4") -> "25"
// LLM calls calculate("25 + 37") -> "62"

// Turn 3: Review history (this proves state persists!)
let result = agent
    .run("Show me all the calculations we've done and any notes we've saved.".to_string())
    .await?;
// LLM calls get_calculation_history() -> "[120.0, 25.0, 62.0]"
// LLM calls get_notes() -> "1. The calculation 15 * 8 equals 120"
```

### Key Pattern: Same Agent Instance

```rust
let mut agent = AgentBuilder::new()
    // ... configuration ...
    .build(llm, tools, store);

// Reuse the SAME agent instance
let result1 = agent.run("First message".to_string()).await?;
let result2 = agent.run("Second message".to_string()).await?;  // Remembers result1
let result3 = agent.run("Third message".to_string()).await?;   // Remembers result1 + result2
```

**Why this works:**

The `Agent` holds a `Session` internally. Each `run()` call:
1. Adds the user message to the session
2. Calls the LLM with the full conversation history
3. Adds the assistant response to the session
4. Saves the session (via the store)

### Expected Output

```
=== Multi-Turn Tool Usage Example ===

--- Turn 1: Calculate and save a note ---
Response: I calculated 15 * 8 = 120 and saved a note about it.
Tool calls: 2

--- Turn 2: More calculations ---
Response: Here are the results: 100 / 4 = 25, and 25 + 37 = 62.
Tool calls: 2

--- Turn 3: Review calculation history and notes ---
Response: Here's what we've done so far:

Calculations: [120.0, 25.0, 62.0]

Notes:
1. The calculation 15 * 8 equals 120

Tool calls: 2

--- Turn 4: Save a summary ---
Response: I've saved a summary note of our calculation session.
Tool calls: 1

=== Final Statistics ===
Session ID: sess_...
Total turns: 11
Total tokens: 1847
```

### Key Learnings

1. **Stateful tools are powerful:** Your tools can maintain state between turns, enabling complex workflows.
2. **Conversation context is automatic:** The agent handles message history for you.
3. **Turn count accumulates:** The `turns` field shows total LLM calls across all `run()` invocations.
4. **Tool state vs conversation state:** Tools hold application state; the agent holds conversation state.

---

## 4. Inter-Agent Communication (`comms_verbose.rs`)

**Location:** `meerkat/examples/comms_verbose.rs`

### What It Demonstrates

Two completely separate agent instances communicating over TCP using encrypted channels. This is the foundation for building multi-agent systems.

### Architecture Overview

```
+----------------------------------------------------------------+
|                        AGENT A                                 |
|  +--------------+  +---------------+  +------------------+     |
|  | CommsManager |  | TCP Listener  |  | Agent + LLM      |     |
|  | (keypair A)  |  | (port 12345)  |  | (send_message    |     |
|  +--------------+  +---------------+  |  list_peers)     |     |
|                                       +------------------+     |
+----------------------------------------------------------------+
                              |
                              | TCP + Encryption
                              v
+----------------------------------------------------------------+
|                        AGENT B                                 |
|  +--------------+  +---------------+  +------------------+     |
|  | CommsManager |  | TCP Listener  |  | Agent + LLM      |     |
|  | (keypair B)  |  | (port 12346)  |  | (processes inbox)|     |
|  +--------------+  +---------------+  +------------------+     |
+----------------------------------------------------------------+
```

### Code Walkthrough

#### Step 1: Identity Setup (Cryptographic Keys)

```rust
let keypair_a = Keypair::generate();
let keypair_b = Keypair::generate();
let pubkey_a = keypair_a.public_key();
let pubkey_b = keypair_b.public_key();
```

**Why keypairs?**

Each agent has a cryptographic identity. Messages are signed and encrypted, ensuring:
- Authentication: You know who sent a message
- Integrity: Messages can't be tampered with
- Confidentiality: Only the intended recipient can read it

#### Step 2: Trusted Peers Configuration

```rust
// Agent A knows about Agent B
let trusted_for_a = TrustedPeers {
    peers: vec![TrustedPeer {
        name: "agent-b".to_string(),
        pubkey: pubkey_b,
        addr: format!("tcp://{}", addr_b),
    }],
};

// Agent B knows about Agent A
let trusted_for_b = TrustedPeers {
    peers: vec![TrustedPeer {
        name: "agent-a".to_string(),
        pubkey: pubkey_a,
        addr: format!("tcp://{}", addr_a),
    }],
};
```

**Why explicit trust?**

Agents only accept messages from peers in their trusted list. This prevents spam and ensures security in multi-agent systems.

#### Step 3: CommsManager Setup

```rust
let config_a = CommsManagerConfig::with_keypair(keypair_a)
    .trusted_peers(trusted_for_a.clone());
let comms_manager_a = CommsManager::new(config_a);
```

The `CommsManager` handles:
- Message encryption/decryption
- Inbox queue management
- Outbound message routing

#### Step 4: TCP Listeners

```rust
let _handle_a = spawn_tcp_listener(
    &addr_a.to_string(),
    secret_a,
    Arc::new(trusted_for_a.clone()),
    comms_manager_a.inbox_sender().clone(),
).await?;
```

Each agent listens on its own TCP port. When a message arrives:
1. Verify the sender's signature against trusted peers
2. Decrypt the message
3. Push to the inbox queue

#### Step 5: Comms Tools for LLM

```rust
let tools_a = CommsToolDispatcher::new(
    comms_manager_a.router().clone(),
    Arc::new(trusted_for_a),
);
```

This gives the LLM two tools:
- `send_message` - Send a message to a named peer
- `list_peers` - List all trusted peers

#### Step 6: Build CommsAgent

```rust
let agent_a_inner = AgentBuilder::new()
    .model(&model)
    .system_prompt(
        "You are Agent A. You can communicate with other agents using the send_message tool."
    )
    .build(llm_a, tools_a, store.clone());

let mut agent_a = CommsAgent::new(agent_a_inner, comms_manager_a);
```

`CommsAgent` wraps a regular `Agent` and adds:
- Automatic inbox polling
- Message injection into conversation

### Execution Flow

```
Phase 1: Agent A sends message
------------------------------
User -> Agent A: "Send 'Hello from Agent A!' to agent-b"
Agent A LLM: Calls send_message tool
send_message: Encrypts + TCP sends to Agent B's port
Agent A LLM: "I've sent the message"

Phase 2: Message delivery (500ms delay)
---------------------------------------
TCP packet arrives at Agent B's listener
Listener: Verifies signature, decrypts, pushes to inbox

Phase 3: Agent B processes inbox
--------------------------------
Agent B: Checks inbox, finds message from agent-a
Agent B LLM: Receives message as context
Agent B LLM: "I received: 'Hello from Agent A!' from agent-a"
```

### Logging Output

The example includes detailed logging wrappers that show every API call:

```
+==============================================================+
|  ANTHROPIC API CALL #1 - Agent A
+==============================================================+
| Model: claude-sonnet-4-20250514
| Tools provided: ["send_message", "list_peers"]
| Messages in context: 2
| Last user message: Send the message 'Hello from Agent A!' to agent-b...
+==============================================================+

+--- LLM REQUESTED TOOL: send_message ---+
| Args: {"to":"agent-b","message":"Hello from Agent A!"}
+----------------------------------------+

+==============================================================+
|  TOOL EXECUTION #1 - Agent A calling 'send_message'
+==============================================================+
| Args: {
|   "to": "agent-b",
|   "message": "Hello from Agent A!"
| }
| SUCCESS: Message sent to agent-b
+==============================================================+
```

### Key Learnings

1. **Agents are fully independent:** Each has its own keypair, port, LLM client, and state.
2. **Security is built-in:** Cryptographic signatures and encryption by default.
3. **Tools enable communication:** The LLM uses tools to interact with the comms layer.
4. **Async inbox processing:** Messages arrive asynchronously; agents check when ready.

---

## Building Your Own Examples

### Template for New Examples

```rust
//! Brief description of what this example demonstrates
//!
//! Run with:
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example your_example
//! ```

use meerkat::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup: Get configuration from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    // 2. Build your agent (choose one approach)

    // Option A: Simple SDK (no tools)
    let result = meerkat::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .run("Your prompt here")
        .await?;

    // Option B: Full builder (with tools)
    // let llm = Arc::new(YourLlmAdapter::new(api_key));
    // let tools = Arc::new(YourToolDispatcher);
    // let store = Arc::new(MemoryStore::new());
    // let mut agent = AgentBuilder::new()
    //     .model("claude-sonnet-4")
    //     .build(llm, tools, store);
    // let result = agent.run("Your prompt".to_string()).await?;

    // 3. Handle the result
    println!("Response: {}", result.text);

    Ok(())
}
```

### Common Patterns

#### Pattern: Tool with External State

```rust
struct DatabaseTools {
    pool: Arc<sqlx::PgPool>,
}

#[async_trait]
impl AgentToolDispatcher for DatabaseTools {
    // ... tool definitions ...

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "query" => {
                let sql = args["sql"].as_str().ok_or("Missing sql")?;
                // Use self.pool to execute query
                Ok("results...".to_string())
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
```

#### Pattern: Composing Multiple Tool Dispatchers

```rust
struct CompositeDispatcher {
    dispatchers: Vec<Arc<dyn AgentToolDispatcher>>,
}

#[async_trait]
impl AgentToolDispatcher for CompositeDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.dispatchers.iter()
            .flat_map(|d| d.tools())
            .collect()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        for dispatcher in &self.dispatchers {
            if dispatcher.tools().iter().any(|t| t.name == name) {
                return dispatcher.dispatch(name, args).await;
            }
        }
        Err(format!("Unknown tool: {}", name))
    }
}
```

#### Pattern: Validating Tool Arguments

```rust
async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "send_email" => {
            // Extract and validate
            let to = args["to"].as_str()
                .ok_or("Missing 'to' field")?;
            let subject = args["subject"].as_str()
                .ok_or("Missing 'subject' field")?;
            let body = args["body"].as_str()
                .ok_or("Missing 'body' field")?;

            // Validate format
            if !to.contains('@') {
                return Err("Invalid email address".to_string());
            }

            // Execute
            send_email(to, subject, body).await
                .map_err(|e| format!("Failed to send: {}", e))
        }
        _ => Err(format!("Unknown tool: {}", name)),
    }
}
```

### Error Handling Best Practices

```rust
// DO: Return descriptive error messages
async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
    let path = args["path"].as_str()
        .ok_or("Missing required 'path' argument")?;

    std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read '{}': {}", path, e))
}

// DON'T: Return generic errors
async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
    let path = args["path"].as_str().ok_or("error")?;
    std::fs::read_to_string(path).map_err(|_| "failed".to_string())
}
```

### Testing Your Agent

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_dispatch() {
        let dispatcher = MathToolDispatcher;

        // Test add
        let result = dispatcher.dispatch(
            "add",
            &json!({"a": 2, "b": 3})
        ).await;
        assert_eq!(result, Ok("5".to_string()));

        // Test missing argument
        let result = dispatcher.dispatch(
            "add",
            &json!({"a": 2})
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore]  // Requires API key
    async fn test_full_agent() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
        let result = meerkat::with_anthropic(api_key)
            .model("claude-sonnet-4")
            .run("Say 'hello' and nothing else")
            .await
            .unwrap();

        assert!(result.text.to_lowercase().contains("hello"));
    }
}
```

---

## Summary

| Example | Key Concept | When to Use This Pattern |
|---------|-------------|-------------------------|
| `simple.rs` | Fluent SDK API | Quick scripts, simple queries |
| `with_tools.rs` | Custom tools + AgentBuilder | Apps that need tool access |
| `multi_turn_tools.rs` | Stateful tools + conversation | Complex workflows, assistants |
| `comms_verbose.rs` | Inter-agent communication | Multi-agent systems |

Start with `simple.rs` to understand the basics, then progress to the others as your needs grow. The examples are designed to build on each other conceptually.
