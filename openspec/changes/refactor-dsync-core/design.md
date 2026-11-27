# Design

## Context

См. `proposal.md` — Why. Технические вводные, которые определяют подход:

- Один Rust-крейт, edition 2021, MSRV 1.80; `src/main.rs` объявляет модули
  напрямую, а `src/lib.rs` — их подмножество (`cli`, `config`, `projects`,
  `protocol`, `ssh`). Из-за этого код компилируется дважды, а интеграционные
  тесты видят только «библиотечную» половину.
- Транспорт — QUIC (`quinn`) + собственный JSON-envelope с `protocol v2`;
  SSH-пуллы — `russh`; TUI — `ratatui`/`crossterm`; конфиг — TOML +
  `toml_edit` для безопасных правок.
- Хаб обновляется сам (`post_pull` → `cargo build` → отложенный
  `systemctl --user restart dsync-hub`), клиенты — тем же `post_pull`.
  Значит, любой рефакторинг уезжает на флот в следующем 15-минутном цикле.
- Тесты: 133 (unit + 3 интеграционных). CI — матрица ubuntu/macOS/Windows,
  `fmt` + `clippy -D warnings` + `test --all-targets` + release build.
- Логи `tracing` в обычном (не TUI) режиме сейчас пишутся в **stdout** —
  это надо исправить до появления `--json` (см. решение 4).

## Goals / Non-Goals

**Goals:**

- Снизить связность: один файл — одна причина для изменения; «боевые» файлы
  без сотен строк тестов.
- Сделать границы (hub, client, transport, storage, presentation) явными и
  тестируемыми в изоляции.
- Убрать дублирование, из-за которого одинаковые баги чинятся дважды.
- Дать стабильные JSON-контракты для панели и скриптов.

**Non-Goals:**

- Не менять wire-протокол v2 и его версию.
- Не добавлять HTTP-API в хаб и не реализовывать панель (это
  `add-fleet-web-ui`).
- Не менять формат конфига, набор команд и текстовый вывод по умолчанию.
- Не редизайнить TUI и не менять раскладку экранов.
- Не переходить на workspace из нескольких крейтов в этом изменении (см.
  решение 1, альтернатива отклонена).

## Decisions

### 1. Strangler-рефакторинг по модулям, а не переписывание

Двигаем код небольшими шагами, после каждого — `fmt`/`clippy`/тесты. Файлы
создаются с новыми границами, старые функции переносятся без изменения
логики; затем удаляются «пустые» обёртки.

Целевая структура (ориентир, не догма):

```
src/lib.rs                 // единственная точка сборки модулей
src/main.rs                // только CLI-dispatch и print_lines
src/infra/
  atomic.rs                // write-temp+rename с правами
  time.rs                  // unix_now/now_secs/format
  retry.rs                 // generic retry + transient-политика
  shell.rs                 // POSIX quoting, join_posix
  fingerprint.rs           // generic TOFU-стор
src/hub/
  mod.rs  serve.rs  dispatch.rs
  state/{mod,index,payload,merge,status}.rs
  pull/{mod,command,retry}.rs
src/client/
  state/{mod,index,tmux,opencode,apply}.rs
src/diagnostics.rs         // StatusReport/DoctorReport/StateReport + schema_version
```

Дополнительно: **единый lib-крейт**. `main.rs` перестаёт объявлять `mod`,
всё живёт в `lib.rs`, `main` использует `dsync::…`. Это убирает двойную
компиляцию и даёт интеграционным тестам доступ к hub/client. Приватность
компенсируем `pub(crate)` по умолчанию и минимальным списком `pub`.

_Альтернатива_: workspace `dsync-core`/`dsync-cli`/`dsync-hub` — чище по
границам, но ломает `cargo install`, добавляет версионирование крейтов и
замедляет текущий цикл. Отложено до появления второго потребителя (панель
с BFF его не требует).

### 2. Границы hub-состояния: фасад + три ответственности

`HubState` остаётся фасадом (тот же публичный интерфейс), внутри:

- `index` — загрузка/сохранение `state/index.json`, сквозной `seq`;
- `payload` — файлы payload'ов, `prune_*`, очистка tmp;
- `merge` — LWW, ключи `(channel,key)`, ретеншн;
- `status` — read-only сводка для `state_status`.

Инвариант: ни один модуль, кроме `index`, не пишет `index.json`; ни один,
кроме `payload`, не трогает файлы payload'ов. Это позволяет тестировать
LWW без файловой системы (in-memory index) и хранилище без сети.

### 3. Клиентский state — по каналам

`client/state.rs` делится на `index` (seq, failures, dead-letter),
`tmux` (collect/apply/symlink), `opencode` (export/import/list) и `mod`
(оркестрация `sync_state`, батчи). Каналы получают общий trait:

```rust
trait StateChannel {
    fn collect(&self, idx: &Index) -> Vec<StateItem>;
    fn apply(&self, item: &StateItem) -> Result<String>;
    fn wanted(&self) -> bool;
}
```

_Почему trait, а не свободные функции_: добавляет явную точку расширения и
позволяет тестировать оркестрацию с фейковым каналом; церемония минимальна
(два канала).

### 4. JSON-контракты и стерильный stdout

- Все `--json`-команды пишут документ в stdout, логи — в stderr:
  `tracing_subscriber::fmt().with_writer(std::io::stderr)` в не-TUI режиме.
  Это же чинит нынешнее смешивание info-логов с текстовым `status`.
- Контракты — отдельные serde-структуры в `src/diagnostics.rs` с
  `schema_version: u32 = 1`; не переиспользуем protocol-структуры напрямую,
  чтобы wire-формат и публичный JSON могли меняться независимо.
