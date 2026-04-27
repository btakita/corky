# Precommit

Steps to run before committing code changes.

## Checklist

1. **Run tests and linting**
   - Rust: `cargo clippy -- -D warnings && cargo test`
   - JS/TS: `npm test` or `bun test`
   - Python: `pytest`
   - If the change touches Gmail auth, reply threading, or connector JSON, also verify the relevant `corky doctor gmail --json` output shape

2. **Add tests for new behavior**
   - New functions/methods need unit tests covering the happy path
   - Bug fixes need regression tests proving the bug is fixed
   - Edge cases identified during implementation need coverage

3. **Audit instruction files** (if changed)
   - Verify CLAUDE.md/AGENTS.md reflects any architectural changes
   - Verify instruction examples use `Message-ID` for `in_reply_to` and Gmail `thread_id` separately
   - Check line budget is within limits

4. **Review diff**
   - No secrets, credentials, or API keys
   - No debug/temporary code left behind
   - No unrelated changes bundled in
