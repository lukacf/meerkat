//! MCP server management CLI commands
//!
//! Provides `rkat mcp add|remove|list|get` commands for managing MCP server configuration.

use meerkat_core::mcp_config::{
    McpConfig, McpScope, McpServerConfig, McpTransportConfig, McpTransportKind, find_project_mcp,
    project_mcp_path, user_mcp_path,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use toml_edit::{Array, DocumentMut, Item, Table};

/// Truncate a string to max_chars, adding "..." if truncated (Unicode-safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_chars {
        let truncated: String = chars[..max_chars.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

/// Mask a secret value, showing only first and last 2 chars (Unicode-safe)
fn mask_secret(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        let prefix: String = chars[..2].iter().collect();
        let suffix: String = chars[chars.len() - 2..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }
}

fn format_server_target(server: &McpServerConfig) -> (McpTransportKind, String) {
    match &server.transport {
        McpTransportConfig::Stdio(stdio) => {
            let cmd = if stdio.args.is_empty() {
                stdio.command.clone()
            } else {
                format!("{} {}", stdio.command, stdio.args.join(" "))
            };
            (McpTransportKind::Stdio, cmd)
        }
        McpTransportConfig::Http(http) => {
            let kind = server.transport_kind();
            (kind, http.url.clone())
        }
    }
}

fn transport_label(kind: McpTransportKind) -> &'static str {
    match kind {
        McpTransportKind::Stdio => "stdio",
        McpTransportKind::StreamableHttp => "streamable-http",
        McpTransportKind::Sse => "sse",
    }
}

/// Add an MCP server to the configuration
pub fn add_server(
    name: String,
    transport: Option<McpTransportKind>,
    url: Option<String>,
    headers: Vec<String>,
    command: Vec<String>,
    env: Vec<String>,
    project_scope: bool,
) -> anyhow::Result<()> {
    let scope = if project_scope {
        McpScope::Project
    } else {
        McpScope::User
    };

    // Check if server already exists in this scope
    if McpConfig::server_exists(&name, scope)? {
        anyhow::bail!(
            "MCP server '{}' already exists in {} scope. Remove it first with: rkat mcp remove {} --scope {}",
            name,
            scope,
            name,
            scope
        );
    }

    // Determine transport type and build server config
    let server = match (transport, url, command.is_empty()) {
        // Explicit stdio transport
        (Some(McpTransportKind::Stdio), _, false) => {
            let env_map = parse_env_vars(&env)?;
            McpServerConfig::stdio(
                name.clone(),
                command[0].clone(),
                command[1..].to_vec(),
                env_map,
            )
        }
        // Explicit stdio but no command
        (Some(McpTransportKind::Stdio), _, true) => {
            anyhow::bail!(
                "Stdio transport requires a command. Usage: rkat mcp add <name> -t stdio -- <command> [args...]"
            );
        }
        // Explicit HTTP transport
        (Some(McpTransportKind::StreamableHttp), Some(url), _) => {
            let header_map = parse_headers(&headers)?;
            McpServerConfig::streamable_http(name.clone(), url, header_map)
        }
        // Explicit SSE transport
        (Some(McpTransportKind::Sse), Some(url), _) => {
            let header_map = parse_headers(&headers)?;
            McpServerConfig::sse(name.clone(), url, header_map)
        }
        // HTTP/SSE without URL
        (Some(McpTransportKind::StreamableHttp | McpTransportKind::Sse), None, _) => {
            anyhow::bail!(
                "HTTP/SSE transport requires --url. Usage: rkat mcp add <name> -t http --url <url>"
            );
        }
        // URL provided, no explicit transport - default to streamable-http
        (None, Some(url), _) => {
            let header_map = parse_headers(&headers)?;
            McpServerConfig::streamable_http(name.clone(), url, header_map)
        }
        // Command provided, no explicit transport - default to stdio
        (None, None, false) => {
            let env_map = parse_env_vars(&env)?;
            McpServerConfig::stdio(
                name.clone(),
                command[0].clone(),
                command[1..].to_vec(),
                env_map,
            )
        }
        // Nothing provided
        (None, None, true) => {
            anyhow::bail!(
                "Either command or URL is required.\n\
                 Stdio: rkat mcp add <name> -- <command> [args...]\n\
                 HTTP:  rkat mcp add <name> --url <url>"
            );
        }
    };

    // Get the file path for this scope
    let path = match scope {
        McpScope::User => {
            user_mcp_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
        }
        McpScope::Project => project_mcp_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine project directory"))?,
    };

    // Add to file using toml_edit to preserve formatting
    add_server_to_file(&path, &server)?;

    let (kind, target) = format_server_target(&server);
    println!(
        "Added {} MCP server '{}' ({}) to {} config: {}",
        transport_label(kind),
        name,
        target,
        scope,
        path.display()
    );
    Ok(())
}

/// Parse KEY=VALUE environment variables
fn parse_env_vars(env: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut env_map = HashMap::new();
    for e in env {
        let parts: Vec<&str> = e.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "Invalid environment variable format: '{}'. Expected KEY=VALUE",
                e
            );
        }
        env_map.insert(parts[0].to_string(), parts[1].to_string());
    }
    Ok(env_map)
}

