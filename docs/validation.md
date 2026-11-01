# Validation

## Automated tests

`cargo test --locked` runs the unit and integration suites. The test client, MCP fixture and process-lifecycle checks are written in Rust, using the project's existing dependencies. No Python runtime, desktop application, credentials or external service is required.

The fixture is a Cargo example at `tests/support/fixture.rs`. Cargo builds it for the tests, but normal `cargo install` does not install it. Integration tests exercise the public executable rather than importing the relay implementation.

The suite covers:

- Cached initialization, paginated discovery and unused cleanup without starting a backend.
- Independent state per connection, state reuse, negotiated protocol versions and operation cancellation.
- Bidirectional requests, media, resource access, prompts, unknown methods and JSON extension fields.
- Cache permissions, symlink rejection, launch changes and live catalog mismatches.
- Authentication error codes, reasons and login URLs reaching the client without entering the cache.
- Automatic collection, concurrent cache creation and warm-cache reuse.
- EOF, TERM and collector-owner SIGKILL, including descendants that ignore TERM.
- Backend failure without automatic replay or restart.
- Codex plugin wrapping, metadata preservation, update handling, private backups and existing-file protection.

Tests inspect and terminate only the temporary process groups they create. They do not change a user's active MCP configuration or installed plugin manifests.

## Native compatibility snapshot

The following checks were performed on Apple Silicon macOS on 2026-11-01 with Codex CLI/app-server 0.166.0 and the installed NodeREPL/Unified Computer Use runtime. They are a compatibility snapshot, not a guarantee for later host or plugin versions.

| Component | Verified behavior |
| --- | --- |
| Legacy REPL and Unified Computer Use REPL | Cold discovery, unused cleanup, JavaScript execution, state reuse, reset and connection isolation |
| Codex lifecycle | Cold discovery, first use and process-group cleanup after archive/unload |
| Unified Computer Use plugin | Original plugin identity and ON_INSTALL policy retained; harmless JavaScript worked through doze |
| Data Analytics plugin | Original plugin identity and ON_USE policy retained; bundled workflow search worked through doze |
| Creative Production plugin | Original plugin identity and ON_USE policy retained; bundled UI resource read worked through doze |
| Xcode | After signing and approving doze, workspace listing worked through Codex from two working directories |

For the plugin checks, all three real backends remained stopped after discovery, started only on their first operation, and were recovered after closing the isolated test connections. Plugin hook definitions were unchanged, and the host's visible hook set matched its baseline. This does not establish every hook's execution behavior.

No desktop windows, browser tabs or screenshots were operated during these checks. Full graphical interaction and future host-version compatibility need separate validation. Closing a parent terminal was not established to release every subagent connection.

## Memory observations

Cold release-build physical-footprint samples were 1.563 MiB for the legacy REPL launch definition and 1.641 MiB for Unified Computer Use. These measure doze while its backend is absent on the tested Mac. They do not measure memory while executing tools and are not guaranteed limits.

## Homebrew preparation

The Formula is a template for a future tagged source archive. Syntax and its local installation-test logic can be checked before publication. A complete `brew install` test still requires the real release URL and SHA-256, so it is a separate publication step in the [Homebrew guide](homebrew.md).

## References

- [MCP lifecycle and shutdown](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Codex MCP configuration](https://learn.chatgpt.com/docs/extend/mcp)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)