- Сериализация — `serde_json::to_writer_pretty` в залоченный stdout; при
  ошибке (EPIPE) выходим штатно, без паники.
- Подпись: `DoctorReport { schema_version, client_version, hub_version,
  checks: Vec<Check> }`, `Check { id, label, status, detail, error }`.
  `StatusReport` = существующий `StatusResponse` + `schema_version` +
  `local_state`.
- Snapshot-тесты сериализации (serde_json::Value), чтобы случайно не
  переименовать поле.

### 5. Общий TOFU-стор

Оба хранилища (hub-сертификат `known_hosts.toml`, SSH host-key
`ssh_known_hosts.toml`) сводятся к
`FingerprintStore { path, mode, map: BTreeMap<String,String> }` с
load/save/get/insert/remove/iter. Различия — только путь и права файла
(0600/0644), они параметры конструктора.

### 6. Retry/backoff как политика, а не копипаста

Один `infra::retry::run(policy, attempts, op)`:
`Policy { base_delay, max_delay, is_transient }`. Hub-пуллы используют
transient-классификацию (`No route to host`, `Connection refused`,
`Permission denied` — не ретраить), клиентский connect — свой набор
таймаутов, но тот же алгоритм. Тесты с `tokio::time::pause()` переносятся
вместе с функцией.

### 7. POSIX-shell команды — один модуль

`infra::shell::{quote, join_posix}`. Правило: любые строки, исполняемые на
удалённой машине, собираются только здесь; `Path::join` для remote-команд
запрещён (именно это дало `...\.git` на Windows-CI). Snapshot-тесты:
путь с пробелом/кавычкой/кириллицей, пустой url, post_pull.

### 8. Ошибки: типизированные на границах, anyhow — в CLI

- `thiserror`-энумы: `HubError` (config/io/protocol/ssh), `StateError`
  (index/payload/serialize), `ClientError` (connect/transport).
- Наружу (в лог/JSON-ответ) — стабильный `Display`; внутри — `source()`.
- `anyhow` остаётся в `main`, `init`, `tui`, `doctor` (сборка отчёта).
- Правило ревью: в путях демона нет `unwrap`/`expect`/`panic!` на
  достижимых ошибках; `expect` допустим только для инварианта с сообщением.
- `Result<_, String>` на границах hub заменяется на `HubError`, строкой
  становится только запись в `PullOutcome.error`.

### 9. Тесты и CI

- Тестовые модули больше ~200 строк выносятся в
  `src/<module>/tests.rs` (тот же crate, `#[cfg(test)] mod tests;`).
- `tests/quic_e2e.rs`: локальный хаб на `127.0.0.1:0` с самоподписанным
  сертификатом, клиент push → pull-plan → state pull, проверка
  совместимости v2.
- `tests/shell_commands.rs`: snapshot'ы remote-команд.
- `cargo clippy --all-targets -- -D warnings` остаётся блокирующим; матрица
  ОС сохраняется.
- Добавить `cargo test --doc` (после унификации lib).

### 10. Зависимости

- Убрать `dirs` в пользу `directories` (сейчас используются оба).
- `chrono` оставить: нужен локальный TZ для `fmt_civil`.
- `teloxide 0.12` — источник module-wide `allow(unreachable_code)` на новом
  rustc. Спайк-обновление (0.12 → актуальная) отдельной задачей; если API
  ломается сильнее, чем на полдня, `bot` уходит за feature `telegram`
  (по умолчанию выключен) и получает tracking-заметку. Удаление бота — не
  в этом изменении (пользовательский сценарий).
- `russh 0.44` — обновление отдельной задачей после стабилизации границ
  `ssh/`.
- `cargo tree -d` — разовая проверка дублей; `cargo deny` не вводим.

### 11. Порядок работ

1. Инфраструктурные модули + перенос хелперов (без смены вызовов).
2. `hub/state` → `state/*`; тесты.
3. `hub/server` → `serve`/`dispatch`/`pull/*`.
4. `client/state` → `state/*` с trait-каналами.
5. `tui/ui` и `tui/cfg` по экранам.
6. Унификация lib/bin, JSON-контракты, stderr-логи.
7. Интеграционные/снапшот-тесты, чистка зависимостей.

После каждого шага — зелёный CI; шаги независимы и могут быть отдельными
коммитами/PR.

## Risks / Trade-offs

- **Механические переносы ломают `#[cfg(test)]`-модули и приватность** →
  после каждого шага `cargo test --all-targets`; переносы без правок логики,
  сначала «переехало и компилируется», потом улучшения.
- **Двойная компиляция lib/bin** → унификация lib даёт один crate, но
  расширяет `pub`-поверхность; компенсируем `pub(crate)` и ревью.
- **JSON-схема закрепляет то, что ещё меняется** → `schema_version` + право
  добавлять поля; снапшот-тесты только на имена/типы, не на значения.
- **Смена stdout→stderr для логов** может сломать чей-то парсинг `dsync
  status 2>&1` → это осознанный bugfix (логи и раньше не были контрактом);
  отмечаем в CHANGELOG и проверяем, что TUI по-прежнему пишет в файл.
- **Обновление teloxide/russh разрастается** → тайм-бокс и feature-флаг;
  рефакторинг не блокируется.
- **Долгий проход по TUI** → TUI-шаг последний и может быть отложен без
  влияния на JSON/панель.

## Migration Plan

Рефакторинг не меняет протокол и формат конфига, поэтому миграции данных нет.
Раскатка: обычный `post_pull` → сборка клиента/хаба; хаб перезапускается
отложенным таймером. Проверка после выката — `dsync status`, `dsync doctor`,
один цикл push/pull, отсутствие ошибок в логе хаба. Rollback — revert
коммита(ов) и повторная сборка `post_pull`; состояние на диске совместимо
в обе стороны.
