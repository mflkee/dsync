# Tasks

## 1. Config editor safety (spec: tui/config-editor-safety)

- [ ] 1.1 Add `toml_edit` to Cargo.toml and verify `cargo build` resolves it (no feature conflicts with existing `toml` crate)
- [ ] 1.2 Implement `ConfigEditor::apply_patch` that reads the live file as `toml_edit::DocumentMut` and inserts/removes `projects.<name>` and `remote.<name>` keys without touching other content; verify with a unit test that comments and an unknown `[state]` section survive an add+remove round-trip
- [ ] 1.3 Rework `ConfigEditor::save` (and the Add/RemoveProject / Add/RemoveRemote paths in `backend.rs`) to use `apply_patch`, and verify `dsync tui` add-project keeps a config with comments intact (manual check on a scratch config copy)
- [ ] 1.4 Implement chezmoi template targeting: resolve template path via `chezmoi source-path` + relative `.tmpl` path; when found, write the patch to the template and run `chezmoi apply <target>`; when missing, refuse with an actionable error; verify on a scratch chezmoi-managed config (manual: add project → template changed, live file regenerated)
- [ ] 1.5 Report a precise diff summary (added/changed/removed keys) to the event log after each save; verify by adding a project in the TUI and checking the Log tab lists exactly `projects.<name>` as added
- [ ] 1.6 Add round-trip unit tests for `apply_patch` (comments, unknown top-level section, UTF-8 values) and verify `cargo test` passes

## 2. Form editing (spec: tui/form-editing)

- [ ] 2.1 Add `cursor` and `focus` state to `FormField`/`Form` in `app.rs`; verify unit test that cursor starts at value end and field focus starts at 0
- [ ] 2.2 Handle Left/Right/Home/End in `handle_key` (cursor moves, clamped to char boundaries via `char_indices`); verify test: insert "c" at column 2 of "main" produces "macin" with cursor after "c"
- [ ] 2.3 Handle Backspace/Delete/Ctrl-U with UTF-8-safe deletes and no-op on empty field; verify tests for mid-field backspace and Ctrl-U clearing
- [ ] 2.4 Handle Up/Down (and Tab) to move focus between fields, typed keys only affect the focused field; verify test: Down twice from field 0 lands typing in field 2
- [ ] 2.5 Render the caret and "insert at column N" hint in `ui.rs` for the focused field; verify manually that a screenshot-style draw shows caret at cursor column
- [ ] 2.6 Verify full form flow manually: add project with mid-value edits and deletes, save, confirm config round-trip still passes 1.6 tests

## 3. Pull visibility and mouse (spec: tui/pull-visibility)

- [ ] 3.1 Enable robust terminal init/restore: `enable_raw_mode` + `EnableMouseCapture` + alternate screen, single `restore()` helper called on all exit paths (clean quit + `std::panic` hook); verify TUI still quits cleanly over `ssh` and shell input is unaffected afterwards
- [ ] 3.2 Handle `TermEvent::Mouse` wheel in the event loop: scroll Log/Help/Doctor content and the active list, ignored while a form is open; verify manual scroll on Log tab and that an open form is untouched by wheel
- [ ] 3.3 Render pull-failure details: on Dashboard/Machines add a detail view (one keystroke) listing machine×project×attempts×last-error rows from existing `MachineStatus.pulls`; verify empty state "no failed pulls" renders with zero errors in a doctor/status-controlled scenario
- [ ] 3.4 Verify mouse degrade path: when `EnableMouseCapture` errors, TUI continues with keyboard scrolling only (manual: run in a minimal terminal if available, else unit-test the fallback branch)

## 4. State tab (spec: tui/state-overview)

- [ ] 4.1 Add `Request::StateStatus` / `Response::StateStatus` to `protocol.rs` and hub handler summarizing `~/.local/share/dsync-hub/state/` per channel (last updated, item count, last error); verify with a hub unit test using a temp state dir
- [ ] 4.2 Client side: backend `Cmd::StateStatus` with timeout/cancellation; on unknown/error response emit `Event::StateStatus { error }`; verify against an old-style hub returns "unavailable" without hanging (test with a stub responder)
- [ ] 4.3 Add State to `Tab::ALL` and render config (tmux/tmux_restore/opencode list, with defaults when `[state]` absent) plus health rows; verify manual: open State tab on a machine with and without `[state]`
- [ ] 4.4 Query `state_status` on tab open and after each push/pull; apply results to the tab; verify the Log tab shows the query result and errors surface the error text
- [ ] 4.5 Toggle tmux/tmux_restore (key + confirm) and edit the opencode project list via form, saving through the 1.2 `apply_patch` path; verify the State tab reflects changes and 1.6 round-trip tests still pass

## 5. Integration

- [ ] 5.1 `cargo build --release` + `cargo test` green; deploy binary, run `dsync tui` end-to-end: add a project, toggle state, scroll with mouse, view pull details, quit — verify no panics and config comments survive (manual checklist)
- [ ] 5.2 Update CHANGELOG.md with the TUI improvements (config-safe editor, State tab, form editing, pull details, mouse scroll)