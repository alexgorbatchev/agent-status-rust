---
created_on: 2026-10-14 13:06
last_modified: 2026-10-14 13:06
status: current
---

# agent-common

Run `just check` for formatting, lint, behavioral tests, and the 90% line coverage gate. Run `just build` to compile all targets. Inherit the parent Rust instructions.

- Keep JSON contracts aligned across observer, ingester, server, and frontend models; preserve completion-field precedence and required snapshot arrays.
- Preserve unknown string tags without adding known lifecycle or activity variants. Ask before adding a lifecycle or activity state.
- Extraction must use explicit telemetry evidence and exclude shell commands, conversational mentions, catalogs, and results. Native call IDs and fallback identities must remain stable across transport replay.
- Use native tracing events and spans. Keep disk output open until command errors are reported; close the active logger through its owner and surface write/flush failures.
- Zero retention limits mean unlimited. Compression, count, and age tests must use actual files; age cleanup must select only this logger's archives.
- Any runtime behavior change requires a corresponding behavioral test-file change and at least 90% line coverage; scripts are excluded.
- Never publish releases, tags, packages, or deployments without explicit authorization.

Dependency choices and verified source differences are recorded in [the dependency reference](docs/internal/dependencies.md).
