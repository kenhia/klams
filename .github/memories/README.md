# Agent memory primers

Source-of-truth copies of agent memory files that should be loaded into
VS Code Copilot's user memory store. These files are committed to the
repo so the team shares a consistent agent-behavior baseline.

Apply them with `just prime-vscode` (stable) or
`just prime-vscode-insiders` (Insiders). The recipes copy each file in
this directory into the Copilot memory-tool directory for the matching
VS Code install.

The memory tool itself stores these under `/memories/` (user scope) —
auto-loaded into every Copilot agent session on that machine,
regardless of workspace.