/// Parse KEY:VALUE headers
fn parse_headers(headers: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut header_map = HashMap::new();
    for h in headers {
        let parts: Vec<&str> = h.splitn(2, ':').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid header format: '{}'. Expected KEY:VALUE", h);
        }
        header_map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
    }
    Ok(header_map)
}

/// Remove an MCP server from the configuration
pub fn remove_server(name: String, scope: Option<McpScope>) -> anyhow::Result<()> {
    // Find which scopes contain this server
    let scopes = McpConfig::find_server_scopes(&name)?;

    if scopes.is_empty() {
        anyhow::bail!("MCP server '{}' not found", name);
    }

    // If scope not specified and exists in multiple, error
    let target_scope = match scope {
        Some(s) => {
            if !scopes.contains(&s) {
                anyhow::bail!("MCP server '{}' not found in {} scope", name, s);
            }
            s
        }
        None => {
            if scopes.len() > 1 {
                anyhow::bail!(
                    "MCP server '{}' exists in multiple scopes: {:?}. Specify --scope to remove from a specific scope.",
                    name,
                    scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>()
                );
            }
            scopes[0]
        }
    };

    // Get the file path for this scope
    let path = match target_scope {
        McpScope::User => {
            user_mcp_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
        }
        McpScope::Project => {
            find_project_mcp().ok_or_else(|| anyhow::anyhow!("No project mcp.toml found"))?
        }
    };

    // Remove from file
    remove_server_from_file(&path, &name)?;

    println!(
        "Removed MCP server '{}' from {} config: {}",
        name,
        target_scope,
        path.display()
    );
    Ok(())
}

