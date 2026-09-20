# Design

## Context

See `proposal.md` — Why. Current state to build on:

- The hub TLS layer uses `with_no_client_auth()` (`src/hub/server.rs`), trusts all SSH host
  keys (`src/ssh/client.rs` `check_server_key` → `Ok(true)`), reads request bodies unbounded
  (`recv.read_to_end(usize::MAX)`), triggers SSH pulls fire-and-forget, and never evicts stale
  machines from `machines.json`. Trust infra to reuse: `src/trust.rs` (`TrustStore`,
  sha256 fingerprints), `src/client/connect.rs` (`TofuVerifier` pattern: expected vs
  observed fingerprint through shared `Arc<Mutex<…>>`).
- Config is additive-capable (`config_version` field, optional sections) and chezmoi-templated
  per machine (`~/.config/dsync/dsync/config.toml`).
- Requests are JSON envelopes over quinn bidi streams (`src/protocol.rs`), already encrypted
  by rustls in transit (TOFU self-signed cert).

## Goals / Non-Goals

**Goals:**
- Authenticate every inbound request to the hub with a per-machine token; reject the rest.
- Bound payload size and concurrency so one client can't starve the hub.
- Track SSH pull outcomes with bounded retries; surface them in status/TUI.
- Prune stale machines; verify SSH host keys via TOFU (extend the existing trust pattern).

