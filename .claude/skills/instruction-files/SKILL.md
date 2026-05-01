# instruction-files

Audit and manage instruction files (AGENTS.md, CLAUDE.md, SKILL.md, runbooks).

## Invocation

```
/instruction-files audit [--fix]
/instruction-files init
```

- `/instruction-files audit` — run the full audit suite against all discovered instruction files
- `/instruction-files audit --fix` — fix auto-fixable issues (staleness touch, budget hints)
- `/instruction-files init` — scaffold `.agent/runbooks/` with bundled defaults

## Audit Checks

The audit discovers and validates these file types:

- `AGENTS.md`, `README.md`, `SPEC.md` for root-level project docs
- `CLAUDE.md` for per-directory agent instructions when `include_claude_md` is enabled
- `.claude/**/SKILL.md` for Claude Code skill definitions
- `.agent/runbooks/*.md` for project runbooks
- `.claude/skills/**/runbooks/*.md` for skill-specific runbooks
- `src/**/AGENTS.md` for submodule or package instructions

### Check Suite

1. **Tree paths** — Verify paths in `## Project Structure` blocks exist on disk
2. **Actionable content** — Flag informational-only sections, large code blocks without imperative context, oversized tables, link-heavy lists
3. **Line budget** — Combined agent instruction files must stay under 1000 lines
4. **Staleness** — Flag instruction files older than the newest source file
5. **Context invariant** — Flag home-directory shortcuts and user-specific absolute paths that won't resolve on other machines

Staleness is mtime-based during local development, so after code changes such as OAuth loopback behavior updates you should refresh the relevant instruction surfaces before expecting `audit-docs` to pass.

### Running the Audit

From the project root:

```bash
# Programmatic (Rust)
use instruction_files::{AuditConfig, run};
run(&AuditConfig::agent_doc(), None).unwrap();

# Via a project precommit step
make precommit
```

## Init Runbooks

`init_runbooks(root)` scaffolds `.agent/runbooks/` with bundled defaults:
- `precommit.md` — standard precommit checklist
- `prerelease.md` — standard prerelease checklist

Never overwrites existing files. Safe to run repeatedly.

## Runbooks

- [Instruction Files Audit](runbooks/instruction-files-audit.md) — step-by-step audit procedure
