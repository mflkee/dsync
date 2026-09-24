# Spec Delta

## Purpose

Надёжное редактирование `config.toml` из TUI: без потери комментариев, неизвестных секций и
без тихого затирания правок на chezmoi-управляемых файлах.

## ADDED Requirements

### Requirement: Config edits preserve file contents
The config editor SHALL apply project/remote changes to the existing TOML file with a
tombstone-preserving merge: comments, blank lines, section ordering and any keys the
current binary does not know MUST survive an edit. The editor SHALL never rewrite the file
from a serde struct that drops unknown content.

#### Scenario: Edit project keeps comments and unknown sections
- **WHEN** the running config contains `[state]`, `[hub.tokens]` and inline comments, and the user adds a project in the TUI
- **THEN** the resulting file still contains the `[state]` and `[hub.tokens]` sections, all comments, and the new `[projects.<name>]` block

#### Scenario: Unknown top-level section survives
- **WHEN** the file contains an unrecognized top-level section written by a newer dsync version, and the user removes a remote in the TUI
- **THEN** the unrecognized section remains in the file unchanged

### Requirement: Chezmoi-managed config is not silently clobbered
For a config file that is chezmoi-managed, the editor SHALL detect it before saving. If a
matching template exists in the chezmoi source path, the TUI SHALL offer to apply the edit
to the template and run `chezmoi apply`; the user MUST confirm this explicitly. If the
template cannot be located, the TUI SHALL refuse the save with an actionable message
instead of writing the live file.

#### Scenario: Edit on chezmoi-managed config goes through the template
- **WHEN** the live config is chezmoi-managed, a source template exists, and the user confirms the template-edit flow
- **THEN** the template is updated, `chezmoi apply` regenerates the live file, and the TUI reports that the change is applied fleet-wide

#### Scenario: Template location unknown blocks the save
- **WHEN** the live config is chezmoi-managed but no source template can be found
- **THEN** the TUI refuses the save and shows a message naming the missing template path instead of writing the live file

### Requirement: Save reports what changed
After a successful config edit the editor SHALL report a precise diff summary (added,
removed or changed keys) so the operator sees exactly what was modified.

#### Scenario: Diff summary after add
- **WHEN** the user saves a new project
- **THEN** the log shows `projects.<name>` as added along with its path, and no other keys as changed