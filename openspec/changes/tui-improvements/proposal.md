# Proposal

## Why

The TUI (`dsync tui`) is functional but has real rough edges: the config editor rewrites
`config.toml` from a serde struct, silently dropping comments and any section the binary
doesn't know, and for chezmoi-managed configs a TUI edit lives only until the next
`chezmoi apply`. State sync (tmux + opencode sessions) shipped in v0.x but is invisible in
the TUI — there is no way to see what the fleet is syncing or to toggle it. Forms only
support appending characters (no cursor navigation, no Home/End), and pull failures are
hidden behind an aggregate "x failed" counter.

## What Changes

- **Config editor rewrites the file safely** (`tui/config-editor-safety`): project/remote
  edits are applied with `toml_edit`, preserving comments, ordering and unknown sections;
  the editor refuses (with an actionable message) to edit a chezmoi-managed live file whose
  template is out of sync, and instead offers to apply the change to the template itself.
- **New State tab** (`tui/state-overview`): shows the `[state]` configuration (tmux /
  tmux_restore / opencode projects) and the state-sync health (last tmux snapshot, opencode
  session counts, in-flight errors) pulled from the hub via a lightweight
  `state_status` query; `t` toggles tabs, per-item editing goes through a form.
- **Form editing gets real cursor keys** (`tui/form-editing`): Left/Right move the cursor,
  Home/End jump, Backspace/Delete edit in place, Ctrl-U clears the field; the form shows a
  visible "insert at column N" indicator.
- **Pull failures and scrolling become visible** (`tui/pull-visibility`): pull records with
  errors are listed with their last error in a dedicated section of the Machines/Dashboard
  tab, and mouse wheel scrolls lists and the log (mouse capture enabled).

No **BREAKING** changes: the TUI stays backward-compatible; the new `state_status` hub
query is additive (older hubs simply return "unavailable").

## Capabilities

- **New Capabilities**:
  - `tui/config-editor-safety`
  - `tui/state-overview`
  - `tui/form-editing`
  - `tui/pull-visibility`
- **Modified Capabilities**: none (`openspec/specs/` is currently empty).

## Impact

- `src/tui/config.rs` (`ConfigEditor`) — replace whole-file TOML rewrite with `toml_edit`
  section patching + chezmoi template awareness.
- `src/tui/backend.rs` — new `Cmd::StateStatus` command, hub query for state summary.
- `src/protocol.rs` + `src/hub/server.rs` + `src/client/connect.rs` — additive
  `state_status` request/response.
- `src/tui/app.rs`, `src/tui/ui.rs` — new tab, form cursor handling, mouse capture
  (crossterm `EnableMouseCapture`), pull-details rendering.
- `Cargo.toml` — add `toml_edit` dependency.
- Tests: `tests/` (config round-trip), plus unit tests in TUI modules.