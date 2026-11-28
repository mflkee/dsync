# dsync fleet panel

Локальная веб-панель над `dsync` CLI: машины, проекты, state, doctor и
безопасные действия (push/pull/deploy/retry). Браузер общается только с BFF,
секреты и `dsync`-токены в браузер не попадают.

## Запуск

```bash
cd web
pnpm install
pnpm dev          # http://127.0.0.1:3133
```

Прод:

```bash
pnpm build
pnpm start        # 127.0.0.1:3133
```

Порт 3133 выбран потому, что 3131/3132 заняты awgq-web. Приложение слушает
только loopback; для доступа из сети нужен `DSYNC_WEB_TOKEN` и обратный прокси
(не входит в текущую версию).

## Переменные

| Переменная | Значение |
|------------|----------|
| `DSYNC_BIN` | путь к CLI (по умолчанию `dsync` из `PATH`, в PATH добавляется `~/.local/bin`) |
| `DSYNC_WEB_TOKEN` | если задан, все API требуют `Authorization: Bearer <token>` |
| `PORT`/`-p` | порт Next.js (в скриптах — 3133) |

## API

`GET /api/health`, `/api/status`, `/api/state`, `/api/jobs`, `/api/jobs/{id}`,
`/api/audit`; `POST /api/actions` (`push|pull|deploy|state-retry`), `POST /api/doctor`.
Действия выполняются фоновыми задачами; повторный запрос на ту же цель
возвращает `409` с id текущей задачи.

## Ограничения

- `state retry` работает только для машины, где запущена панель;
- deploy и state-read требуют обновлённого хаба (proto 3) и `dsync deploy`;
- аудит хранится в `~/.local/share/dsync/web/audit.jsonl` (0600, ротация 5 МБ);
- тема фиксированно тёмная; редактирование конфига и перезапуск сервисов не
  входят в панель.
