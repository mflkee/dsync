# Design

## Context

См. `proposal.md` — Why. Вводные:

- Панель живёт в репозитории `dsync` (`web/`) и запускается на машине, где
  уже есть конфиг и токен dsync (ноутбук или archlinux-server). Это даёт
  доступ к хабу через NetBird без публикации нового API.
- Хаб общается по QUIC-протоколу v2; браузер его не понимает. Между
  браузером и CLI ставится Next.js BFF (route handlers, Node runtime).
- JSON-контракты `status`/`doctor`/`state list` появляются в изменении
  `refactor-dsync-core` (`machine-readable-diagnostics`); панель зависит от
  `schema_version = 1`.
- Локальный прецедент — `~/projects/awgq/web`: Next.js 16, React 19,
  Tailwind 4, shadcn (`base-nova`, neutral, lucide), pnpm, BFF поверх CLI,
  bind `127.0.0.1`. Повторяем стек, чтобы не плодить зоопарк.

## Goals / Non-Goals

**Goals:**

- Один экран для флота: статус, машины, проекты, state, диагностика.
- Безопасные действия в один клик с аудитом и защитой от дублей.
- Минимум новой серверной логики: BFF — тонкий адаптер над CLI.

**Non-Goals:**

- Новый HTTP API в хабе (в этом изменении добавляется только `deploy` по
  существующему QUIC-каналу).
- Многопользовательская авторизация, роли, SSO, доступ из интернета.
- Редактирование конфигов, перезапуск сервисов, произвольные команды.
- Потоковые логи/WebSocket; real-time push-уведомления.

## Decisions

### 1. Стек и структура приложения

`web/`: Next.js (App Router) + React + TypeScript + Tailwind + shadcn/ui,
пакетный менеджер pnpm, `dev`/`start` на `127.0.0.1:3132` (3131 занят
awgq). Структура:

```
web/
  src/app/            # layout, страницы, /api route handlers
  src/components/     # компоненты экранов + shadcn ui
  src/lib/dsync.ts    # execFile-обёртка над CLI, парсеры JSON
  src/lib/jobs.ts     # очередь задач, статусы, TTL
  src/lib/audit.ts    # JSONL-аудит
  src/lib/allowlist.ts# какие действия/аргументы разрешены
```

_Альтернатива_: отдельный репозиторий — отвергнута: панель тесно связана с
версией CLI и JSON-схемой, удобнее версионировать вместе.

### 2. BFF строго через `execFile`, без shell

`lib/dsync.ts` вызывает `execFile("dsync", args, { env, timeout, maxBuffer })`.
`shell: true` запрещён; никакой конкатенации команд. Аргументы формируются
только из allowlist и проверенных значений:

- `machine` — сверяется с именами из свежего `status --json`;
- `project` — сверяется с именами проектов из `status --json`;
- `state key` — регулярка `^[a-z]+:[A-Za-z0-9._-]+$` и наличие в
  `state list --json`.

Переменная `DSYNC_BIN` позволяет подменить бинарь в тестах (фикстура), по
умолчанию `dsync` из `PATH` с добавленным `~/.local/bin`.

### 3. Модель задач вместо синхронных запросов

Любое действие создаёт `Job` в памяти процесса:

```ts
type Job = {
  id: string;            // uuid
  action: "push"|"pull"|"deploy"|"state-retry"|"doctor";
  target: { machine?: string; project?: string; key?: string; force?: boolean };
  state: "queued"|"running"|"ok"|"failed";
  code: number|null;
  startedAt: number; finishedAt?: number;
  output: string[];      // ring buffer, последние ~200 строк, без секретов
  error?: string;
};
```

- один worker (последовательные действия) + лимит «одна задача на цель»;
  повтор возвращает `409` с `jobId` текущей задачи;
- TTL завершённых задач — 1 час, активных — без TTL;
- API: `POST /api/actions` → `{jobId}`, `GET /api/jobs/{id}` → `Job`,
  `GET /api/jobs` → последние 20;
