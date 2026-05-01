# Prerelease

Steps to run before publishing a release.

## Common (all targets)

1. Run full test suite
2. Verify version bumped in manifest (Cargo.toml / package.json / pyproject.toml)
3. Update VERSIONS.md / CHANGELOG.md with new version entry
4. Audit instruction files for staleness and correctness
   - For OAuth/browser-flow changes, confirm docs mention listener-first bind order and any session-level callback-port override
   - For `corky draft send` changes, confirm docs mention the dedicated `gmail.compose` send token key and the rerun-`corky draft send` 401 remediation path
   - For shared token/sync-state changes, confirm docs mention lock files, atomic replace, and merge-on-save behavior
5. No secrets in the diff or release notes
6. No machine-local paths in released files
7. For Gmail connector-facing changes, verify `corky doctor gmail --json`, `corky sync refetch --json`, and draft JSON surfaces still match the documented contract

## Target: cargo (crates.io)

- `cargo publish --dry-run`
- Verify `Cargo.toml` metadata (description, license, repository)

## Target: npm

- `npm pack --dry-run`
- Verify `package.json` fields (name, version, main/exports)

## Target: github-release

- Push a version tag: `git tag v<version> && git push origin v<version>`
- CI creates the release with cross-platform binaries
