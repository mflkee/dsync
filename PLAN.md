# dsync — Plan & Progress

## Что сделано

### High Priority (все выполнены)
- **Desktop nodejs конфликт**: удалён конфликтующий `nodejs-lts-iron`, dsync repo сброшен
- **archlinux-server hub pull**: `find` вместо zsh glob (`$path/*/.git` → `find $path -maxdepth 2 -name .git`)
- **Опечатка `pinfo.get('achines')`**: исправлена на `pinfo.get('machines', [])`
- **File lock**: `fcntl.flock` в `lock.py`, предотвращает параллельные запуски dsync
- **safe_pull**: различает network errors vs реальные конфликты (проверяет stderr на `CONFLICT` + `git status --porcelain` для `UU`)
- **Retry**: `with_retry()` в `retry.py`, SSH операции используют `retries=3`
- **cmd_push exit code**: возвращает non-zero при ошибке remote sync
- **Логирование**: `setup_logging()` вызывается в `main()`, DEBUG в chezmoi.py/ssh_client.py, INFO в cli.py
- **Валидация конфига**: `Config.validate()` проверяет пустые host и отсутствие `[git] source`

### Medium Priority (выполнены)
- **Syncthing health**: модуль `syncthing.py` — проверка через `pgrep` + `syncthing cli`, auto-restart, auto-resolve конфликтов (архивирование `*sync-conflict-*` файлов)
- **CLI `dsync syncthing`**: подкоманды `status`, `resolve`, `restart`
- **Health check в sync flow**: после remote sync автоматически проверяет syncthing на каждой машине
- **UI**: прогресс `[3/6]`, dry-run вывод с `~`/`-` индикаторами, error summary в конце

### Тесты
- 39 тестов, все проходят
- ruff check/format чисто

## Machines

| Machine       | IP / Host                          | Status      | Notes                     |
|---------------|------------------------------------|-------------|---------------------------|
| server        | (это эта машина)                   | OK          | mkair-server              |
| desktop       | archlinux-desktop-12-158           | OK          | Syncthing OK              |
| server-tmn    | (NetBird)                          | OK          | Syncthing OK              |
| archlinux-server | (NetBird)                       | OK          | Syncthing не установлен   |
| notebook      | (NetBird)                          | Offline     | Недоступен                |
| antix1        | (NetBird)                          | Offline     | Недоступен                |

## Что дальше

### Архитектура
- [ ] Plugin/hook система:allows扩展ение dsync через внешние скрипты
- [ ] Health check для других сервисов (Obsidian, MCP, etc.)
- [ ] Dashboard / TUI для мониторинга всех машин

### Syncthing
- [ ] Автоматическая настройка shared folders через syncthing API
- [ ] Device pairing между машинами
- [ ] Мониторинг sync progress

### CLI
- [ ] `dsync machines` — управление списком машин
- [ ] `dsync logs` — просмотр логов
- [ ] `dsync doctor` — полная диагностика всех сервисов

### Hub
- [ ] Автоматический hub pull/push в sync flow
- [ ] Hub status в summary

## Конфигурация

### chezmoi.toml (на каждой машине)
```toml
sourceDir = "~/dotfiles"

[age]
  recipient = "age1yxduppvdkg0v4mksvyd7nlg0dj8ggctjm0aymdy6vrfq40j5caksq4nxg4"
```

### dsync config (`~/.config/dsync/config.toml`)
```toml
[git]
  source = "~/dotfiles"
  branch = "main"
  remote = "origin"
  remote_url = "https://github.com/mflkee/dotfiles.git"

[logging]
  level = "INFO"
```

## Conventions
- Conflict strategy: `ours` (local wins)
- Remote machines: auto-resolve via `git reset --hard origin/{branch}`
- SSH: key-based auth, `sshpass` fallback with password 7405
- Branch naming: `{machine}-dotfiles` for remote branches
