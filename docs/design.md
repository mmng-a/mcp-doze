# Design

The MCP client is the only server configuration source. Its existing command becomes `mcp-doze run -- <original command> <original arguments>`. The environment and working directory continue to come from that client. This project does not own a server registry, synchronize configuration, or aggregate multiple MCP servers behind one endpoint.

## States

1. **Cold**: answer initialization, discovery and ping from the snapshot. Optional verified cleanup acknowledgements also stay cold.
2. **Starting**: spawn one backend. Reuse the client's initialization data, with the inner protocol version set to the version negotiated with the client. Verify initialization and every discovery page before forwarding operations. Bound pending input; cancellation removes pending work.
3. **Running**: forward original JSON frames in both directions. Do not serialize all messages through a typed MCP model, which could remove unknown extensions.
4. **Closing**: accept no new work. Recover the dedicated process group. Do not restart or replay.

Server-to-client requests can flow during startup and normal operation. The matching client responses pass back without being mistaken for new operations. Internal initialization and discovery IDs are consumed before client operations are released to the backend.

The snapshot contains normalized full discovery lists. Process-specific pagination cursors are not replayed across restarts.

## Ownership

There is one wrapper and at most one running backend per stdio connection. The wrapper uses the host-created dedicated process group, or establishes its own before spawning. The backend inherits that group. Cleanup never signals a shared group.

Cache refresh uses a separate backend group so its command can return after collection and cleanup. A wrapper does not create an external monitor or watch a global thread registry. A parent's disappearance does not imply that independently held child connections should be killed.

## Deliberate boundaries

There is no idle timeout, shared backend, transparent recovery after lost state, HTTP transport, client configuration manager or automatic migration of app-managed plugins. Unknown methods are forwarded when needed; only explicitly understood cold control traffic is handled locally.

Discovery collection advertises no optional client capabilities and cannot satisfy interactive client requests other than ping. A server requiring those interactions for discovery should remain directly connected until that integration is implemented and verified.

The cache identity does not attempt to fingerprint every inherited environment variable or every loaded dependency. Selected variables and explicit file fingerprints let the host's existing launch definition express the relevant context. Live validation before the first operation catches changes in the actual initialization and catalog.

The wrapper is responsible for its owned Unix group, not processes that detach or applications reached through external services. Final group KILL may terminate the wrapper too. This avoids a persistent process monitor and makes TERM-resistant descendants recoverable.

## Automatic collection

`--auto-refresh` is an explicit run-time option; manual strict mode remains available. A missing, invalid or changed snapshot is collected under a per-identity lock. Concurrent starters check the completed cache after acquiring that lock rather than duplicate the collector. No execution results or credentials are saved.

A short-lived invocation of the same binary owns the collector's process group and monitors an ownership pipe. EOF on that pipe triggers group recovery even if its parent was killed. The outer connection remains responsive to host disconnects while collection is pending. This helper exists only during collection and does not become a daemon. The host receives cached discovery after collection has ended.

Upstream initialization/discovery JSON-RPC errors travel over the private helper pipe and are returned with the host's request IDs. A warm cached connection likewise receives the upstream reason if live initialization fails before its operation. The error is not added to the discovery cache.
