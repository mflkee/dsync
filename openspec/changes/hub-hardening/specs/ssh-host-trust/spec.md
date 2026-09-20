# Spec Delta

## Purpose

Verifies the SSH host keys the hub relies on when pulling from fleet machines, using the same trust-on-first-use pattern already used for the QUIC hub certificate.

## ADDED Requirements

### Requirement: SSH host keys are verified by the hub
Before running a pull command over SSH, the hub SHALL verify the remote host key. On first contact with a machine, the hub SHALL record the host key (TOFU) in a persistent store; on later contacts, the presented key SHALL match the recorded one or the pull SHALL fail. The stored value SHALL be a fingerprint (e.g. `sha256:...`) rather than the raw key.

#### Scenario: First contact records the host key
- **WHEN** the hub connects via SSH to a machine it has never connected to before
- **THEN** the pull proceeds and the machine's host-key fingerprint is persisted

#### Scenario: Unchanged host key accepted
- **WHEN** the hub connects to a machine whose presented host key matches the stored fingerprint
- **THEN** the pull proceeds normally

#### Scenario: Changed host key rejected
- **WHEN** the hub connects to a machine whose presented host key differs from the stored fingerprint
- **THEN** the pull fails with an actionable error naming the mismatch and how to accept the new key

### Requirement: SSH trust can be reset
The hub SHALL provide a way to forget a machine's stored SSH host-key fingerprint, after which the next pull re-trusts via TOFU.

#### Scenario: Reset command forgets the key
- **WHEN** an operator resets SSH trust for a machine that previously failed verification
- **THEN** the machine's stored fingerprint is removed and the next pull re-records the presented key

### Requirement: Doctor reports SSH trust state
`dsync doctor` SHALL report, for each configured remote machine, whether its SSH host key is trusted, untrusted (first contact), or mismatched.

#### Scenario: Doctor lists per-machine SSH trust
- **WHEN** `dsync doctor` runs on the hub
- **THEN** the output includes SSH host-key trust state for each configured remote machine