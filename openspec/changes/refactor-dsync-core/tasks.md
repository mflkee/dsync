# Tasks

## 1. Инфраструктурные модули

- [x] 1.1 Создать `src/infra/{atomic,time,retry,shell,fingerprint}.rs`, подключить в `lib.rs`; каждый модуль получает unit-тесты. Проверка: `cargo test infra::` зелёный.
- [x] 1.2 Перевести hub- и SSH-TOFU-сторы на общий `FingerprintStore` с параметрами пути/прав. Проверка: существующие тесты `trust`/`ssh::trust` проходят без правок ожиданий.
- [x] 1.3 Перевести `retry_pull` и клиентский connect на общий `infra::retry` с политикой transient. Проверка: тесты backoff/`tokio::time::pause` зелёные, число попыток не изменилось.
- [x] 1.4 Собрать все remote-команды через `infra::shell` (`quote`, `join_posix`), убрать `Path::join` из `build_pull_command`. Проверка: новые snapshot-тесты для путей с пробелом, кавычкой, кириллицей и Windows-хоста; `cargo test shell` зелёный.
- [x] 1.5 Добавить в `CHANGELOG.md` запись о внутренних переездах (без пользовательских изменений кроме JSON-вывода). Проверка: запись присутствует, `cargo fmt --check` проходит.

## 2. Hub state: индекс, payload, merge, статус

- [ ] 2.1 Вынести `state/{index,payload}.rs`, оставив `HubState` фасадом; файлы payload'ов и `index.json` пишутся только своими модулями. Проверка: тесты `hub::state` зелёные, grep по записи `index.json` — одно место.
- [ ] 2.2 Вынести `state/merge.rs` (LWW, seq, ретеншн) и `state/status.rs`; LWW тестируется на in-memory индексе без ФС. Проверка: тесты LWW/статуса зелёные, новые тесты без tempdir.
- [ ] 2.3 Перенести тестовый модуль `hub/state.rs` в `hub/state/tests.rs` (или соседний `tests`-модуль), не меняя количество проверок. Проверка: `cargo test --all-targets` показывает то же число тестов, «боевой» файл < 700 строк.

## 3. Hub server: serve, dispatch, pull

- [ ] 3.1 Вынести accept-loop и диспетчер запросов в `hub/{serve,dispatch}.rs`; envelope-ошибки не меняются. Проверка: тесты конкуренции/лимитов и `serve_loop` зелёные.
- [ ] 3.2 Вынести pull-planner/executor в `hub/pull/{mod,command,retry}.rs`; `pull_skip_reason` и порядок «started/unchanged/offline» сохраняются. Проверка: тесты pull-plan и snapshot remote-команды зелёные.
- [ ] 3.3 Ввести `HubError` на границах и заменить `Result<_, String>` там, где это не wire-формат; строкой остаётся только `PullOutcome.error`. Проверка: `cargo clippy --all-targets -- -D warnings`, тесты протокола/пуллов зелёные.

## 4. Client state: каналы и оркестрация

- [ ] 4.1 Вынести локальный индекс (`seq`, failures, dead-letter) в `client/state/index.rs`. Проверка: тесты retry/dead-letter/`prune_unwanted_index` зелёные.
- [ ] 4.2 Ввести trait `StateChannel` и разнести `tmux`/`opencode` по модулям; оркестрация тестируется с фейковым каналом. Проверка: `cargo test client::state` зелёный, есть тест оркестрации на фейке.
- [ ] 4.3 Вынести батчи/пагинацию/применение в `client/state/mod.rs`; поведение «уже применённое не применяем» сохранено. Проверка: тесты `apply_skips_already_applied_items`, `foreign_tmux_snapshot_is_never_applied` зелёные.

## 5. TUI: экраны и редактор конфига

- [ ] 5.1 Разложить `tui/ui.rs` по экранам/виджетам; тексты и горячие клавиши не меняются. Проверка: unit-тесты раскладки/статусов зелёные; ручной smoke — Dashboard, Machines, Projects, State, Help открываются.
- [ ] 5.2 Разложить `tui/cfg.rs` (формы, патчи `toml_edit`); round-trip конфига сохраняет комментарии. Проверка: тесты round-trip и сохранения через chezmoi-шаблон зелёные.

