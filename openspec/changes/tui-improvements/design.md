# Design

## Context

TUI-модуль — `src/tui/` (app.rs / backend.rs / ui.rs / cfg.rs / mod.rs, ratatui 0.30 +
crossterm 0.29), конфиг — `src/config.rs` (serde, `toml`), протокол — `src/protocol.rs`
(QUIC/quinn, запрос/ответ по типу месседжа). Полный мотивацию см. proposal.md - Why;
поведенческие требования — в specs/ (4 delta).

Ключевые текущие ограничения, из которых исходим:

- `ConfigEditor::save()` (`src/tui/cfg.rs`) десериализует `Config`, мутирует, пишет
  `toml::to_string_pretty(...)` целиком → теряются комментарии, порядок секций и любые
  ключи не из схемы `Config`.
- Пуши правок через hub не происходит: правка живёт локально до `dsync push`; для
  chezmoi-конфига живой файл — рендер шаблона, и TUI-правка умрёт на следующем
  `chezmoi apply`.
- State sync (`[state]`, `Src/client/state.rs`) полностью невидим в TUI.
- Формы (`Form` в app.rs) — однострочный append-only ввод.
- Pull-рекорды: `MachineStatus.pulls` содержит записи `PullRecord { ok, attempts,
  finished, error? }` — ошибка уже есть в протоколе, TUI её не показывает детально.

## Goals / Non-Goals

**Goals**

- Правки конфига из TUI никогда не теряют данные файла (комментарии/секции/chezmoi).
- Вкладка State: конфиг + здоровье state-sync с хаба.
- Полноценный ввод в формах (курсор, Home/End, Backspace/Delete, Ctrl-U).
- Детальные failed pulls + mouse-скролл с корректным restore терминала.

**Non-Goals**

- Не редактируем `[hub.tokens]` секреты из TUI (sidecar `tokens.toml`) — только индикация.
- Без полного protocol-редизайна: `state_status` — аддитивный запрос, старые хабы
  отвечают ошибкой → TUI показывает "unavailable".
- Без animated progress per-file для push/pull (спиннер остаётся).

## Decisions

### D1. Сохранение конфига через `toml_edit`-патчи, а не serde-rewrite
Вместо «десериализовать → мутировать → переписать целиком»: читаем файл в
`toml_edit::DocumentMut`, вносим точечные правки (insert/remove ключей `projects.<name>`
и т.п.), пишем обратно. Комментарии/порядок/неизвестные секции сохраняются сами.
- Альтернативы: (а) `#[serde(flatten)] extra: toml::Value` + перепись — сохраняет
  неизвестные ключи, но **не** комментарии; (б) полный round-trip через toml_edit
  document → serde — сложнее, чем точечные патчи. Выбран патч-подход: минимальный риск.
- Добавляем `toml_edit` (уже зависит от `toml` — версии совместимы) в Cargo.toml.

### D2. Chezmoi-флоу: править шаблон, а не живой файл
При `chezmoi_managed`: ищем шаблон через `chezmoi source-path` + относительный путь
(`.tmpl`-суффикс). Если найден — пишем патч в шаблон, зовём `chezmoi apply <target>`
(в `Cmd::ApplyChezmoi`, backend-поток), показываем подтверждение пользователю ДО записи.
Если шаблон не найден / chezmoi не установлен — отказ с сообщением (см. спека
config-editor-safety).
- Альтернатива: писать живой файл с предупреждением — отвергнута: тихо теряется при
  следующем apply.

### D3. `state_status` — аддитивный запрос в протокол
Новый variant `Request::StateStatus` / `Response::StateStatus { channels: [...] }`:
хаб отдаёт по каждому каналу (`tmux`, `opencode`) последний `updated`, кол-во items и
последнюю ошибку валидации/импорта (хаб уже хранит `StateItem` в `~/.local/share/
dsync-hub/state/`). Клиент: `Cmd::StateStatus` из backend, ответ → `Event::StateStatus`.
Старый хаб: decode-ошибка/нет обработчика → TUI показывает "unavailable".
- Обновление: повторный запрос при открытии вкладки + после каждого push/pull.

### D4. Формы: курсор в value + фокус по полям
`FormField` (app.rs) получает `cursor: usize`. Обработка в `handle_key` при открытой
форме: Left/Right/Home/End/Backspace/Delete/Ctrl-U; Up/Down (и Tab) переключают
активное поле (`focus: usize`). Рендер: каретка в value (инвертированный символ или
`▏`), hint "insert at column N". Это чисто UI-слой, backend-команды не меняются.

### D5. Mouse capture + детали failed pulls
- `ratatui::init()` → ручной `crossterm::enable_raw_mode` + `EnableMouseCapture` +
  `AlternateScreen`; все exit-пути — через единый `restore()` с `DisableMouseCapture`.
- Pull-детали: на Dashboard/Machines — новый режим (клавиша, напр. `e` на выбранной
  машине / `d` — детали), рисующий список `machine × project × error` из
  `MachineStatus.pulls` (error поле уже есть в `PullRecord` — только рендер).
- Wheel-события: `TermEvent::Mouse` → app.scroll (логи/списки), игнорируются при
  открытой форме.

## Risks / Trade-offs

- [toml_edit-патч по ключу `projects.<name>` не найден (проект удалён с сервера при
  переустановке/правках вне TUI)] → при отсутствии ключа: явная ошибка «key not found»,
  без молчаливой вставки неправильного места.
- [chezmoi apply --force может перезаписать другие live-файлы? нет — `chezmoi apply
  <target>` точечный] → применяем только конкретный `<target>`, не весь профиль.
- [state_status на старом хабе ломает decode, backend может зациклиться] → таймаут +
  терминирующая ошибка → Event::StateStatus{error} → "unavailable", канал не блокируем
  (токен отмены как у Poll).
- [Mouse capture на не поддерживающих терминалах] → graceful: если `EnableMouseCapture`
  вернул ошибку — продолжаем без мыши (клавиатурный скролл остаётся).
- [Форма с курсором: бэкспейс на кириллице рвёт UTF-8 при char-границах] →
  оперируем `char_indices`, режем по границам char, не байтам.

## Migration Plan

- Deploy: обычный `cargo build --release` + развоз бинаря через dsync post_pull
  (как заведено). `state_status` обратно совместим: старый клиент ↔ новый хаб и
  наоборот не ломается (неизвестный variant → ошибка → fallback).
- Rollback: пересборка предыдущего тега; конфиг-правки, сделанные новым редактором,
  остаются валидным TOML (совместимы со старым бинарём — файл читается обычным serde).

## Open Questions

- Нужна ли клавиша «применить и сразу push» после правки конфига (сейчас правка
  локальная, hub узнаёт на следующем `dsync push`)? — не влияет на спеки; решается при
  реализации UX.
- Показывать ли в State-табе сводку по сессиям opencode (кол-во экспортированных за
  последний цикл)? — данные уже в state-индексе, но точный источник (хаб vs локальный
  `~/.local/share/dsync/state.json`) можно выбрать при реализации.