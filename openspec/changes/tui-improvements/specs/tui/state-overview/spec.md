# Spec Delta

## Purpose

Вкладка State в TUI dsync: видимость конфигурации синхронизации состояния (tmux, сессии
opencode) и здоровья state-sync по флоту, с переключением настроек из интерфейса.

## ADDED Requirements

### Requirement: State tab shows state configuration
The TUI SHALL provide a State tab that displays the parsed `[state]` configuration:
`tmux` and `tmux_restore` booleans, and the `[state.opencode]` project list (or its default
of all configured projects). Values MUST be read from the live config at tab open and after
every config change.

#### Scenario: State tab reflects current config
- **WHEN** the user opens the State tab and the config has `[state] tmux = true, tmux_restore = false`
- **THEN** the tab shows tmux enabled, tmux_restore disabled, and the effective opencode project list

#### Scenario: Empty state section renders defaults
- **WHEN** the config has no `[state]` section at all
- **THEN** the State tab shows the defaults (tmux enabled, tmux_restore disabled) and notes that opencode sync covers all configured projects

### Requirement: State sync health is visible
The TUI SHALL query the hub for a state-sync summary (`state_status`) and show, per
channel: last successful sync time, number of locally stored items, and the most recent
error, if any. When the hub does not support the query, the TUI SHALL display
"unavailable" rather than failing.

#### Scenario: Health shown from hub response
- **WHEN** the hub answers `state_status` with a last-ok timestamp and an error for the tmux channel
- **THEN** the State tab shows the timestamp and surfaces the tmux error text

#### Scenario: Older hub reports unavailable
- **WHEN** the hub rejects or does not answer the `state_status` query
- **THEN** the State tab shows "state_status: unavailable" and keeps the rest of the UI responsive

### Requirement: State settings can be toggled
The TUI SHALL let the user toggle `tmux`/`tmux_restore` (single-key action with
confirmation) and edit the `[state.opencode]` project list via the form; saves MUST go
through the same content-preserving editor as project/remote edits.

#### Scenario: Toggle tmux with confirmation
- **WHEN** the user toggles `tmux` on the State tab and confirms
- **THEN** the config file is rewritten with `tmux` flipped and the State tab reflects the new value

#### Scenario: Edit opencode project list
- **WHEN** the user edits the opencode project list through the form and saves
- **THEN** `[state.opencode] projects` is updated, unknown config content is preserved, and the active list on the tab changes