//! Simple example demonstrating basic RAIK usage
//!
//! Run with:
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example simple
//! ```

use raik::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    // Create and run an agent with the simplified SDK
    let result = raik::with_anthropic(api_key)
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant. Be concise in your responses.")
        .max_tokens(1024)
        .run("What is the capital of France? Answer in one sentence.")
        .await?;

    println!("Response: {}", result.text);
    println!("\n--- Stats ---");
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Total tokens: {}", result.usage.total_tokens());

    Ok(())
}
