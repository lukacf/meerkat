---
name: Task Workflow
description: How to use task_create/task_update/task_list for structured work tracking
requires_capabilities: [builtins]
---

# Task Workflow

Use the task management tools for structured work tracking.

## Creating Tasks

Use `task_create` with a clear subject and description:
- Subject should be imperative ("Fix login bug", "Add API endpoint")
- Description should include acceptance criteria

## Status Transitions

Tasks flow through: `pending` -> `in_progress` -> `completed`
- Set `in_progress` when you start working on a task
- Set `completed` only when fully done
- Use `deleted` to remove irrelevant tasks

## Blocking Relationships

Use `addBlocks` and `addBlockedBy` to express dependencies:
- A task with `blockedBy` cannot be started until dependencies complete
- Check `TaskList` to find unblocked tasks

## Best Practices

- Create tasks before starting work to track progress
- Update status as you work
- Use `TaskList` after completing a task to find the next one
- Prefer working on tasks in ID order (lowest first)
