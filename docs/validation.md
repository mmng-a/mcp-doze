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

The following historical samples were collected on one Apple Silicon Mac. Dates and times below are JST. They describe the measured builds and installed runtimes at those times; they have not been rerun for every release.

### Existing server processes, 2026-10-31 22:37

A snapshot of a Codex environment with many retained threads counted these processes and summed their physical footprints:

| Component | Processes | Total footprint |
| --- | ---: | ---: |
| Legacy Node REPL | 62 | 584.5 MiB |
| Unified Computer Use launcher | 62 | 979.7 MiB |
| Unified Computer Use REPL child | 62 | 565.1 MiB |

The Unified Computer Use launcher and child totals add to 1,544.8 MiB. The README rounds the legacy REPL and combined Computer Use totals to approximately 585 MiB and 1,545 MiB. The snapshot did not establish that every process was unused or match every process to an individual thread. It is context for the cost of retaining many server instances, not a measured saving from doze.

### Cold release-build wrapper, 2026-11-01 01:23

An isolated native compatibility probe used a release build of mcp-doze with Codex CLI/app-server 0.166.0 and the installed legacy Node REPL and Unified Computer Use runtimes. After initialization, discovery and unused cleanup, the probe confirmed that the owned process group contained only mcp-doze and sampled its footprint:

| Launch definition | mcp-doze processes | Backend processes | Wrapper footprint |
| --- | ---: | ---: | ---: |
| Legacy Node REPL | 1 | 0 | 1.563 MiB |
| Unified Computer Use | 1 | 0 | 1.641 MiB |

Each row is a single observed sample, not an average, a peak or a guaranteed limit. The figures exclude backend startup, cache collection and tool execution. The same probe separately verified first use, state reuse, reset and cleanup; it did not record a memory comparison during tool execution.

### Method and interpretation

Both measurements read `ri_phys_footprint` through macOS `proc_pid_rusage` and converted bytes to MiB by dividing by 1,048,576. The existing-server snapshot summed the selected component processes; the cold probe measured one wrapper process per launch definition.

Physical footprint accounts for compressed and swapped memory as well as noncompressed dirty memory, so it differs from RSS and from the amount of physical RAM that would be freed by stopping a process. See [Apple's explanation of memory footprint](https://developer.apple.com/documentation/xcode/analyzing-the-memory-usage-of-your-metal-app).

The snapshots have different process counts, ages and workloads and are not a controlled before/after benchmark. They do not establish a percentage reduction, CPU or energy savings, or a change in system-wide memory pressure or swap use. Once a backend starts, total memory use includes both the wrapper and that backend, which stays running until the client releases the connection. Actual savings depend on the workload and how many backends remain unused.

## Homebrew distribution

On 2026-11-01, the public `mmng-a/tap/mcp-doze` Formula was checked with Homebrew 7.0.11 on Apple Silicon macOS. `brew style`, `brew audit --strict`, a build-from-source installation of the `v0.1.2` tag archive, and `brew test` all passed. The downloaded source archive matched the Formula's SHA-256. The installed package's test verified its version and plugin-definition wrapping under a temporary Codex home.

The repository retains a Formula template for future releases. Update the version and archive checksum, then repeat the installation checks in the [Homebrew guide](homebrew.md).

## References

- [MCP lifecycle and shutdown](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Codex MCP configuration](https://learn.chatgpt.com/docs/extend/mcp)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)
