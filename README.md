# mcp-doze

[English](README.md) | [日本語](README.ja.md)

**Reduce resource usage from unused MCP servers.**

In Codex, [subagents inherit their parent's MCP configuration](https://learn.chatgpt.com/docs/agent-configuration/subagents). As chats and subagents accumulate, the same local MCP server can end up running in many separate instances.

A server using tens of megabytes may seem small, but many instances can add up to gigabytes. Servers can start just to list their tools, consuming memory even when those tools are never used.

mcp-doze is a wrapper that serves initialization and discovery from a cache, then starts the real server when an operation arrives. Keep your MCPs registered while reducing unused server processes, including local MCPs bundled with Codex plugins.

## Install

macOS is the tested platform. The implementation targets Unix; Windows is not supported.

With Homebrew 7 or later, add the tap and trust the mcp-doze Formula before installing:

```sh
brew tap mmng-a/tap
brew trust --formula mmng-a/tap/mcp-doze
brew install mmng-a/tap/mcp-doze
```

## Usage

Update an existing MCP registration to launch its server through mcp-doze. For a Node.js server in Codex's `config.toml`:

```toml
[mcp_servers.example]
command = "mcp-doze"
args = ["run", "--auto-refresh", "--", "node", "/absolute/path/to/server.mjs"]
```

If the MCP client cannot find `mcp-doze` because its `PATH` does not include Homebrew's bin directory, set `command` to the absolute path returned by `command -v mcp-doze` in your terminal. Keep the original environment, working directory, approval policy and tool allowlists.

## Codex plugins

To wrap bundled MCPs too, run this before starting Codex:

```sh
mcp-doze codex sync-plugins
```

This changes launch definitions inside the original plugins while preserving their identity, authentication policy, skills and hook definitions. Plugin updates may replace those definitions, so run it again before startup. See the [integration details](docs/usage.md#codex-plugins).

## Notes

- Local stdio MCP connections only. HTTP connections are outside the wrapper.
- A missing or outdated cache temporarily starts the real server to collect discovery metadata.
- Each MCP connection has its own doze process. Once started, a backend keeps its state until the client closes the connection; there is no idle timeout.

## Documentation

- [Configuration and behavior](docs/usage.md): options, cache, authentication and plugin integration
- [Design](docs/design.md) and [validation, including memory observations](docs/validation.md)
- [Source builds and development](docs/development.md) and [Homebrew distribution](docs/homebrew.md)

## License

[MIT](LICENSE). Design reference: [lazy-mcp-wrapper](https://github.com/sleticalboy/lazy-mcp-wrapper).
