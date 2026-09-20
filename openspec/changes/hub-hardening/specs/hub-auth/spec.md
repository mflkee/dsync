# Spec Delta

## Purpose

Authorizes inbound QUIC requests to the hub so only known fleet machines can push state and trigger SSH pulls, with per-machine secrets that can be rotated per machine.

## ADDED Requirements

### Requirement: Hub requires authenticated requests
The hub SHALL reject any inbound request (push, pull, or status) that does not carry a valid token for a known fleet machine, and SHALL validate tokens using a constant-time comparison. Requests that fail validation SHALL NOT trigger SSH pulls, SHALL NOT modify hub state, and SHALL be answered with an error response carrying an actionable message.

#### Scenario: Valid token accepted
- **WHEN** a client sends a request with a machine name that exists in hub configuration and a matching token
- **THEN** the hub processes the request normally

#### Scenario: Unknown machine rejected
- **WHEN** a client sends a request whose machine name is not present in the hub's token configuration
- **THEN** the hub returns an error response identifying the unknown machine and does not process the request

#### Scenario: Wrong token rejected
- **WHEN** a client sends a request for a known machine but with a mismatched token
- **THEN** the hub returns an error response and does not process the request

#### Scenario: Unauthenticated push does not trigger pulls
- **WHEN** an unauthenticated or rejected push request arrives while another fleet machine is online
- **THEN** the hub rejects the request and no SSH pull is initiated on behalf of that request

### Requirement: Tokens are configured per machine and never disclosed
The hub SHALL read machine tokens from `[hub] tokens` in its config, and clients SHALL read their token from `[hub_connect] token`. Tokens SHALL NOT appear in any command output (`status`, `tui`, `doctor`, logging) in plain form; logs SHALL redact them.

#### Scenario: Token not shown in status output
- **WHEN** a user runs `dsync status` or opens the TUI with an authenticated hub
- **THEN** no token value is printed or displayed

#### Scenario: Tokens redacted in logs
- **WHEN** the hub or client logs a request or an authentication failure involving a token
- **THEN** the log line contains no full token value

### Requirement: Fail-secure hub startup
The hub SHALL refuse to start when `[hub] tokens` is absent, with an error message that explains how to configure tokens and where to generate them (`dsync init` on the hub role).

#### Scenario: Hub without tokens refuses to start
- **WHEN** `dsync hub` is started with a config that has no `[hub] tokens` section
- **THEN** the hub exits with an actionable error and does not bind the port

### Requirement: Setup and diagnostics integrate tokens
`dsync init` SHALL generate a token for the hub role and write the matching client token for local-client and fleet use. `dsync doctor` SHALL report whether the configured hub is reachable-and-authenticated and whether the local client token is present.

#### Scenario: Init writes tokens for the hub role
- **WHEN** `dsync init` completes for a machine with the hub role
- **THEN** the generated config contains a `[hub] tokens` entry for the local machine and the client side references the matching token

#### Scenario: Doctor flags a missing client token
- **WHEN** `dsync doctor` runs on a client whose config has no `[hub_connect] token`
- **THEN** the doctor output reports the missing token as a warning with a fix hint

#### Scenario: Doctor verifies authenticated connectivity
- **WHEN** `dsync doctor` runs on a client with a valid token
- **THEN** the doctor output reports the hub as reachable and authenticated