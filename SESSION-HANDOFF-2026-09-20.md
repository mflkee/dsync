# SESSION HANDOFF — dsync (обновлено 2026-09-22)

> ⚠️ **Временный файл.** Прочитай в новой сессии opencode и **удали после использования**
> (`rm ~/projects/dsync/SESSION-HANDOFF-2026-09-20.md` + убери запись `SESSION-HANDOFF*.md` из `.gitignore`).
> В `.gitignore`, чтобы внутренняя инфраструктура не утекла в публичный GitHub и не попадала
> в автокоммиты «project sync: dsync».

---

## 0. Что это

dsync — Rust-инструмент синхронизации флота (dotfiles + projects + состояние) через
hub-coordinated QUIC (quinn) + TOFU, поверх git + SSH (russh). Репо публичный:
`git@github.com:mflkee/dsync.git`. Плюс Telegram-бот (`dsync bot`) и TUI (`dsync tui`).

**Правило AGENTS.md:** живые файлы и `~/dotfiles` правятся напрямую — dsync сам
захватывает изменения при push (chezmoi re-add, sha256-state). `chezmoi edit` не нужен.

## 1. Дорожная карта

- Phase 0 — очистка репо ✅
- Phase 1 — база: TOFU, config_version, `watch`, CI 3-OS ✅
- Phase 2 — `dsync init` мастер ✅
- **State sync — tmux + сессии opencode (через хаб, авто в push/pull) ✅ (2026-09-22)**
- Phase 4 — security/reliability (остаток) 🔜
- Phase 5 — crates.io + cargo-dist + AUR/homebrew 🔜
- Phase 6 — продвижение 🔜

## 2. Сделано (всё в origin/main)

### Phases 0–2
- TOFU hub auth, persistent hub cert, `machine.ssh_key`, `dsync hub`/`watch`, CI matrix,
  README/badges, `dsync init` (интерактивный мастер).
- Работа пользователя: `dsync capture` (авто-захват live-dotfiles), `dsync bot` (telegram).

### State sync (коммиты 493c44f…f268155, 2026-09-21/22)
- **Протокол**: `state_push`/`state_pull` + `StateItem {channel,key,updated,origin,seq,meta,data}`.
- **Хаб**: хранилище на диске (`~/.local/share/dsync-hub/state/`), LWW по `updated`,
  сквозной `seq`, выдача «новее моего seq, кроме своих».
- **Клиент** `src/client/state.rs`: collect/apply, локальный индекс
  `~/.local/share/dsync/state.json` (seq + что отправлено/применено).
  - `tmux`: снапшот tmux-resurrect (дедуп по хешу содержимого).
  - `opencode`: `session list --format json` → `session export --standalone` изменившихся →
    `session import` на других машинах.
- **Авто**: встроено в `push`/`pull`/`watch`; ошибки state не валят общий push.
- **Конфиг** `[state]` (в chezmoi-шаблоне, применён на всех машинах):
  ```toml
  [state]
  tmux = true
  tmux_restore = false   # true — resurrect restore в живой tmux
  [state.opencode]       # по умолчанию все [projects.*]
  ```

### Побочные фиксы (важные)
- **hub-пулл больше не теряет WIP**: `git stash push && git pull` → **`git pull --rebase --autostash`**
  (`src/hub/server.rs`). Раньше незакоммиченная работа молча уезжала в стеши — из-за этого
  агент на desktop выключал таймер.
- **opencode V2 `session export` недетерминированно обрезает вывод** (баг CLI) — решено
  `--standalone` + валидация JSON с ретраями.
- Эхо-петля импорт→пере-отправка — решена освежением индекса после импорта.
- QUIC idle-timeout при долгом сборе — сбор делается до подключения.
- Сбойные элементы не теряются (seq откатывается).

## 3. Состояние

- **git**: `main` = `f268155`, всё запушено; worktree/branch `feat/state-sync` удалены.
- **тесты**: 22+66+3 зелёные, clippy 0.
- **Задеплоено**: хаб на archlinux-server (`~/.local/bin/dsync-hub`), клиенты
  `~/.local/bin/dsync` на mkair/desktop/notebook.
- **Desktop-таймер**: включён обратно; hub-пулл теперь autostash (WIP агента в
  `~/projects/mushroomchess` не трогается).
- **Проверено**: сессия `ses_f3b844870ffePmrEVos0lZqSUU` (mushroomchess, mkair) появилась
  на desktop и наоборот.

## 4. Инфраструктура

- Hub: `archlinux-server:42069` (Netbird `100.89.126.211`), unit `dsync-hub.service`
  (`dsync-hub daemon`), data `~/.local/share/dsync-hub/`.
- Деплой хаба: `systemctl --user stop dsync-hub.service` → `cat target/release/dsync |
  ssh archlinux-server "cat > /tmp/x && chmod +x /tmp/x && mv /tmp/x ~/.local/bin/dsync-hub"` →
  `systemctl --user start dsync-hub.service`.
- Машины: desktop (`100.89.12.158`), notebook (`100.89.198.212`), archlinux-mkair, archlinux-server.
- **Секреты**: per-machine `~/.config/dsync/dsync/tokens.toml` (0600, chezmoi-ignored).
  Новой машине завести вручную, иначе «offline» (auth failed). Токены хаба — в
  `tokens.toml` на сервере (`[hub] tokens`).
- ⚠️ **Не редактировать `~/projects/dsync` напрямую незакоммиченным** — хаб на чужом пуше
  делает `git pull --rebase --autostash` (теперь безопаснее, но всё же). Для разработки —
  `git worktree add`.
- ⚠️ Конфиг `~/.config/dsync/dsync/config.toml` — chezmoi-шаблон; правки в
  `~/dotfiles/dot_config/dsync/dsync/config.toml.tmpl`.

## 5. Что дальше

1. **notebook: opencode V2** — там ещё V1 (`/usr/bin/opencode 1.18.21`), сессии не синкаются.
   Ставить как на desktop: `npm config set prefix ~/.opencode`, `allow-scripts=@opencode/cli`,
   `npm i -g @opencode/cli`.
2. **Reboot notebook/desktop** после `pacman -Syu` (ядро обновлено). Desktop не ребутить,
   пока там работает агент.
3. `tmux_restore = true` — если хочется авто-восстановление раскладки в живом tmux.
4. **Phase 4**: security/reliability (аудит хаба, ретраи, лимиты).
5. **Phase 5**: crates.io (0.1.0), cargo-dist, AUR/homebrew.
6. **Phase 6**: продвижение.
7. Хлам в репо: legacy Python (`build/lib/dsync/*.py`, `src/dsync.egg-info`, `src/dsync/__pycache__`)
   всё ещё отслеживается — вычистить; `PLAN.md` устарел.

## 6. Ключевые файлы

- `src/client/state.rs` — tmux/opencode state sync (collect/apply/индекс)
- `src/protocol.rs` — `StateItem`, `StatePush/Pull*`
- `src/hub/state.rs` — хранилище состояния хаба
- `src/hub/server.rs` — `state_push`/`state_pull` + `--autostash` pull
- `src/client/{push,pull}.rs` — вызов `state::sync_state`
- `src/init.rs`, `src/config.rs`, `src/cli.rs`, `src/main.rs`
- `~/dotfiles/dot_config/dsync/dsync/config.toml.tmpl` — `[state]` + проекты
- `~/dotfiles/AGENTS.md` — документация фичи (обновлена)