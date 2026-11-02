# Development

[README](../README.md)

## Build from source

Requires Rust 1.85 or later to build. macOS is the tested platform; the implementation uses Unix APIs. Windows is not supported. The installed executable does not require Rust, Node.js or Python to run; the MCP server you wrap has its own requirements.

```sh
git clone https://github.com/mmng-a/mcp-doze.git
cd mcp-doze
cargo install --path . --locked
```

## Checks

`mcp-doze` is implemented in Rust. Each connection has its own server process and state.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Tests use Rust and local MCP fixtures. They do not require Python, external services or desktop applications. See [design](design.md) and [validation](validation.md) for implementation boundaries and compatibility evidence.

