Use spawn-agent for all codebase exploration.
Each agent reply must be super-compact: symbols only, no prose.
Do not read repository source files directly (except this file, plan files, and files you create).
Do not run broad discovery commands; only targeted reads through agents.
Keep edits minimal and scoped; do not run broad context reads after starting implementation.
Terminate and clean up agent processes immediately once they return their results (close each with the returned ID).
