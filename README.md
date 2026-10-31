# mcp-doze

Start a stdio MCP server on its first real operation. Keep it alive until the host disconnects.

`mcp-doze` is a small Rust wrapper, not an MCP registry or gateway. Your existing MCP client remains the only configuration source. Each client connection gets its own backend process and state.

- Answer initialization and discovery from a private local cache.
- Start the real server only when an operation needs it.
- Preserve state across calls and turns; there is no idle timeout.
- Forward requests, responses, notifications and unknown JSON fields in both directions.
- Stop the owned process group on disconnect or termination.
- No background daemon, shared backend or HTTP listener.

## Install

Requires Rust 1.85 or later. macOS is the primary tested platform; the process implementation uses Unix APIs. Windows is not supported.

```sh
git clone https://github.com/mmng-a/mcp-doze.git
cd mcp-doze
cargo install --path . --locked
```

The executable is `mcp-doze`. This repository does not install or modify any MCP configuration automatically.

## Use

First, collect the server's discovery information. Run this from the **same working directory and environment** that the MCP client will use:

```sh
mcp-doze cache refresh -- python3 /absolute/path/to/server.py
```

Then wrap the existing command in your client's configuration. For example, in Codex's `config.toml`:

```toml
[mcp_servers.example]
command = "/absolute/path/to/mcp-doze"
args = ["run", "--", "python3", "/absolute/path/to/server.py"]
```

Keep the existing environment, working directory, approval settings and tool allowlists in the client configuration. There is no second server configuration file to maintain.

```text
client connection A → mcp-doze A → server A, started on demand
client connection B → mcp-doze B → server B, started on demand
```

Discovery metadata may be reused. Live servers, variables and browser state are never shared.

### Commands and options

```text
mcp-doze run [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache refresh [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache inspect [OPTIONS] -- COMMAND [ARG...]

--cache-dir PATH       Override the cache directory
--cache-env NAME       Include an environment variable in the cache identity; repeatable
--fingerprint PATH     Track an additional script or manifest; repeatable
--repl-cleanup         Enable the verified REPL turn_ended acknowledgement while cold
--timeout-ms N         Initialization and I/O timeout, default 10000
```

Use the same options and backend command for `refresh`, `inspect` and `run`. Arguments after `--` are passed directly to the real server; no shell is involved.

The cache identity includes the executable, arguments, cwd, file argument metadata, explicit fingerprints, selected environment values and cleanup policy. Environment variables are inherited unchanged, but only variables named by `--cache-env` affect the cache key. Include variables that change the available tools or instructions. Their values are hashed into the identity and are not stored as plain text.

If a server loads code that is not a file argument, add it with `--fingerprint`. Directory fingerprints identify the directory itself, not every file under it; use files or manifests to track code updates.

### REPL cleanup notifications

Some REPL servers expose a `turn_ended` tool that the host calls after each turn, even before the REPL is used. To avoid starting the server for that cleanup alone:

```sh
mcp-doze cache refresh --repl-cleanup -- /absolute/path/to/repl-server
mcp-doze run --repl-cleanup -- /absolute/path/to/repl-server
```

This is an explicit opt-in for a verified input schema: three required, nonempty strings named `hook_event_name`, `session_id` and `turn_id`, with no extra arguments. Cache collection rejects a different schema. While cold, the wrapper returns the verified empty cleanup result. Once the server is running, cleanup calls go to the real server. `js_reset` is always a real operation.

## Cache and failure behavior

Cache collection starts the real server once, reads its initialization result and all supported discovery pages, stops the collector's process group, and atomically saves the result. It stores instructions, capabilities, tool schemas and resource/prompt listings, not execution results or runtime state.

Default location:

- macOS: `~/Library/Caches/mcp-doze`
- Other Unix systems: `~/.cache/mcp-doze`
- With `XDG_CACHE_HOME`: `$XDG_CACHE_HOME/mcp-doze`

Directories must be owned by the current user with mode `0700`; files must have mode `0600`. A missing, changed, corrupt or inaccessible cache produces an error with a refresh hint. It never silently falls back to eager startup.

Before the first real operation, the wrapper checks the live server's initialization and complete discovery results against the snapshot. A mismatch invalidates the snapshot and stops before sending the operation. Live list-change notifications are forwarded and invalidate the disk cache.

Caches cannot detect an unannounced catalog change before the server starts. Servers whose discovery depends on unsupported client interactions or varies per connection may need to stay directly connected.

Backend startup failure, malformed transport or a backend crash closes the connection. Operations are never automatically retried or replayed, and a lost REPL is not silently restarted.

## Lifetime and cleanup

The connection owns a dedicated process group. The backend inherits that group, including ordinary child processes. EOF, SIGTERM, SIGINT, SIGHUP, output failure or an unrecoverable error starts cleanup. The wrapper closes backend input, sends TERM, then KILL after a two-second grace period. Final group KILL intentionally includes the wrapper itself; a used wrapper can therefore exit by signal during normal disconnect.

A turn finishing does not stop the backend. The host must actually release the MCP connection. A terminal closing may not immediately release every connection in a shared app server. Parent/subagent cleanup decisions remain the host's responsibility.

Processes that deliberately detach with `setsid()`, already-running applications and external services are outside the owned group. Killing only the wrapper with uncatchable SIGKILL cannot run its cleanup. The wrapper never finds and kills applications by name.

## Limits and compatibility

- Local stdio JSON-RPC only; no HTTP/SSE/OAuth proxy or server aggregation.
- Maximum frame size: 64 MiB. Pending startup input is also limited to 64 MiB; bounded I/O channels can hold additional frames.
- Startup and individual writes have a timeout. The host's tool timeout also includes lazy startup time.
- Discovery must be cacheable; not every MCP server has a stable catalog.
- Reconnecting or reloading MCP settings creates a new backend and loses its previous state.
- Bundled plugin servers must be wrapped through supported host configuration. Editing a downloaded plugin cache is not supported.

The repository's test suite uses local fixtures and never operates desktop applications. Native REPL/Computer Use and isolated Codex checks are described in [validation](docs/validation.md); full desktop interaction and automatic plugin migration are not claimed.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/test_e2e.py --binary target/debug/mcp-doze -v
cargo build --release --locked
```

The end-to-end suite owns and cleans only its temporary process groups. It checks lazy discovery, state reuse and isolation, reverse requests, cancellation, metadata and media forwarding, cache invalidation, crashes and TERM-resistant descendants.

## License

MIT. The implementation is independent Rust code. [lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper) was a design reference; no shared daemon or idle shutdown is used.