## 6. Один lib-крейт вместо lib+bin копии

- [x] 6.1 Перенести все `mod`-объявления в `lib.rs`, `main.rs` использует `dsync::…`; видимость по умолчанию `pub(crate)`. Проверка: `cargo build` и `cargo test --all-targets` зелёные, в `cargo build -v` каждый модуль компилируется один раз.
- [x] 6.2 Добавить `cargo test --doc` в CI и локальную проверку. Проверка: job зелёный, doc-тесты (если есть) проходят.

## 7. JSON-диагностика

- [x] 7.1 Переключить `tracing` в не-TUI режиме на stderr; TUI по-прежнему пишет в `~/.local/share/dsync/tui.log`. Проверка: `dsync status --json 2>/dev/null | jq .` парсится, логи видны только в stderr.
- [x] 7.2 Добавить `schema_version` и `local_state` в `status --json` через `diagnostics::StatusReport`; снапшот-тест имён/типов полей. Проверка: тест сериализации + `jq -e '.schema_version'`.
- [x] 7.3 Реализовать `doctor --json` (`Check { id,label,status,detail,error }`, коды возврата 0/≠0) по сценариям спецификации. Проверка: тесты «всё ок», «хаб недоступен», «warn без fail».
- [x] 7.4 Реализовать `state list --json` с сортировкой и запретом payload'ов/секретов. Проверка: тесты пустого индекса, dead-letter и отсутствия `data`/токенов.
- [x] 7.5 Описать контракты в `README.md` (раздел «Machine-readable output») с примерами `jq`. Проверка: команды из документации выполняются как написано.

## 8. Интеграционные тесты и CI

- [ ] 8.1 Добавить `tests/quic_e2e.rs`: локальный хаб на `127.0.0.1:0` + клиент, push → pull-plan → state pull. Проверка: `cargo test --test quic_e2e` зелёный на трёх ОС.
- [ ] 8.2 Добавить тест совместимости v2: заранее снятые JSON-конверты старого клиента принимаются новым хабом и наоборот. Проверка: тест в `tests/protocol_compat.rs`.
- [ ] 8.3 Проверить полный CI (ubuntu/macOS/Windows) и убрать `allow(unreachable_code)` из `bot.rs`, если спайк teloxide прошёл. Проверка: все три job'а зелёные, `grep -r allow(unreachable_code) src` пуст или содержит только обоснованный случай.

## 9. Зависимости

- [ ] 9.1 Убрать `dirs`, оставить `directories`; заменить все вызовы. Проверка: `cargo build`, `cargo tree -i dirs` пуст.
- [ ] 9.2 Тайм-бокс-спайк обновления `teloxide`; при успехе — обновить и убрать module-wide allow, при провале — feature `telegram` (off по умолчанию) + заметка в README. Проверка: `cargo test` и `cargo test --features telegram` (если включён) зелёные.
- [ ] 9.3 Спайк обновления `russh` после стабилизации `ssh/`; проверить TOFU, pull-команды и e2e. Проверка: тесты `ssh::*` и `quic_e2e` зелёные; при отказе — зафиксировать причину в README/CHANGELOG.
- [ ] 9.4 Прогнать `cargo tree -d` и убрать дубли, которые не тянутся транзитивно; результат записать в `CHANGELOG.md`. Проверка: список дублей сокращён или обоснован.

## 10. Выкат и проверка на флоте

- [ ] 10.1 Обновить `README.md`/`AGENTS.md`-раздел про state и диагностику, если поменялись команды или потоки логов. Проверка: документация соответствует `--help`.
- [ ] 10.2 Канареечный выкат: собрать на ноутбуке, дождаться `post_pull` на хабе, проверить `dsync status`/`doctor` и два цикла push/pull. Проверка: версия хаба/клиентов совпадает, в логе хаба нет ошибок, JSON-выводы валидны.
- [ ] 10.3 Проверить откат: revert коммита, `post_pull` пересобирает бинарь, состояние на диске читается обеими версиями. Проверка: `dsync status` работает после отката на ноутбуке (без выката на весь флот).

## Workflow follow-up

- Архивировать изменение после ревью и зелёного CI.
- Убедиться, что `openspec validate refactor-dsync-core --strict` проходит.
