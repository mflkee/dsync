# Spec Delta

## Purpose

Removes long-dead machines from hub storage so fleet state and status views do not accumulate offline machines forever.

## ADDED Requirements

### Requirement: Stale machines are pruned
The hub SHALL remove a machine from its stored state (and from status responses) when no push has been received from it within `[hub] retention` (default 30 days). Pruning SHALL apply to already-stored data on load and to new data as it ages, and SHALL be persisted.

#### Scenario: Recently active machine is kept
- **WHEN** a machine pushed within the retention window
- **THEN** the machine remains in hub state and status output

#### Scenario: Machine older than the window is pruned
- **WHEN** a machine's last push is older than the retention window
- **THEN** the machine no longer appears in hub state or status output, and the pruning is persisted

#### Scenario: Retention window is configurable
- **WHEN** an operator sets `[hub] retention` to a custom duration
- **THEN** the hub prunes machines inactive for that duration instead of the default

#### Scenario: Pruned machine reappears on new push
- **WHEN** a previously pruned machine sends a new valid push
- **THEN** the hub stores it again as an online machine