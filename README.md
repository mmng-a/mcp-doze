# mcp-doze

[English](README.md) | [日本語](README.ja.md)

**Reduce resource usage from unused MCP servers.**

When several local MCP servers are configured, `mcp-doze` keeps unused servers from running in the background. It serves tool lists from a cache and starts the real server when an operation arrives.

Once started, the same server handles later operations and keeps its state until the client disconnects. Add `mcp-doze` before an existing server command to use it.

## Example

```diff
-node /absolute/path/to/server.mjs
+mcp-doze run --auto-refresh -- node /absolute/path/to/server.mjs
```

Keep the server registered in your MCP client. Configuration, credentials and approval policy remain with the client.

## Install

### Homebrew (macOS)

With Homebrew 7 or later, add the tap and trust the mcp-doze Formula before installing:

```sh
brew tap mmng-a/tap
brew trust --formula mmng-a/tap/mcp-doze
brew install mmng-a/tap/mcp-doze
```

The [tap](https://github.com/mmng-a/homebrew-tap) builds from source; Homebrew provides Rust as a build dependency. See the [packaging guide](docs/homebrew.md) for release maintenance.

### From source

Requires Rust 1.85 or later to build. macOS is the tested platform; the implementation uses Unix APIs. Windows is not supported. The installed executable does not require Rust, Node.js or Python to run; the MCP server you wrap has its own requirements.

```sh
git clone https://github.com/mmng-a/mcp-doze.git
cd mcp-doze
cargo install --path . --locked
```

## Quick start

Keep your existing server registration and put `mcp-doze run --auto-refresh --` before its command. For example, in Codex's `config.toml`:

```toml
[mcp_servers.example]
command = "/absolute/path/to/mcp-doze"
args = ["run", "--auto-refresh", "--", "node", "/absolute/path/to/server.mjs"]
```

Use `command -v mcp-doze` to find the installed path. Preserve the original environment, working directory, approval policy and tool allowlists. No second server registry is needed.

| When | What happens |
| --- | --- |
| Discovery cache is missing or outdated | A temporary server collects discovery metadata, then stops |
| A connection asks for initialization or lists | doze replies from the cache |
| The first operation arrives | doze starts the server and checks its discovery results |
| Later operations arrive | The same server handles them; original JSON frames pass through |
| The client releases the connection | doze stops the server's owned process group |

The first connection in a new working directory may need to collect a cache. A cached tool list does not mean that authentication has been checked.

## Commands

```text
mcp-doze run [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache refresh [OPTIONS] -- COMMAND [ARG...]
mcp-doze cache inspect [OPTIONS] -- COMMAND [ARG...]
mcp-doze codex sync-plugins [--codex-home PATH]

--auto-refresh         Collect missing or outdated discovery metadata automatically
--cache-dir PATH       Override the cache directory
--cache-env NAME       Include an environment variable in the cache identity; repeatable
--fingerprint PATH     Track an extra script or manifest; repeatable
--repl-cleanup         Acknowledge the verified turn_ended tool before server startup
--timeout-ms N         Initialization and individual write timeout; default 10000
```

Arguments after `--` go directly to the server without a shell. `--auto-refresh` affects `run` only. Without it, an unavailable cache produces an error and a refresh hint.

To collect a cache explicitly, use the same command, working directory, environment and cache options as the client:

```sh
mcp-doze cache refresh -- node /absolute/path/to/server.mjs
mcp-doze cache inspect -- node /absolute/path/to/server.mjs
```

### Cache identity and storage

The cache identity includes the executable, arguments, working directory, metadata for file arguments, explicit fingerprints, selected environment values and cleanup policy. Inherited environment variables are passed through unchanged. Only names supplied through `--cache-env` affect the cache key; select variables that change the catalog or instructions. Their values are hashed rather than stored in plain text.

Use `--fingerprint` for code or manifests loaded indirectly. A directory fingerprint tracks the directory itself, not its contents.

Default cache directories are `~/Library/Caches/mcp-doze` on macOS and `~/.cache/mcp-doze` on other Unix systems. `XDG_CACHE_HOME`, when set, takes precedence. Directories must be owned by the user with mode `0700`, and files with mode `0600`.

### REPL cleanup

Some REPL hosts call a `turn_ended` tool even if the REPL has not been used. Add `--repl-cleanup` to avoid starting the server solely for that call:

```sh
mcp-doze run --auto-refresh --repl-cleanup -- /absolute/path/to/repl-server
```

This option requires the verified schema: exactly three required nonempty strings, `hook_event_name`, `session_id` and `turn_id`. A different schema is rejected. Before startup, doze returns the empty cleanup result; after startup, the real server handles cleanup. Reset operations always start the real server.

## Authentication and errors

Credentials stay with the MCP client, the server or the provider's existing credential store. doze has no credential store and does not perform OAuth login. HTTP MCP connections remain outside this wrapper.

Initialization/discovery errors are returned to the client with their error codes and details. Tool errors are forwarded unchanged. Authentication errors and execution results are not cached. The client decides how to display an error or login prompt.

Before forwarding the first operation, doze compares the live initialization and catalog with the cache. A mismatch invalidates the cache and closes the connection before executing the operation. A server crash or malformed transport also closes the connection. Operations are never automatically retried, and a lost REPL is not silently restarted.

## Codex plugins

Wrap bundled MCPs inside their original plugins to retain plugin identity, authentication policy, skills and hook definitions:

```sh
mcp-doze codex sync-plugins
```

This opt-in command scans `plugins/cache/<marketplace>/<plugin>/<version>/.mcp.json` under `CODEX_HOME`, defaulting to `~/.codex`. It changes only `command` and `args` for stdio definitions that are not explicitly disabled. HTTP definitions and already wrapped servers are unchanged. CUA receives `--repl-cleanup`. Private backups go to `backups/mcp-doze/plugins` in the same Codex home.

The command edits downloaded plugin caches, rather than using an official Codex transport override. Plugin updates can replace those files. Run it before starting Codex to reapply wrapping. A launcher earlier on `PATH` can do this automatically:

```sh
#!/bin/sh
set -eu
/absolute/path/to/mcp-doze codex sync-plugins >/dev/null
exec /absolute/path/to/the/real/codex "$@"
```

The launcher and real Codex binary must have different paths. Synchronization failures stop the launcher with an error. An existing Codex app-server can retain old launch definitions until it exits; doze does not restart it or interrupt ongoing work.

## Xcode

Xcode identifies the agent that starts `mcpbridge` by its immediate parent's code signature. Approve doze as a separate agent through Xcode's normal flow. To retain that identity across upgrades, sign the installed binary with the same signing identity and identifier each time:

```sh
security find-identity -v -p codesigning
codesign --force --sign "YOUR_CODE_SIGNING_IDENTITY" \
  --identifier io.github.mmng-a.mcp-doze --timestamp=none /absolute/path/to/mcp-doze
```

`XcodeListWorkspaces` can report an approval error without requesting access. `XcodeOpenWorkspace` requests access and opens the specified workspace when approved. For headless Xcode, inspect `xcrun mcp-server status` and approve the relevant pending request using Xcode's official command. doze does not modify Xcode's approval database.

## Lifecycle and compatibility

- One backend per connection. No shared backend, resident daemon or idle timeout.
- A turn ending does not stop the backend. The host must release its MCP connection; closing a terminal may leave connections in a shared app-server.
- EOF, termination signals and unrecoverable errors trigger cleanup: close server input, send TERM, then KILL after two seconds. Final group KILL includes doze itself, so a used wrapper can exit by signal during normal cleanup.
- Detached processes, existing applications and external services are outside the owned group. Killing only doze with uncatchable SIGKILL can leave the running backend behind.
- Local stdio JSON-RPC only. Current discovery uses the `initialize` handshake; the `2026-07-28` `server/discover` flow is not implemented.
- Discovery must be cacheable. Connection-dependent catalogs or interactive discovery need separate compatibility work.
- Maximum frame size and queued startup input: 64 MiB each. Bounded I/O channels can hold additional frames.
- The host's tool timeout must allow for lazy server startup. Reconnecting creates a new backend and loses its previous state.

## Development

`mcp-doze` is implemented in Rust. Each connection has its own server process and state.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Tests use Rust and local MCP fixtures. They do not require Python, external services or desktop applications. See [design](docs/design.md) and [validation](docs/validation.md) for implementation boundaries and compatibility evidence.

## License

[MIT](LICENSE). Independent Rust implementation, with [lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper) as a design reference.
