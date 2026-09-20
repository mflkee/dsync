# Spec Delta

## Purpose

Bounds the resources a single client can consume on the hub (payload size, in-flight work) so a faulty or malicious fleet member cannot exhaust hub memory or task capacity.

## ADDED Requirements

### Requirement: Request payload size is bounded
The hub SHALL reject any request whose serialized payload exceeds the configured `[hub] max_message_size` (default 8 MiB), and SHALL do so without processing the payload. Requests up to the limit SHALL be handled normally.

#### Scenario: Oversized request rejected
- **WHEN** a client sends a request larger than `max_message_size`
- **THEN** the hub returns an error response naming the size limit and does not process the request

#### Scenario: Request within limit accepted
- **WHEN** a client sends a request at or below `max_message_size`
- **THEN** the hub processes the request normally

#### Scenario: Limit is configurable
- **WHEN** an operator sets `[hub] max_message_size` to a custom value
- **THEN** the hub enforces that value instead of the default

### Requirement: In-flight work is capped
The hub SHALL limit the number of concurrently processed requests (connections and/or spawned pull tasks) to `[hub] max_concurrency` (default 32). Excess requests SHALL either wait for a slot or be rejected with a clear retryable error, and SHALL NOT be dropped silently.

#### Scenario: Burst within the limit is processed concurrently
- **WHEN** the number of concurrent requests is at or below `max_concurrency`
- **THEN** all requests are processed without error

#### Scenario: Burst beyond the limit is back-pressured
- **WHEN** concurrent requests exceed `max_concurrency`
- **THEN** excess requests receive a clear error or wait for a slot, and every request is either processed or answered with an explicit retryable error