# Tasks

## 1. Deploy: протокол и хаб

- [ ] 1.1 Добавить `DeployRequest`/`DeployResponse` в протокол, поднять `PROTOCOL_VERSION` до 3. Проверка: тест сериализации и тест «старые конверты v2 десериализуются».
- [ ] 1.2 Реализовать обработчик deploy в хабе: allowlist машин/проектов, проверка online, force-семантика, карта активных `(machine,project)`. Проверка: тесты force/offline/дубль/чужой токен.
- [ ] 1.3 Добавить CLI `dsync deploy --machine [--project] [--force]` с валидацией локального конфига и понятными ошибками. Проверка: unit-тесты аргументов + e2e-тест против локального хаба.
- [ ] 1.4 Писать аудит deploy в лог хаба (кто, цель, проект, force, длительность, результат). Проверка: тест лог-строки, grep по отсутствию токена.
- [ ] 1.5 Обновить README/CHANGELOG по новой команде и proto 3. Проверка: `dsync deploy --help` соответствует документации.

## 2. Каркас веб-приложения

- [ ] 2.1 Создать `web/` (Next.js App Router, TS, Tailwind 4), `dev`/`start` на `127.0.0.1:3132`, pnpm. Проверка: `pnpm build` и `pnpm dev` открывают пустую страницу.
- [ ] 2.2 Инициализировать shadcn (`base-nova`, neutral, lucide) и добавить компоненты: `sidebar, card, table, badge, button, dropdown-menu, sheet, alert-dialog, dialog, tabs, input, select, separator, skeleton, tooltip, sonner, scroll-area, alert, empty, spinner`. Проверка: `components.json` валиден, `pnpm build` зелёный, shadcn audit checklist выполнен.
- [ ] 2.3 Настроить lint/typecheck/Vitest и скрипты `lint`, `typecheck`, `test`. Проверка: `pnpm lint && pnpm typecheck && pnpm test` зелёные.
- [ ] 2.4 Каркас layout: sidebar-навигация, header, тёмная/светлая тема, русские подписи, адаптивная сетка. Проверка: Playwright открывает все разделы, консоль без ошибок.

## 3. BFF: CLI, задачи, аудит, API

- [ ] 3.1 `lib/dsync.ts`: `execFile` без shell, `DSYNC_BIN`, PATH с `~/.local/bin`, таймауты 60/300 с, типизированные парсеры `status`/`doctor`/`state list`, фильтрация секретов. Проверка: unit-тесты парсеров и фильтра на фикстурах.
- [ ] 3.2 `lib/allowlist.ts`: разрешённые действия и проверка machine/project/state-key по свежим данным. Проверка: тесты «неизвестное действие», «чужая машина», «кривой ключ» → 400.
- [ ] 3.3 `lib/jobs.ts`: очередь, один worker, конфликт на цель, TTL, ring-buffer вывода. Проверка: тесты конфликта, таймаута, перехода `queued→running→ok/failed`.
- [ ] 3.4 `lib/audit.ts`: JSONL 0600, ротация 5 МБ, чтение последних N, без секретов. Проверка: тесты записи/чтения/ротации.
- [ ] 3.5 Route handlers: `GET /api/health|status|state|jobs|jobs/{id}|audit`, `POST /api/actions|doctor`; единый формат ошибок, `no-store`. Проверка: тесты route handlers с фиктивным `DSYNC_BIN` (200/400/409/500).
- [ ] 3.6 Проверка Origin/Host и Content-Type на POST, лимит тела 16 КБ, CSP/заголовки безопасности. Проверка: тесты «cross-origin POST → 403», «не-JSON → 415», «большое тело → 413».

## 4. Экраны: Dashboard и Machines

- [ ] 4.1 Dashboard: карточки online/offline, дрейф версий, упавшие pull'ы, dead-letter; баннеры «хаб недоступен» и «данные устарели»; время снимка. Проверка: Playwright на фикстурах «всё ок», «хаб недоступен», «есть проблемы».
- [ ] 4.2 Machines: таблица с сортировкой, sheet с деталями и исходами pull'ов, раскрытие длинных ошибок, disabled-действия для offline. Проверка: Playwright + тест сортировки/фильтра.
- [ ] 4.3 Действия на Dashboard/Machines: push, pull, doctor через job-модель, подтверждение и тосты результата. Проверка: Playwright «deploy/push создаёт задачу и показывает результат».

## 5. Экраны: Projects, State, Doctor, Activity, Settings

- [ ] 5.1 Projects: матрица машина×проект, фильтр «только проблемы», счётчик скрытых, пустое состояние, deploy с подтверждением и force-switch. Проверка: Playwright на фикстурах.
- [ ] 5.2 State: tabs tmux/opencode, dead-letter с причиной, retry одного/всех с подтверждением, подпись «локальный state машины панели». Проверка: Playwright + тест 400 на несуществующий ключ.
- [ ] 5.3 Doctor: запуск, индикатор, ok/warn/fail-бейджи, версии и время, сохранение прошлого результата до завершения нового. Проверка: Playwright на трёх фикстурах.
- [ ] 5.4 Activity: список задач и аудит, обновление по опросу, отметка «задача прервана» после рестарта. Проверка: тест с перезапуском процесса/фикстурой.
- [ ] 5.5 Settings: интервал 5–60 с в localStorage, тема, read-only хаб/наличие токена; несовместимость `schema_version` и proto < 3 → баннер и disabled deploy. Проверка: Playwright «старый хаб».

## 6. Тесты, CI и запуск

- [ ] 6.1 Настроить Playwright с фиктивным `DSYNC_BIN` (ok/ошибка/пусто) и smoke-сценарий по всем экранам. Проверка: `pnpm test:e2e` зелёный локально.
- [ ] 6.2 CI job `web`: pnpm install --frozen-lockfile, lint, typecheck, vitest, build. Проверка: job зелёный на push.
- [ ] 6.3 Проверка безопасности: bind loopback, отказ при не-loopback без токена, отсутствие секретов в ответах/аудите. Проверка: тесты + ручной curl.
- [ ] 6.4 `web/README.md`: запуск dev/prod, переменные (`DSYNC_BIN`, `DSYNC_WEB_TOKEN`, `PORT`), systemd-пример, ограничения. Проверка: команды из README выполняются на чистой машине.

## 7. Интеграция и выкат

- [ ] 7.1 Прогнать панель на ноутбуке против живого хаба: Dashboard, Machines, Projects, State, Doctor; проверить реальные ошибки/пустые состояния. Проверка: скриншоты и отсутствие ошибок в консоли/логах.
- [ ] 7.2 Проверить deploy на реальной машине (например, `awgq` на `mkair-server-tmn`) с force и без; убедиться в записи аудита в хабе и панели. Проверка: `dsync status` показывает свежий исход, лог хаба содержит запись.
- [ ] 7.3 Обновить `CHANGELOG.md` и `README.md` (раздел про панель), указать зависимость от `refactor-dsync-core`. Проверка: документация соответствует фактическому поведению.
- [ ] 7.4 Проверить откат: остановить панель, откатить `dsync` до proto 2, убедиться, что push/pull работают, панель показывает несовместимость. Проверка: `dsync status` и `doctor` зелёные после отката.

## Workflow follow-up

- Архивировать изменение после ревью и зелёного CI.
- Убедиться, что `openspec validate add-fleet-web-ui --strict` проходит.
