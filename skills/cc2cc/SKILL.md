---
name: cc2cc
description: Transfer a saved conversation between Claude Code and ClawCodex using its session ID. Use when the user wants to continue an existing conversation in the other tool or return after continuing it there.
---

# cc2cc

Use the installed `cc2cc` command. Do not ask the user for a transcript path or manually edit session files.

- If the user has supplied the source session ID and destination, run `cc2cc convert SESSION_ID --to claude` or `cc2cc convert SESSION_ID --to clawcodex`.
- If the ID is missing, run `cc2cc list` (or `cc2cc list --from claude` / `--from clawcodex`) and help the user select the session. Use the tool name and original working directory to distinguish sessions. Ask only when the intended session is ambiguous.
- Conversion also registers the new history in the destination tool. Keep the source session's working directory; do not substitute the current directory.
- Read the JSON result. Report the new `session_id`, `cwd`, and `resume` command. For ClawCodex, start it in `cwd` and enter the returned `/resume ID` command. For Claude Code, use the returned command directly.
- `cc2cc verify NEW_SESSION_ID` checks the saved original and the converted conversation. `--dry-run` on `convert` validates without writing.
- If a Claude conversation has multiple branches, use `--leaf UUID` only after identifying which branch the user wants. Do not guess or silently combine branches.

Source files are never overwritten. The command stores originals and restores supported metadata when converting back, including appended messages. Do not claim that every field is understood by the destination model, that a running tool or agent was migrated, or that a successful conversion proves an API request will succeed.

If validation fails, report the error and stop. Do not repair tool results by inventing output, remove preservation records, or retry with weakened checks. Do not send a model prompt merely to test the transfer unless the user requests it.