- потеря задач при рестарте панели допустима (это UI-состояние); аудит
  сохраняется отдельно.

_Почему не SSE/WebSocket_: одно действие длится десятки секунд, опрос раз в
секунду проще, переживает перезагрузку страницы и не требует прокси.

### 4. Аудит — append-only JSONL

`~/.local/share/dsync/web/audit.jsonl`, права 0600, запись
`{ts, action, target, result, code, error?}`. Никаких токенов и payload'ов.
Ротация: при >5 МБ файл переименовывается в `audit.jsonl.1` (одна копия).
Чтение — последние 200 записей для экрана Activity.

### 5. API-контракты

| Метод | Путь | Назначение |
|-------|------|------------|
| GET | `/api/health` | живость панели, версия, без CLI |
| GET | `/api/status` | `dsync status --json` + `updatedAt` |
| GET | `/api/state` | `dsync state list --json` + `updatedAt` |
| GET | `/api/jobs`, `/api/jobs/{id}` | состояние задач |
| POST | `/api/actions` | создание задачи |
| GET | `/api/audit?limit=` | журнал действий |
| POST | `/api/doctor` | задача doctor (тот же job-механизм) |

Все ответы — JSON; ошибки — `{error: {code, message, detail?}}` с
человекочитаемым русским текстом. `Cache-Control: no-store`,
`runtime = "nodejs"`, `dynamic = "force-dynamic"`.

### 6. UI: экраны и shadcn-компоненты

Каркас — sidebar + header (block `dashboard-01`/`sidebar-07` как референс,
не как жёсткая зависимость). Компоненты из `@shadcn`:
`sidebar`, `card`, `table`, `badge`, `button`, `dropdown-menu`, `sheet`,
`alert-dialog`, `dialog`, `tabs`, `input`, `select`, `separator`, `skeleton`,
`tooltip`, `sonner`, `scroll-area`, `alert`, `empty`, `spinner`.

Экраны:

- **Dashboard**: карточки «машин online/offline», «дрейф версий», «упавшие
  pull'ы», «dead-letter»; баннеры недоступности/устаревания; быстрые
  действия.
- **Machines**: таблица (машина, online, last push, версия, проблемы) +
  `sheet` с деталями и действиями. Офлайн-действия disabled с tooltip.
- **Projects**: таблица «машина × проект», фильтр «только проблемы»,
  счётчик скрытых, действие deploy с подтверждением.
- **State**: `tabs` tmux/opencode, таблица dead-letter, retry с
  `alert-dialog`; подпись «локальный state машины панели».
- **Doctor**: таблица проверок с `badge` ok/warn/fail, запуск, время.
- **Activity**: список задач + аудит, обновление по опросу.
- **Settings**: интервал обновления (select), тема (next-themes), read-only
  адрес хаба и наличие токена (без значения).

### 7. Обновление данных

SWR (лёгкий, умеет `refreshInterval`, `revalidateOnFocus`, дедупликацию).
Интервал по умолчанию 10 с, настраивается 5–60 с и хранится в
`localStorage`. При `document.hidden` опрос останавливается; при возврате —
немедленный `mutate`. `updatedAt` каждого снимка показывается; старше
3×интервала — бейдж «устарело».

_Альтернатива_: TanStack Query — мощнее, но избыточен для 3 read-эндпоинтов.

### 8. Безопасность

- bind `127.0.0.1` по умолчанию; при не-loopback адресе — обязательный
  `DSYNC_WEB_TOKEN` (Bearer) и отказ стартовать без него;
- mutating-запросы: проверка `Origin`/`Host` (same-origin), требование
  `Content-Type: application/json`, запрет CORS;
- CSP без `unsafe-eval`, `frame-ancestors 'none'`; `X-Content-Type-Options`;
- никакого `dangerouslySetInnerHTML`; вывод CLI — текст, не HTML;
- лимит тела запроса 16 КБ, таймауты CLI: 60 с (read/actions), 300 с
  (deploy/doctor);
