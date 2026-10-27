---
created_on: 2026-10-14 13:06
last_modified: 2026-10-14 13:06
status: current
---

# Dependencies and source contracts

This reference describes dependency integration and material behavior differences for maintainers of `agent-common`.

The source is `agent-status/packages/agent-common`, whose Go manifest pins `agent-parser` v1.0.1 and lumberjack v2.2.1. Its root README declares MIT copyright 2026 Alex Gorbatchev. Rust uses the independently migrated sibling `agent-parser` and preserves the shared telemetry JSON field names.

| Source dependency or primitive | Rust integration |
| :--- | :--- |
| Go encoding/json | Native serde models and explicit snapshot serialization. |
| Go regexp, crypto/sha256 | `regex` and `sha2`; transcript whitespace classes preserve Go's ASCII regexp semantics. Persisted observation IDs preserve Go's HTML-safe string encoding before hashing. |
| slog | Native `tracing` events, dispatches, span extensions and a subscriber layer; no slog compatibility facade. |
| lumberjack | `flexi_logger` file-writer builder, timestamp names, gzip compression, synchronous cleanup and count policy. A narrow age policy applies to native-selected archives because the native cleanup enum does not combine age and count. |
| Home directory lookup | `dirs` plus standard environment and filesystem operations. |

The Rust API owns each logger and passes its native dispatch explicitly. This replaces the source's global mutable rotator. Native span scope names qualify their fields; entering a child span does not retroactively regroup parent attributes. Disk file names follow the maintained writer's native convention. The native writer performs compression and count cleanup synchronously; its own diagnostic paths can report cleanup failures to stderr. Recorded console/file write and flush failures are returned by `finish()`.

## Verified corrections

- The Go environment parser accepts zero backup/age limits but `NewRotator` replaces them with defaults. Rust preserves explicit zero independently for each limit. The behavioral mutation check restores that source defect and confirms the zero-limit test fails.
- The Go capacity resolver tries broad family guesses before catalog prefixes and iterates a map for fallback prefix matches. Rust chooses the longest catalog prefix first, keeping `gpt-4-32k-custom` and `o1-preview-custom` capacities specific and deterministic.
- Published capacity values replace stale source entries: [GPT-5](https://developers.openai.com/api/docs/models/gpt-5) is 400,000 tokens; [GPT-5.4](https://developers.openai.com/api/docs/models/gpt-5.4) and [GPT-5.5](https://developers.openai.com/api/docs/models/gpt-5.5) are 1,050,000; [Gemini 2.5 Pro](https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro) accepts 1,048,576 input tokens. [Claude context documentation](https://platform.claude.com/docs/en/build-with-claude/context-windows) gives the Opus 5, Sonnet 5, Fable 5 and Fable 5.1 catalog entries a 1,000,000-token window. Other inherited catalog entries and unknown-family fallbacks remain estimates rather than live provider metadata.
- Invalid log levels return a typed I/O input error instead of silently selecting `info`. Service-derived log paths reject path components; callers choose explicit file paths through `FileConfig`.

No provider API calls or live agent service deployment are part of local verification. The crate does not implement observer/server behavior; those are separate consumers.
