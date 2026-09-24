# Spec Delta

## Purpose

Видимость сбоев pull-оркестрации и удобная прокрутка списков и лога: failed pulls с
текстом последней ошибки на Dashboard/Machines и навигация колесом мыши.

## ADDED Requirements

### Requirement: Pull failures are enumerated with error text
The Machines and Dashboard tabs SHALL list machines with failed pull records and show, for
each affected machine×project pair, the pull status (error, attempt count, last attempt
time) and the last error text. The aggregate "x failed" counter SHALL remain, but the
detail list MUST be reachable with one keystroke from any view where it is offered.

#### Scenario: Failed pull with error text
- **WHEN** a machine's pull for "dsync" failed twice, the hub records the attempts and an error message
- **THEN** the detail list shows that machine, "dsync", attempt count 2, the last attempt time and the recorded error text

#### Scenario: No failures shows a clean state
- **WHEN** all registered machines report successful pulls
- **THEN** the detail section shows "no failed pulls" and no error rows are rendered

### Requirement: Mouse wheel scrolling
The TUI SHALL capture mouse input (crossterm `EnableMouseCapture`) and scroll the active
list (Machines, Projects, Doctor output) or the Log with the wheel; scrolling MUST never
change the selected tab or steal focus from a form.

#### Scenario: Wheel scrolls the log
- **WHEN** the Log tab is active and has more lines than the viewport
- **THEN** wheel-up/wheel-down moves the log scroll offset within bounds

#### Scenario: Wheel does not disturb a focused form
- **WHEN** a modal form is open and the user scrolls the wheel
- **THEN** the form content and focus are unchanged

### Requirement: Mouse events restore terminal state on exit
When the TUI enabled mouse capture, it MUST disable it on any exit path (clean quit,
panic guard, error return) so the shell never receives mouse escape sequences.

#### Scenario: Clean quit resets mouse mode
- **WHEN** the user quits the TUI after scrolling with the wheel
- **THEN** the terminal is restored to alternate-screen mode without mouse capture and subsequent shell input is unaffected