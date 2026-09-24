# Spec Delta

## Purpose

Полноценная навигация по тексту в модальных формах TUI: перемещение курсора,
редактирование в середине строки и понятное отображение позиции ввода.

## ADDED Requirements

### Requirement: Cursor navigation inside form fields
Form text fields SHALL support horizontal cursor movement: Left/Right move the cursor by
one character, Home/End jump to the start/end of the value. Entered characters insert at
the cursor, not only at the end. The cursor column MUST stay clamped to the value length.

#### Scenario: Inserting in the middle of a value
- **WHEN** the field value is "main" and the user moves the cursor to the third column and types "c"
- **THEN** the value becomes "macin" and the cursor sits after the inserted character

#### Scenario: Cursor never exceeds the value
- **WHEN** the user presses End then Right repeatedly
- **THEN** the cursor stays at the last character position

### Requirement: In-place deletion
Backspace SHALL delete the character before the cursor and Delete the character under the
cursor. With an empty field, both keys MUST be inert. Ctrl-U SHALL clear the whole field.

#### Scenario: Backspace mid-field
- **WHEN** the value is "main" with the cursor after the second column and the user presses Backspace
- **THEN** the value becomes "min" and the cursor moves to the second column

#### Scenario: Ctrl-U clears the field
- **WHEN** the value is "feature/experiment" and the user presses Ctrl-U
- **THEN** the field is empty and the cursor is at column zero

### Requirement: Visible input position
The form SHALL render a visible caret at the cursor and an "insert at column N" hint when
the field is focused, so the operator always knows where the next keystroke lands.

#### Scenario: Caret and column hint shown
- **WHEN** a text field has focus and the cursor is at column 3
- **THEN** the field is rendered with a distinct caret at the 4th character slot and the hint shows "insert at column 3"

### Requirement: Selection follows focus across fields
Arrow Up/Down SHALL move focus between form fields; a focused field MUST be visually
distinct (border/color), and only the focused field receives typing keys.

#### Scenario: Tab between fields
- **WHEN** a project form has three fields and the user presses Down twice
- **THEN** focus moves from the first to the second to the third field, and typed characters go into the third field only