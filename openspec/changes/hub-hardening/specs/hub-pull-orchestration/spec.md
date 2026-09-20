# Spec Delta

## Purpose

Makes hub-initiated SSH pulls to fleet machines reliable and observable: transient failures are retried instead of silently lost, and pull outcomes are visible in fleet status.

## ADDED Requirements

### Requirement: Failed SSH pulls are retried with backoff
When a hub-initiated SSH pull fails (network error, timeout, SSH auth failure, git error), the hub SHALL retry the pull up to `[hub] pull_retries` additional times (default 2) with increasing backoff between attempts. The pull SHALL be considered successful only after one attempt succeeds.

#### Scenario: Transient failure eventually succeeds
- **WHEN** the first SSH pull attempt fails with a transient error and a later attempt succeeds within the retry budget
- **THEN** the pull is recorded as successful and the machine receives the update

#### Scenario: Retries exhausted
- **WHEN** all attempts (initial plus `pull_retries`) fail
- **THEN** the pull is recorded as failed with the last error, and no further automatic attempts are made for that trigger

### Requirement: Pull outcomes are tracked and surfaced
The hub SHALL record, per machine and project, the outcome of the most recent SSH pull (success/failure, error message, timestamp) and SHALL include this information in pull/status responses so that `dsync status` and the TUI can display it.

#### Scenario: Successful pull visible in status
- **WHEN** a machine's last SSH pull succeeded
- **THEN** `dsync status` and the TUI show that machine's pull as OK with a timestamp

#### Scenario: Failed pull visible in status
- **WHEN** a machine's last SSH pull failed after retries
- **THEN** `dsync status` and the TUI show the failure and its error message instead of hiding it

### Requirement: No triggered pull is silently dropped
Every triggered SSH pull SHALL end in one of three observable states: succeeded, retried-then-succeeded, or failed-with-recorded-error. A pull SHALL NOT disappear with only a debug log line.

#### Scenario: Pull failure is always recorded
- **WHEN** a pull trigger completes without success
- **THEN** a failure record with the error reason exists in hub state and is retrievable via status