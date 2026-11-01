# Validation

The automated suite uses Python standard-library MCP fixtures. `cargo test` covers cache protection and collector cleanup. `tests/test_e2e.py` exercises the executable through its public CLI.

Native compatibility was investigated on Apple Silicon macOS with Codex CLI/app-server 0.166.0 and the installed NodeREPL/Unified Computer Use runtime. The initial Rust protocol prototype demonstrated:

- Discovery and unused cleanup without launching the real MCP.
- JavaScript state reuse across calls and turn-end cleanup.
- Explicit reset, including reinitialization of the Computer Use API object.
- Separate state for separate connections and survival of the other connection after one closes.
- Isolated real Codex discovery, first JavaScript operation and archive-triggered group cleanup.

The production `mcp-doze` release build was then checked on 2026-11-01 against both installed REPL variants. For each variant, all five native checks passed: cold discovery/unused cleanup, state/turn-end/reset, real Codex cold discovery, real Codex first JavaScript use, and archive recovery of the owned group. No active MCP configuration was changed.

Latest cold physical-footprint samples were 1.563 MiB for the legacy-REPL launch definition and 1.641 MiB for the Unified Computer Use launch definition. These measurements cover the wrapper while its backend is absent, not memory while executing tools. They are observations on that machine, not guaranteed limits.

The initial debug prototype had measured about 1.4 MiB while cold; production results above supersede that historical figure.

An isolated Codex lifecycle probe also observed distinct MCP process groups per thread, release on archive/unload, and stdin EOF when the app server crashed. It did not establish that closing a parent terminal releases every subagent connection.

No desktop windows, browser tabs or screenshots were operated during those compatibility checks. Full graphical interaction, the host's app-managed plugin registration path, and cross-version compatibility require their own validation. No user's active MCP configuration is changed by the repository tests.

## References

- [MCP lifecycle and stdio shutdown](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Codex MCP configuration](https://learn.chatgpt.com/docs/extend/mcp)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)

## Automatic collection and authentication (0.1.1)

Five additional tests cover automatic first collection, warm reuse, entrypoint updates, concurrent initialization, authorization rejection, and early EOF/TERM/SIGKILL recovery. A separate warm-cache test checks that initialization authorization errors reach the host with their code, reason and login URL and do not enter the cache. Manual mode remains strict.

In isolated Codex checks, the installed legacy REPL worked through the wrapper from two working directories. Xcode workspace discovery worked in one context, but in another the wrapper received an agent-approval error while direct connection succeeded. Xcode was therefore left directly connected rather than changing its approvals. The error text was visible through the wrapper.

A bundled Computer Use replacement retained tool names but lost its plugin provenance in host metadata (`pluginId` became null). On-use authentication and plugin hooks were not established as equivalent. App-managed plugin connections were kept unchanged.

## Plugin transport integration and Xcode approval (0.1.2)

The Codex synchronization tests check metadata preservation, unchanged HTTP/disabled entries, idempotence, new plugin versions, private backups and rejection of symlink manifests before writes. The adapter changes transport command/arguments inside the installed plugin rather than adding top-level MCP registrations.

In an independent Codex app-server with the existing plugin installations, Unified Computer Use, Data Analytics and Creative Production retained their plugin identifiers and ON_INSTALL/ON_USE authentication policies. All three real backends stayed stopped after catalog discovery. Harmless JavaScript, a bundled Data workflow search, and reading Creative Production's bundled UI resource each started the corresponding backend on first use and returned successfully. Owned processes stopped after archiving the test thread and closing its isolated app-server. Plugin hook definitions were unchanged; the host's visible hook set matched its baseline.

Xcode identifies the immediate parent of `mcpbridge` by its signature. After signing doze with a stable identifier and approving that agent through Xcode's official flow, read-only workspace listing succeeded through Codex from both tested working directories. No real workspace was opened. Reinstalling the wrapper requires reapplying the same signing identity.
