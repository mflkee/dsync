# Tasks

## 1. Schema & protocol foundation

- [x] 1.1 Add `subtle` to `Cargo.toml` and verify `cargo build` succeeds with it
- [x] 1.2 Extend `Config`: `HubConfig` gains optional `tokens: HashMap<String,String>`, `max_message_size: u64` (default 8 MiB), `max_concurrency: u32` (default 32), `retention_days: u64` (default 30), `pull_retries: u32` (default 2); `HubConnectConfig` gains optional `token`. Verify round-trip serde tests and that old configs (without the new fields) still parse
- [x] 1.3 Extend `protocol.rs`: `token: String` (serde default empty) on `PushRequest`/`PullRequest`/`StatusRequest`; add `PullOutcome { ok, error, attempts, finished_at }` and a `pulls` map on `MachineState` with `#[serde(default)]`. Verify unit tests for (de)serialization including absence of new fields
- [x] 1.4 Add a constant-time token compare helper (via `subtle::ConstantTimeEq`) and verify unit tests cover equal vs unequal strings

## 2. hub-auth

- [x] 2.1 Fail-secure startup: `run_server` bails with an actionable error when `[hub] tokens` is empty/absent; verify by starting `dsync hub` with a token-less config (expect exit + message, no port bind)
- [x] 2.2 Hub request validation: before `handle_push`/`handle_pull`/`handle_status`, look up the request machine name in `[hub] tokens` and constant-time-compare the token; on mismatch or unknown machine return an error response, modify nothing, and trigger no SSH pulls. Verify with a local quinn endpoint test (valid token accepted; wrong token / unknown machine rejected; rejected push triggers no pull)
- [x] 2.3 Clients send their token on every request (`push`, `pull`, `status` paths); verify via a test that the sent message contains the configured token and that a wrong-configured token yields the hub's rejection error surface
- [x] 2.4 `dsync init`: hub role generates a per-machine token (`uuid` v4 hex), writes `[hub] tokens` and the local `[hub_connect] token`; verify the generated/round-tripped config contains matching tokens
- [x] 2.5 `dsync doctor`: report missing `[hub_connect] token` as a warning and verify authenticated connectivity when token is present (doctor's hub ping uses the token)

## 3. hub-request-limits

- [x] 3.1 Bounded request read: read with `take(max_message_size + 1)`; if over limit, respond with an explicit size error and process nothing. Verify with a test sending an oversized payload (assert rejection) and a within-limit payload (assert normal processing)
- [x] 3.2 Concurrency cap: guard request handling with a `Semaphore(max_concurrency)`; acquisition timeout (5 s) yields an explicit "hub busy — retry later" error, never a silent drop. Verify with a test that saturates the semaphore and observes the explicit error

## 4. hub-pull-orchestration

- [x] 4.1 Store `PullOutcome` per (machine, project) in `HubState`, persisted inside `machines.json`; verify unit tests for record/load round-trip and `#[serde(default)]` tolerance of old files
- [x] 4.2 Replace fire-and-forget in `trigger_remote_pulls` with retry loop (initial + `pull_retries` attempts, backoff `5s·2^n` capped at 60 s) that records the final outcome; verify with a test using a short backoff where the first SSH attempt fails and a later one succeeds, plus a test where all attempts fail and the outcome stores the last error
- [x] 4.3 Include pull outcomes in `PullResponse`/`StatusResponse`; `dsync status` prints per-machine pull result lines and the TUI shows them (Projects/Machines tabs). Verify manually against a local hub + fake remote (and `cargo test` for the status formatting)

## 5. hub-state-retention

- [x] 5.1 Prune stale machines (last_push > 0 AND older than `retention_days`) at hub startup and before serving `status`/`pull`; never evict a machine with `last_push == 0`. Verify unit tests: recent machine kept, old machine pruned (and persisted), zero-last_push machine kept, custom retention window honored
- [x] 5.2 Confirm pruning integrates with the atomic save (tmp + rename) — a pruned set is what gets persisted; verify the existing save-path test still passes

## 6. ssh-host-trust

- [x] 6.1 Add `ssh_known_hosts.toml` store in the hub data dir (`{host:port → sha256 fingerprint}`) with load/save/reset; verify unit tests for round-trip and missing-file default
- [x] 6.2 Fingerprint the presented SSH key blob (russh `PublicKey` bytes via the 0.44 API resolved during implementation) and wire `SshClient`'s `check_server_key` with expected/observed records (mirror of `TofuVerifier`): first contact persists the fingerprint after a successful connect, unchanged key accepted, changed key fails the pull with an actionable mismatch error. Verify with unit tests around the fingerprint helper and a fake handler-state test
- [x] 6.3 CLI `trust ssh list` / `trust ssh rm <host:port>`; verify commands show/remove host fingerprints and that after `rm` the next pull re-trusts
- [x] 6.4 `dsync doctor`: per-remote SSH trust state (trusted / untrusted-first-contact / mismatch); verify against a hub with one configured remote in each state

## 7. Verifying & release notes

- [x] 7.1 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and the full `cargo test` suite pass with the new code
- [x] 7.2 Update README (Security section: token auth + SSH host trust; config example with `[hub] tokens`, `[hub_connect] token`, limits/retention) and CHANGELOG (Unreleased: added auth/limits/retry/retention/ssh-trust, **BREAKING** note for hub tokens). Verify rendered docs mention the migration steps from design.md
- [ ] 7.3 Live migration smoke test (manual): on the hub host add `[hub] tokens`, deploy the new binary, restart `dsync-hub.service`, add a client token, then `dsync doctor` → `dsync push` → `dsync status` validates auth, pulls, and outcome display end-to-end