# Proposal

## Why

The QUIC hub currently accepts **any** peer on the network: the server TLS config uses
`with_no_client_auth()` (`src/hub/server.rs:242`), so anyone who can reach port 42069 can
push spoofed machine state, flood the hub with heavy messages (`recv.read_to_end(usize::MAX)`,
unbounded), and trigger SSH pulls against fleet machines. On the reliability side, SSH pulls
are fire-and-forget (`trigger_remote_pulls` spawns tasks with no retry — a temporarily-down
machine silently loses a pull cycle), stale machines linger in `machines.json` forever
(`HubState::update_machine` never evicts), and the hub's outbound SSH accepts **any** host
key (`check_server_key` returns `Ok(true)`), leaving hub→machine pulls open to MITM. Phase 4
hardens the hub into a trustable coordinator.

## What Changes

- **Client authentication on the hub** (`hub-auth`): the hub rejects requests from
  unrecognized machines. Each configured machine gets its own token; the client presents it
  per-request and the hub validates it in constant time. Requests with unknown machine names
  or mismatched tokens get a clear error, are logged, and are never forwarded to SSH pulls.
  **BREAKING**: a hub with tokens configured rejects unauthenticated clients; existing
  deployments must add tokens before upgrading (fail-secure: hub without tokens refuses to
  start with an actionable error; `dsync doctor` surfaces the fix).
- **Bounded hub resources** (`hub-request-limits`): maximum request payload size and a
  cap on concurrent connections/requests, so a single (even authenticated) client cannot
  exhaust hub memory or task count.
- **Reliable SSH pull orchestration** (`hub-pull-orchestration`): misses are not silent —
  SSH pulls get bounded retries with backoff, and per-machine/recent-pull status is surfaced
  in `dsync status`/TUI so failed pulls are visible instead of vanishing into a log line.
- **Stale-machine retention** (`hub-state-retention`): machines no longer seen within a
  configurable window are pruned from hub state (retention, not just "offline forever").
- **SSH host-key trust on the hub** (`ssh-host-trust`): the hub no longer accepts any SSH
  host key when pulling from fleet machines — first contact remembers the key (TOFU, same
  pattern as `known_hosts.toml`), later pulls verify it; changed keys fail loudly.
- `dsync init` generates/embeds tokens for both hub and client roles; `dsync doctor` checks
  token presence and SSH host trust.

## Capabilities

### New Capabilities
- `hub-auth`: hub identity/authorization for inbound QUIC requests — machine tokens,
  constant-time validation, rejection semantics, init/doctor integration.
- `hub-request-limits`: bounded request payload size and hub concurrency limits.
- `hub-pull-orchestration`: reliable hub→machine SSH pulls — bounded retry with backoff,
  pull outcomes tracked and surfaced in status/TUI.
- `hub-state-retention`: pruning of stale machine state from hub storage.
- `ssh-host-trust`: TOFU verification of fleet SSH host keys by the hub for outbound pulls.

### Modified Capabilities
- None (fresh OpenSpec project — all capabilities are new).

## Impact

- **Affected code**: `src/hub/server.rs` (auth check, limits, pull trigger), `src/hub/state.rs`
  (retention/pruning, pull-outcome tracking), `src/client/connect.rs` (send token on every
  request), `src/client/push.rs`/`pull.rs` (request auth field), `src/protocol.rs` (auth on
  requests, new fields), `src/config.rs` + schema (`[hub] tokens`, `[hub_connect] token`,
  `[hub] limits`, `[hub] retention`, `[hub] ssh_known_hosts`), `src/trust.rs` (SSH host-key
  store), `src/ssh/client.rs` (host-key check), `src/init.rs` (token generation/enrollment),
  `src/doctor.rs` (new checks), `src/tui/` (pull-outcome display).
- **Config**: new optional sections; tokens are read from config only (never logged or shown
  in `status`/`tui`).
- **Deployment**: hub restart required; existing hubs must add `[hub] tokens` (fail-secure
  startup). Fleet clients add `[hub_connect] token`.
- **Binaries/services**: `dsync-hub.service` unchanged; data dir gains
  `ssh_known_hosts.toml` alongside `machines.json`.