**Non-Goals:**
- Full mTLS (client certificates / hub-as-CA) — see Decisions; per-machine tokens are the v1.
- Fine-grained RBAC or per-command authorization beyond machine identity.
- Content security of the git repos themselves (hub already pulls from each project's origin).
- Achieving 100% uptime of fleet pulls: the goal is bounded, visible retries, not delivery guarantees.

## Decisions

### 1. Per-machine shared-secret tokens in the request envelope
`[hub] tokens = { desktop = "…", notebook = "…" }` on the hub; `[hub_connect] token = "…"`
on each client. Every `PushRequest`/`PullRequest`/`StatusRequest` gains a `token` field
(serde default empty). The hub looks up the machine name, compares with the token in
**constant time**, and rejects mismatches before any state change or pull trigger.

- *Why envelope, not a TLS change:* keeps QUIC/rustls untouched, works with the existing
  JSON protocol, and the token is encrypted in transit by the TOFU TLS session.
- *Constant-time compare:* `subtle` crate (`ConstantTimeEq`); avoid hand-rolled loops.
- *Alternatives considered:* single shared fleet token (simpler, but any holder can
  impersonate any machine — rejected); TLS client certificates via rustls `client_auth`
  + rcgen CA (stronger identity, but adds a CA lifecycle, cert enrollment/rotation for a
  4-machine fleet, and init/trust UX — deferred; the token map makes the upgrade path to
  mTLS natural later since identity is already per machine).

### 2. Fail-secure startup; additive config, same `config_version`
`run_server` refuses to start when `[hub] tokens` is empty, with a message pointing at
`dsync init` (hub role) or manual config. Client-side, the token is optional only in the
sense that runtime behavior is: hub rejects → client surfaces the error.

- `Config` schema additions are all optional (`Option`), so `config_version` stays `1` and
  old config files still parse; only the hub start check enforces the new requirement.
- `dsync init` hub role generates a token per fleet machine (`uuid` v4 hex), writes
  `[hub] tokens` locally, and prints/records the client tokens for out-of-band distribution
  (chezmoi template per host, `age`-encrypted secret, or manual copy). `doctor` checks that
  a client token exists and that connectivity through auth succeeds.

### 3. Limits: `max_message_size` + `max_concurrency`
Read the request with `AsyncReadExt::take(limit+1)`; if it exceeds `limit`, respond with an
explicit error and skip processing (never buffer the whole unbounded body). Wrap request
handling in a `tokio::sync::Semaphore(max_concurrency)`; acquisition uses a short timeout
(e.g. 5 s), and a timeout yields an explicit "hub busy — retry later" error instead of
unbounded queuing. A rejection is never silent.

### 4. Pull orchestration: tracked retries with outcome registry
Replace the fire-and-forget spawn in `trigger_remote_pulls` with a per-(machine, project)
task that:
- retries up to `pull_retries` additional times (default 2) with backoff `5s · 2^n`, capped
  at 60 s;
- records the outcome (`PullOutcome { ok, error, attempts, finished_at }`) into
  `HubState` under the machine (persisted inside `machines.json` with `#[serde(default)]`
  so old files load);
- includes outcomes in `PullResponse`/`StatusResponse` → `dsync status` prints a
  per-machine pull line, TUI shows it in the Projects/Machines tabs.

A failed pull therefore terminates in `ok=false` + error string in state, never just a log
line. Retries respect the same semaphore as request handling (no retry storm past
`max_concurrency`).

### 5. Retention pruning
Prune machines whose `last_push > 0` and `now - last_push > retention` (default 30 days;
`[hub] retention_days`). Guard `last_push > 0` so a freshly re-added machine is never
instantly evicted. Prune opportunistically: at hub startup (after load) and before each
`status()`/`pull` read. Persisted by the existing atomic tmp+rename save.

### 6. SSH host-key TOFU on the hub
- New persistent store `ssh_known_hosts.toml` in the hub data dir (same layout pattern as
  `trust.rs` `known_hosts.toml`): `{ "host:port" → fingerprint }`, where fingerprint is
  `sha256:<hex>` of the presented key blob (reuse `trust::fingerprint` helpers; russh
  `PublicKey::key_data()` / serialized blob is the source).
- `SshClient` handler gains the `expected`/`observed` fields (mirror of `TofuVerifier`);
  `check_server_key` returns `false` on mismatch (russh then fails the connection with the
  actionable message). After a successful first connect, persist the observed fingerprint.
- CLI: extend `dsync trust` with `ssh list` / `ssh rm <host:port>` (flattened into the
  existing `TrustAction` enum). `doctor` reports per-remote SSH trust: trusted / untrusted
  (first contact) / mismatch.

## Risks / Trade-offs

- **Tokens at rest in config files** → clients' `config.toml` should be chmod 0600 and
  chezmoi-encrypted (`age`); tokens are never logged (redact in tracing), never printed by
  `status`/`tui`/`doctor`.
- **Constant-time compare implemented wrong** → use the `subtle` crate, don't hand-roll.
- **Retry loops hammering a down machine** → bounded attempts (default 2 retries), capped
  exponential backoff, and `max_concurrency` semaphore bounds total in-flight pulls.
- **SSH TOFU first contact during a MITM** → same trust model as the QUIC layer; operator can
  inspect the recorded fingerprint (`trust ssh list`) and reset it; matching behavior
  documented as TOFU, not PKI.
- **`machines.json` gains fields** → all new fields `#[serde(default)]`; old files load,
  new binary writes them; rollback to the previous binary is safe.
- **Fleet outage during rollout** — fail-secure hub requires tokens before clients have them
  → deploy order below; doctor guides the migration; the old binary is the rollback.

## Migration Plan

1. On the hub host: stop `dsync-hub.service`; add `[hub] tokens` (generate via `dsync init`
   hub role, or write by hand); record client tokens.
2. Deploy the new `dsync-hub` binary; start the service — it refuses to start without
   tokens, so config must be correct first.
3. Deploy the new `dsync` client + add `[hub_connect] token` to each client config
   (chezmoi template: per-host token from an `age` secret).
4. Verify: `dsync doctor` (trust + auth) → `dsync push` → hub triggers pulls → `dsync status`
   / TUI shows pull outcomes.
5. Rollback: redeploy the previous binaries; revert config tokens (additive schema means the
   old binary ignores the new fields).

## Open Questions

- Exact russh API for the raw key blob used in the fingerprint (`key_data` vs serialized
  bytes) — resolved during implementation from the `russh 0.44` docs; the spec (fingerprint
  semantics) is unaffected.
- Whether `trust ssh rm` should also accept a machine name alias — cosmetic CLI choice,
  deferred.