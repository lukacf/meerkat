---
name: Shell Patterns
description: Background job patterns with shell and job management tools
requires_capabilities: [builtins, shell]
---

# Shell Patterns

## Running Commands

Use `shell` to execute commands. For long-running processes, use `background=true`.

## Background Jobs

1. Start: `shell` with `background=true` returns a `job_id`
2. Check: `shell_job_status` with the job ID to see progress
3. Cancel: `shell_job_cancel` to stop a running job

## Output Handling

- Shell output is truncated to prevent context overflow
- For large outputs, pipe through `head`, `tail`, or `grep`
- Use `shell_job_status` to poll for completion

## Working Directory

- Commands run in the project root by default
- Use absolute paths or `cd` within the command when needed

## Timeout Management

- Default timeout applies to foreground commands
- Background jobs run until completion or cancellation
- Use `shell_job_cancel` for stuck background processes