/// List configured MCP servers
pub fn list_servers(scope: Option<McpScope>, json_output: bool) -> anyhow::Result<()> {
    let servers = match scope {
        Some(s) => {
            let config = McpConfig::load_scope(s)?;
            config
                .servers
                .into_iter()
                .map(|server| meerkat_core::mcp_config::McpServerWithScope { server, scope: s })
                .collect()
        }
        None => McpConfig::load_with_scopes()?,
    };

    if json_output {
        let json: Vec<serde_json::Value> = servers
            .iter()
            .map(|s| match &s.server.transport {
                McpTransportConfig::Stdio(stdio) => serde_json::json!({
                    "name": s.server.name,
                    "transport": "stdio",
                    "command": stdio.command,
                    "args": stdio.args,
                    "env": stdio.env,
                    "scope": s.scope.to_string(),
                }),
                McpTransportConfig::Http(http) => serde_json::json!({
                    "name": s.server.name,
                    "transport": match s.server.transport_kind() {
                        McpTransportKind::Sse => "sse",
                        _ => "streamable-http",
                    },
                    "url": http.url,
                    "headers": http.headers,
                    "scope": s.scope.to_string(),
                }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        if servers.is_empty() {
            println!("No MCP servers configured.");
            println!("\nAdd a server with: rkat mcp add <name> -- <command> [args...]");
            return Ok(());
        }

        println!("{:<20} {:<10} {:<16} TARGET", "NAME", "SCOPE", "TRANSPORT");
        println!("{}", "-".repeat(60));
        for s in &servers {
            let (kind, target) = format_server_target(&s.server);
            // Truncate command if too long (Unicode-safe)
            let cmd_display = truncate_str(&target, 40);
            println!(
                "{:<20} {:<10} {:<16} {}",
                s.server.name,
                s.scope,
                transport_label(kind),
                cmd_display
            );
        }
    }

    Ok(())
}

/// Get details of a specific MCP server
pub fn get_server(name: String, scope: Option<McpScope>, json_output: bool) -> anyhow::Result<()> {
    let servers = match scope {
        Some(s) => {
            let config = McpConfig::load_scope(s)?;
            config
                .servers
                .into_iter()
                .filter(|server| server.name == name)
                .map(|server| meerkat_core::mcp_config::McpServerWithScope { server, scope: s })
                .collect::<Vec<_>>()
        }
        None => McpConfig::load_with_scopes()?
            .into_iter()
            .filter(|s| s.server.name == name)
            .collect(),
    };

    if servers.is_empty() {
        anyhow::bail!("MCP server '{}' not found", name);
    }

    let server = &servers[0];

    if json_output {
        let json = match &server.server.transport {
            McpTransportConfig::Stdio(stdio) => serde_json::json!({
                "name": server.server.name,
                "transport": "stdio",
                "command": stdio.command,
                "args": stdio.args,
                "env": stdio.env,
                "scope": server.scope.to_string(),
            }),
            McpTransportConfig::Http(http) => serde_json::json!({
                "name": server.server.name,
                "transport": match server.server.transport_kind() {
                    McpTransportKind::Sse => "sse",
                    _ => "streamable-http",
                },
                "url": http.url,
                "headers": http.headers,
                "scope": server.scope.to_string(),
            }),
        };
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("Name:    {}", server.server.name);
        println!("Scope:   {}", server.scope);
        match &server.server.transport {
            McpTransportConfig::Stdio(stdio) => {
                println!("Transport: stdio");
                println!("Command: {}", stdio.command);
                if !stdio.args.is_empty() {
                    println!("Args:    {}", stdio.args.join(" "));
                }
                if !stdio.env.is_empty() {
                    println!("Env:");
                    for (k, v) in &stdio.env {
                        let display_value = if k.to_lowercase().contains("key")
                            || k.to_lowercase().contains("secret")
                            || k.to_lowercase().contains("token")
                            || k.to_lowercase().contains("password")
                        {
                            mask_secret(v)
                        } else {
                            v.clone()
                        };
                        println!("  {}={}", k, display_value);
                    }
                }
            }
            McpTransportConfig::Http(http) => {
                let transport = match server.server.transport_kind() {
                    McpTransportKind::Sse => "sse",
                    _ => "streamable-http",
                };
                println!("Transport: {}", transport);
                println!("URL:       {}", http.url);
                if !http.headers.is_empty() {
                    println!("Headers:");
                    for (k, v) in &http.headers {
                        let display_value = if k.to_lowercase().contains("key")
                            || k.to_lowercase().contains("secret")
                            || k.to_lowercase().contains("token")
                            || k.to_lowercase().contains("password")
                        {
                            mask_secret(v)
                        } else {
                            v.clone()
                        };
                        println!("  {}: {}", k, display_value);
                    }
                }
            }
        }
    }

    Ok(())
}

// === File editing helpers ===

fn add_server_to_file(path: &Path, server: &McpServerConfig) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Read or create document
    let mut doc = if path.exists() {
        let contents = fs::read_to_string(path)?;
        contents.parse::<DocumentMut>()?
    } else {
        DocumentMut::new()
    };

    // Ensure [[servers]] array exists
    if !doc.contains_key("servers") {
        doc["servers"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }

    let servers = doc["servers"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid mcp.toml: 'servers' is not an array of tables"))?;

    // Check for duplicate
    if servers
        .iter()
        .any(|t| t.get("name").and_then(|v| v.as_str()) == Some(&server.name))
    {
        anyhow::bail!("MCP server '{}' already exists in this file", server.name);
    }

    let mut table = Table::new();
    table["name"] = toml_edit::value(&server.name);

    match &server.transport {
        McpTransportConfig::Stdio(stdio) => {
            table["command"] = toml_edit::value(&stdio.command);

            if !stdio.args.is_empty() {
                let mut args = Array::new();
                for arg in &stdio.args {
                    args.push(arg.as_str());
                }
                table["args"] = toml_edit::value(args);
            }

            if !stdio.env.is_empty() {
                let mut env_table = toml_edit::InlineTable::new();
                for (k, v) in &stdio.env {
                    env_table.insert(k, v.as_str().into());
                }
                table["env"] = toml_edit::value(env_table);
            }
        }
        McpTransportConfig::Http(http) => {
            table["url"] = toml_edit::value(&http.url);
            if !http.headers.is_empty() {
                let mut header_table = toml_edit::InlineTable::new();
                for (k, v) in &http.headers {
                    header_table.insert(k, v.as_str().into());
                }
                table["headers"] = toml_edit::value(header_table);
            }
            if matches!(server.transport_kind(), McpTransportKind::Sse) {
                table["transport"] = toml_edit::value("sse");
            }
        }
    }

    servers.push(table);

    // Write back
    fs::write(path, doc.to_string())?;
    Ok(())
}

fn remove_server_from_file(path: &Path, name: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("Config file does not exist: {}", path.display());
    }

    let contents = fs::read_to_string(path)?;
    let mut doc = contents.parse::<DocumentMut>()?;

    let servers = doc["servers"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow::anyhow!("Invalid mcp.toml: 'servers' is not an array of tables"))?;

    // Find and remove the server
    let initial_len = servers.len();
    servers.retain(|t| t.get("name").and_then(|v| v.as_str()) != Some(name));

    if servers.len() == initial_len {
        anyhow::bail!("MCP server '{}' not found in {}", name, path.display());
    }

    // Write back
    fs::write(path, doc.to_string())?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_server(name: &str, cmd: &str, args: Vec<&str>) -> McpServerConfig {
        McpServerConfig::stdio(
            name,
            cmd,
            args.into_iter().map(|s| s.to_string()).collect(),
            HashMap::new(),
        )
    }

    #[test]
    fn test_add_server_to_new_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let server = create_test_server("test-server", "npx", vec!["-y", "@test/server"]);
        add_server_to_file(&file, &server).unwrap();

        // Verify file was created and contains the server
        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains("[[servers]]"));
        assert!(contents.contains(r#"name = "test-server""#));
        assert!(contents.contains(r#"command = "npx""#));
        assert!(contents.contains(r#"args = ["-y", "@test/server"]"#));
    }

    #[test]
    fn test_add_server_to_existing_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        // Create initial file with a server
        fs::write(
            &file,
            r#"# My MCP config
[[servers]]
name = "existing"
command = "existing-cmd"
"#,
        )
        .unwrap();

        let server = create_test_server("new-server", "new-cmd", vec![]);
        add_server_to_file(&file, &server).unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        // Should preserve comment
        assert!(contents.contains("# My MCP config"));
        // Should have both servers
        assert!(contents.contains(r#"name = "existing""#));
        assert!(contents.contains(r#"name = "new-server""#));
    }

    #[test]
    fn test_add_server_with_env() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "secret123".to_string());
        let server = McpServerConfig::stdio("env-server", "cmd", vec![], env);
        add_server_to_file(&file, &server).unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains(r#"env = { API_KEY = "secret123" }"#));
    }

    #[test]
    fn test_add_duplicate_server_fails() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let server = create_test_server("dup-server", "cmd", vec![]);
        add_server_to_file(&file, &server).unwrap();

        // Adding same name again should fail
        let result = add_server_to_file(&file, &server);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_remove_server_from_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        // Create file with two servers
        fs::write(
            &file,
            r#"[[servers]]
name = "keep-me"
command = "keep"

[[servers]]
name = "remove-me"
command = "remove"
"#,
        )
        .unwrap();

        remove_server_from_file(&file, "remove-me").unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains(r#"name = "keep-me""#));
        assert!(!contents.contains(r#"name = "remove-me""#));
    }

    #[test]
    fn test_remove_nonexistent_server_fails() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        fs::write(
            &file,
            r#"[[servers]]
name = "only-server"
command = "cmd"
"#,
        )
        .unwrap();

        let result = remove_server_from_file(&file, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_remove_from_missing_file_fails() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("nonexistent.toml");

        let result = remove_server_from_file(&file, "any");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello...");
        assert_eq!(truncate_str("hi", 2), "hi");
    }

    #[test]
    fn test_truncate_str_unicode() {
        // Emoji are multi-byte but single char - truncate by char count
        assert_eq!(truncate_str("🎉🎊🎈🎁", 3), "...");
        // "hello 世界" is 8 chars, doesn't need truncation at max 8
        assert_eq!(truncate_str("hello 世界", 8), "hello 世界");
        assert_eq!(truncate_str("hello 世界!", 8), "hello...");
    }

    #[test]
    fn test_mask_secret_short() {
        assert_eq!(mask_secret("abc"), "****");
        assert_eq!(mask_secret("abcd"), "****");
    }

    #[test]
    fn test_mask_secret_long() {
        assert_eq!(mask_secret("secret123"), "se...23");
        assert_eq!(mask_secret("my-api-key"), "my...ey");
    }

    #[test]
    fn test_mask_secret_unicode() {
        // "密码很长的" is 5 chars, so shows first 2 and last 2
        assert_eq!(mask_secret("密码很长的"), "密码...长的");
    }

    // HTTP/SSE transport tests

    #[test]
    fn test_add_http_server_to_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let server = McpServerConfig::streamable_http(
            "http-server",
            "https://api.example.com/mcp",
            HashMap::new(),
        );
        add_server_to_file(&file, &server).unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains("[[servers]]"));
        assert!(contents.contains(r#"name = "http-server""#));
        assert!(contents.contains(r#"url = "https://api.example.com/mcp""#));
        assert!(!contents.contains("command")); // Should not have stdio fields
    }

    #[test]
    fn test_add_sse_server_to_file() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let server =
            McpServerConfig::sse("sse-server", "https://api.example.com/sse", HashMap::new());
        add_server_to_file(&file, &server).unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains("[[servers]]"));
        assert!(contents.contains(r#"name = "sse-server""#));
        assert!(contents.contains(r#"url = "https://api.example.com/sse""#));
        assert!(contents.contains(r#"transport = "sse""#));
    }

    #[test]
    fn test_add_http_server_with_headers() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("mcp.toml");

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Custom".to_string(), "value".to_string());
        let server =
            McpServerConfig::streamable_http("auth-server", "https://api.example.com/mcp", headers);
        add_server_to_file(&file, &server).unwrap();

        let contents = fs::read_to_string(&file).unwrap();
        assert!(contents.contains(r#"name = "auth-server""#));
        assert!(contents.contains("headers"));
        assert!(contents.contains("Authorization"));
        assert!(contents.contains("Bearer token123"));
    }

    #[test]
    fn test_parse_headers_valid() {
        let headers = vec![
            "Content-Type:application/json".to_string(),
            "Auth: Bearer xyz".to_string(),
        ];
        let result = parse_headers(&headers).unwrap();
        assert_eq!(
            result.get("Content-Type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(result.get("Auth"), Some(&"Bearer xyz".to_string()));
    }

    #[test]
    fn test_parse_headers_invalid() {
        let headers = vec!["InvalidHeader".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid header format")
        );
    }

    #[test]
    fn test_parse_env_vars_valid() {
        let env = vec!["KEY=value".to_string(), "FOO=bar=baz".to_string()];
        let result = parse_env_vars(&env).unwrap();
        assert_eq!(result.get("KEY"), Some(&"value".to_string()));
        assert_eq!(result.get("FOO"), Some(&"bar=baz".to_string())); // value can contain =
    }

    #[test]
    fn test_parse_env_vars_invalid() {
        let env = vec!["INVALID".to_string()];
        let result = parse_env_vars(&env);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid environment variable")
        );
    }

    #[test]
    fn test_format_server_target_stdio() {
        let server = McpServerConfig::stdio(
            "test",
            "npx",
            vec!["-y".to_string(), "@test/server".to_string()],
            HashMap::new(),
        );
        let (kind, target) = format_server_target(&server);
        assert_eq!(kind, McpTransportKind::Stdio);
        assert_eq!(target, "npx -y @test/server");
    }

    #[test]
    fn test_format_server_target_http() {
        let server =
            McpServerConfig::streamable_http("test", "https://api.example.com", HashMap::new());
        let (kind, target) = format_server_target(&server);
        assert_eq!(kind, McpTransportKind::StreamableHttp);
        assert_eq!(target, "https://api.example.com");
    }

    #[test]
    fn test_format_server_target_sse() {
        let server = McpServerConfig::sse("test", "https://api.example.com/sse", HashMap::new());
        let (kind, target) = format_server_target(&server);
        assert_eq!(kind, McpTransportKind::Sse);
        assert_eq!(target, "https://api.example.com/sse");
    }

    #[test]
    fn test_transport_label() {
        assert_eq!(transport_label(McpTransportKind::Stdio), "stdio");
        assert_eq!(
            transport_label(McpTransportKind::StreamableHttp),
            "streamable-http"
        );
        assert_eq!(transport_label(McpTransportKind::Sse), "sse");
    }
}