- allowlist действий и аргументов; неизвестное — 400.

### 9. Новый deploy: протокол и хаб

- `PROTOCOL_VERSION` 2 → 3 (новый тип запроса, старые не меняются).
- `protocol::DeployRequest { machine, token, target_machine, project:
  Option<String>, force: bool }`, `DeployResponse { ok, results:
  Vec<DeployResult>, error }`.
- Хаб: тот же pull-planner, но с `force` (игнорирует deployed-commit и
  `pull_only_on_change`), проверкой online-порога и картой активных
  deploy'ев `(machine, project) → job_id`; ответ — после завершения SSH
  (клиентский таймаут ≥ `pull_timeout_secs`).
- Хаб логирует запрос (кто, куда, проект, force, длительность, результат).
- Панель определяет поддержку по `(proto N)` в `hub_version` из
  `status --json`; при N < 3 кнопка deploy disabled с подсказкой.
- `dsync deploy` печатает результат построчно; без `--json` (в v1) —
  панель использует exit code + stdout; при желании позже добавим `--json`.

_Альтернатива_: переиспользовать `push --target`, но он не умеет
per-project и force, и смешивает семантику push и deploy.

### 10. Тесты

- Unit (Vitest): парсеры JSON, allowlist, job-очередь (конфликт/таймаут),
  формат аудита.
- Route handlers с `DSYNC_BIN` → shell-фикстура, отдающая заранее снятые
  JSON-документы; проверка кодов 200/400/409/500.
- Playwright smoke: Dashboard → Machines → deploy (фикстура) → Activity;
  пустые состояния и «хаб недоступен» (фикстура с exit 1).
- CI: отдельный job `web` (pnpm install --frozen-lockfile, typecheck, lint,
  vitest, build). Playwright — локально/по расписанию, не в матрице ОС.

### 11. Запуск

`web/README.md`: `pnpm install`, `pnpm dev`; прод — `pnpm build && pnpm
start`. Опционально — systemd user unit `dsync-web.service`
(`WorkingDirectory=~/projects/dsync/web`, `ExecStart=pnpm start`,
`Environment=PORT=3132`), но в этом изменении не включается автоматически.
Приложение не добавляется в `[projects.*]`-автозапуск и не стартует на
каждой машине.

## Risks / Trade-offs

- **CLI-вывод меняется без предупреждения** → зависимость от
  `schema_version`; неподдерживаемая версия — баннер, а не частичный разбор.
- **Долгий `post_pull` (сборка) упирается в таймаут** → задачи + опрос;
  таймаут 300 с, состояние `running` видно.
- **Потеря задач при рестарте панели** → это UI-состояние; аудит на диске,
  после рестарта показываем «задача прервана».
- **Секреты в stderr CLI** → BFF фильтрует строки с `token`, `password`,
  `PRIVATE KEY` перед показом; полный вывод не пишется в аудит.
- **Next.js 16/React 19 — свежие версии** → фиксируем точные версии и
  pnpm-lock; сборка в CI.
- **Локальный bind не защищает от других пользователей машины** → панель
  рассчитана на однопользовательскую машину; при необходимости —
  `DSYNC_WEB_TOKEN` и не-loopback bind.
- **Дублирование логики между CLI и JSON-парсером UI** → BFF не
  интерпретирует бизнес-состояния, только раскладывает документ по экранам.

## Migration Plan

1. Сначала выкатить `refactor-dsync-core` (JSON-контракты, schema_version).
2. Затем `dsync deploy` + proto 3: собрать, обновить хаб (post_pull),
   проверить, что клиенты v2 продолжают push/pull.
3. Панель запускается вручную на ноутбуке; проверка сценариев из spec.
4. Rollback: остановить панель; откат `dsync`-коммита возвращает proto 2
   (deploy-команда исчезает); панель показывает ошибку совместимости.
