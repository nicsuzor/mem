---
alias:
  - wf-finish
description: mem finish template -- defines task delivery as a PR into main, verified with the full Rust check suite, with independent QA for functional changes and merge by the repository owner.
id: wf-finish
tags:
  - wf-template
  - finish
  - mem
title: mem Task Finish Template
type: template
---

## What this template does

Specifies how tasks in `nicsuzor/mem` finish. Every change delivers as a pull request into `main`. Functional changes require independent QA review on a clean checkout. The repository owner merges.

## Finish Policy

- **Target branch**: `main`. There is no integration branch. Before branching, confirm the live base with `git ls-remote --heads origin`; never take a base branch name from a task or note.
- **Delivery mechanism**: Branch from `origin/main` as `task/<task-id>-<slug>`, push, and open a pull request (`gh pr create --base main`). The PR title is a conventional commit (`feat(scope): ...`, `fix(scope): ...`, `docs: ...`, `chore: ...`); release-please derives versions and the changelog from it.
- **Merging**: Do not merge. `main` accepts changes only through a pull request; the repository owner merges. A verifying task merges only when the repository owner has authorised the merge on that task.
- **Commit trailer**: Commits must carry `Task: <task-id>` (and `Epic: <epic-id>` if applicable).
- **QA review**:
  - **Required**: For any change to Rust code (`src/`, `tests/`, `build.rs`), `Cargo.toml`/`Cargo.lock`, document or frontmatter schema, CLI behaviour, MCP tool surface or behaviour, CI workflows, or release and install scripts. `/dispatch` mints a follow-up verifying task.
  - **None**: For documentation-only changes (`README.md`, `specs/`, `references/`, `.agent/`, `.agents/`) with no functional impact.

## Worker Completion Checklist

Before marking `status: done`, the worker must:

1. Run, from the repository root, and confirm each passes:
   - `cargo check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo test --no-fail-fast`
   - `./scripts/inventory.py --lint`
   - `git ls-files -ci --exclude-standard` (must print nothing)
2. Paste the `cargo test` runner summary (per-target `test result:` lines) into the PR body and the task record. PR CI runs only on pull requests and is not sufficient evidence on its own.
3. When the MCP tool surface changes, update both `list_tools()` and `call_tool()` in `src/mcp_server.rs` and the tool catalogue in `.agent/CORE.md`. When a `FactSource` emitter is added or moved, update `INVENTORY.md`.
4. Push the feature branch and open a PR targeting `main`.
5. Check off each acceptance criterion on the PKB task with pinpoint evidence (`file:line`, command output, PR link).
6. Mark task `status: done` and release claim.

## Follow-up QA Task Specification

Where QA is required, the verifying task runs independently on a clean checkout:

- **Title**: `QA: <primary task title>`
- **Parent**: Same parent as primary task
- **Depends on**: `[<primary-task-id>]`
- **Workflow**: Composes independent verification workflow
- **Goal**: Independently verify the PR on a clean checkout of its head branch against the literal acceptance criteria, rerunning the Worker Completion Checklist commands and recording each criterion as MET or UNMET with evidence. Report the verdict on the PR and the task. Merge into `main` only when every criterion is MET and the repository owner has authorised the merge on the task; otherwise leave the PR for the repository owner.
