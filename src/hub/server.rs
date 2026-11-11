use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quinn::{Endpoint, Incoming, ServerConfig};
use tokio::io::AsyncReadExt;
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::config::{Config, SecretTokens};
use crate::protocol::{
    error_envelope, PullOutcome, PullRequest, PullResponse, PushRequest, PushResponse,
    StatePullRequest, StatePullResponse, StatePushRequest, StatePushResponse, StateStatusRequest,
};

use super::state::HubState;
use super::token_matches;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Итоговая карта токенов хаба: конфиг главнее, sidecar-файл — fallback.
/// Пустая карта означает fail-secure (хаб не стартует).
fn effective_hub_tokens(
    cfg_tokens: &std::collections::HashMap<String, String>,
    secret: SecretTokens,
) -> std::collections::HashMap<String, String> {
    if !cfg_tokens.is_empty() {
        cfg_tokens.clone()
    } else {
        secret.hub.map(|h| h.tokens).unwrap_or_default()
    }
}

pub async fn run_server(cfg: Config) -> Result<()> {
    // Fail-secure: без токенов хаб не стартует (см. hub-auth). Источник —
    // `[hub] tokens` из конфига, иначе — sidecar `tokens.toml` (главный конфиг
    // управляется chezmoi и перегенерируется при `chezmoi apply`, поэтому
    // секреты в нём не живут; см. config::read_secret_tokens).
    let (cfg_tokens, bind): (std::collections::HashMap<String, String>, SocketAddr) = {
        let hub_cfg = cfg
            .hub
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no [hub] section in config — can't run daemon"))?;
        (hub_cfg.tokens.clone(), hub_cfg.bind.parse()?)
    };
    let hub_tokens = effective_hub_tokens(&cfg_tokens, crate::config::read_secret_tokens());
    if hub_tokens.is_empty() {
        anyhow::bail!(
            "hub refuses to start: [hub] tokens is missing/empty.\n\
             Configure per-machine tokens: rerun `dsync init` with the hub role, set\n\
             [hub] tokens in the config, or create the sidecar file:\n\n\
             ~/.config/dsync/dsync/tokens.toml\n\
             [hub]\n\
             tokens = {{ \"<machine>\": \"<token>\" }}\n\n\
             and put the matching [hub_connect] token on each client."
        );
    }
    // Резолвим токены один раз и кладём обратно в конфиг: проверка запросов
    // (authorized) читает cfg.hub.tokens, так что быть должна уже резолвленная
    // карта, а не только конфиговая.
    let mut cfg = cfg;
    cfg.hub.as_mut().unwrap().tokens = hub_tokens;
    let hub_cfg = cfg.hub.as_ref().unwrap();

    info!(
        "starting dsync hub {} on {bind}",
        crate::protocol::version_string()
    );

    let data_dir = cfg.hub_data_dir();

    let (cert, key) = load_or_generate_certs(&cfg, &data_dir)?;
    let server_config = make_server_config(cert, key)?;
    let endpoint = Endpoint::server(server_config, bind)?;

    let state = Arc::new(HubState::new(
        Some(data_dir.clone()),
        hub_cfg.retention_days,
    ));
    if state.prune_stale().await {
        info!("pruned stale machines at startup");
    }
    // Гигиена: осиротевшие tmp-файлы атомарных записей (`tmp` + `rename`)
    // остаются, если процесс убили между create и rename. Наблюдались сотни
    // таких файлов в каталоге хаба — чистим всё старше часа.
    let removed_tmp =
        crate::hub::state::cleanup_stale_temp_files(&data_dir, Duration::from_secs(3600));
    if removed_tmp > 0 {
        info!(
            "removed {removed_tmp} stale tmp file(s) from {}",
            data_dir.display()
        );
    }
    // Мусор от каналов, которых больше нет в коде/конфиге (например `zellij`,
    // поддержка которого удалена): применяться всё равно некому.
    if let Some(st) = cfg.state.as_ref() {
        let mut keep = std::collections::HashSet::new();
        if st.tmux {
            keep.insert("tmux".to_string());
        }
        if st.opencode.is_some() {
            keep.insert("opencode".to_string());
        }
        if !keep.is_empty() {
            let pruned = state.prune_state_channels(&keep).await;
            if pruned > 0 {
                warn!("pruned {pruned} state item(s) from unknown channels");
            }
        }
    }

    let max_message_size = hub_cfg.max_message_size;
    let semaphore = Arc::new(Semaphore::new(hub_cfg.max_concurrency as usize));
    let cfg = Arc::new(cfg);

    info!("hub listening on {bind}");

    // Основной цикл accept'а живёт в serve_loop (его же гоняют эндпоинт-тесты);
    // здесь ждём его завершения или Ctrl-C.
    let serve = serve_loop(endpoint, state, cfg, max_message_size, semaphore);
    tokio::select! {
        _ = serve => {}
        _ = signal::ctrl_c() => {
            info!("shutting down hub");
        }
    }

    info!("hub stopped");
    Ok(())
}

/// Основной цикл обработки соединений — вынесен отдельно, чтобы тесты могли
/// поднять реальный quinn-endpoint и прогнать auth/limits без daemon-цикла.
async fn serve_loop(
    endpoint: Endpoint,
    state: Arc<HubState>,
    cfg: Arc<Config>,
    max_message_size: u64,
    semaphore: Arc<Semaphore>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        let cfg = cfg.clone();
        let sem = semaphore.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(incoming, state, cfg, max_message_size, sem).await {
                error!("connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    incoming: Incoming,
    state: Arc<HubState>,
    cfg: Arc<Config>,
    max_message_size: u64,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let connection = incoming.await?;
    let remote = connection.remote_address();
    info!("new connection from {remote}");

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                // Ограничение параллельности: 5с на слот, иначе — явная ошибка
                // «hub busy», а не молчаливая потеря запроса.
                let permit = match tokio::time::timeout(
                    Duration::from_secs(5),
                    semaphore.clone().acquire_owned(),
                )
                .await
                {
                    Ok(Ok(p)) => p,
                    Ok(Err(_)) => {
                        error!("semaphore closed");
                        break;
                    }
                    Err(_) => {
                        let err =
                            error_envelope("hub busy — retry later (max_concurrency reached)");
                        let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                        continue;
                    }
                };

                // Ограничение размера сообщения: читаем не более limit+1 байт.
                let mut buf = Vec::new();
                {
                    let mut limited = (&mut recv).take(max_message_size + 1);
                    let _ = limited.read_to_end(&mut buf).await;
                }
                if buf.len() as u64 > max_message_size {
                    let err = error_envelope(format!(
                        "request exceeds max_message_size ({max_message_size} bytes)"
                    ));
                    let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                    continue;
                }

                let msg_str = String::from_utf8_lossy(&buf);
                match serde_json::from_str::<serde_json::Value>(&msg_str) {
                    Ok(val) => {
                        let kind = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let resp = match kind {
                            "push" | "pull" | "status" | "state_push" | "state_pull"
                            | "state_status"
                                if authorized(&val, cfg.hub.as_ref()) =>
                            {
                                match kind {
                                    "push" => handle_push(val, &state, &cfg).await,
                                    "pull" => handle_pull(val, &state).await,
                                    "state_push" => handle_state_push(val, &state).await,
                                    "state_pull" => handle_state_pull(val, &state, &cfg).await,
                                    "state_status" => handle_state_status(val, &state).await,
                                    _ => handle_status(&state).await,
                                }
                            }
                            "push" | "pull" | "status" | "state_push" | "state_pull"
                            | "state_status" => {
                                let machine =
                                    val.get("machine").and_then(|v| v.as_str()).unwrap_or("?");
                                warn!("authentication failed for machine '{machine}'");
                                error_envelope(format!(
                                    "authentication failed for machine '{machine}': check \
                                     [hub_connect] token and [hub] tokens"
                                ))
                            }
                            _ => {
                                error!("unknown message type: {kind}");
                                let err = error_envelope(format!("unknown message type: {kind}"));
                                let _ = send.write_all(&serde_json::to_vec(&err)?).await;
                                continue;
                            }
                        };

                        let data = serde_json::to_vec(&resp)?;
                        send.write_all(&data).await?;
                    }
                    Err(e) => {
                        error!("invalid JSON: {e}");
                    }
                }
                drop(permit);
            }
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                info!("connection from {remote} closed");
                break;
            }
            Err(e) => {
                error!("connection error from {remote}: {e}");
                break;
            }
        }
    }

    Ok(())
}

/// Проверяет, что запрос несёт валидный токен известной машины флота.
fn authorized(val: &serde_json::Value, hub: Option<&crate::config::HubConfig>) -> bool {
    let Some(hub) = hub else {
        return false;
    };
    let Some(machine) = val.get("machine").and_then(|v| v.as_str()) else {
        return false;
    };
    let token = val.get("token").and_then(|v| v.as_str()).unwrap_or("");
    match hub.tokens.get(machine) {
        Some(expected) => token_matches(expected, token),
        None => false,
    }
}

async fn handle_push(
    val: serde_json::Value,
    state: &Arc<HubState>,
    cfg: &Config,
) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PushRequest>(val) {
        let machine = req.machine.clone();
        state
            .update_machine(crate::protocol::MachineState {
                name: machine.clone(),
                // Время ставит хаб своими часами: req.timestamp — это часы
                // клиента, и при их уходе вперёд/назад машина навсегда
                // выпадает из online (online = last_push <= 35 мин назад).
                last_push: unix_now(),
                projects: req.projects.clone(),
                pulls: std::collections::HashMap::new(),
            })
            .await;
        info!("push from {machine} accepted");

        trigger_remote_pulls(&req, cfg, state).await;

        serde_json::to_value(PushResponse {
            ok: true,
            error: None,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid push request")
    }
}

/// Решает, разворачивать ли проект на флоте при получении push:
/// явные `[projects.*]` — всегда; авто-проекты — только если не включён
/// контроль-режим (`[auto_projects] sync = false` → анонс без разворота).
fn should_pull_project(is_auto: bool, auto_sync: Option<bool>) -> bool {
    !is_auto || auto_sync.unwrap_or(true)
}

async fn trigger_remote_pulls(req: &PushRequest, cfg: &Config, state: &Arc<HubState>) {
    let Some(remote_cfg) = &cfg.remote else {
        return;
    };
    let projects_cfg = cfg.projects.as_ref();
    let auto_cfg = cfg.auto_projects.as_ref();
    let pull_retries = cfg
        .hub
        .as_ref()
        .map(|h| h.pull_retries)
        .unwrap_or(crate::config::default_pull_retries());
    // Exec пула живёт дольше connect (post_pull — иногда cargo build), поэтому
    // таймауты раздельные: connect 30 c, exec — [hub] pull_timeout_secs.
    let pull_timeout = cfg
        .hub
        .as_ref()
        .map(|h| h.pull_timeout_secs)
        .unwrap_or_else(crate::config::default_pull_timeout_secs);
    let store_path = crate::ssh::trust::SshHostTrustStore::path(&cfg.hub_data_dir());
    let ssh_key = cfg.machine.ssh_key_path();
    // Экономика пуллов: не дёргать машины, которые давно не выходили на связь,
    // и не пулить проект, чей commit уже развёрнут на этой машине.
    let offline_skip = cfg
        .hub
        .as_ref()
        .map(|h| h.offline_skip_after_secs)
        .unwrap_or_else(crate::config::default_offline_skip_after_secs);
    let only_on_change = cfg
        .hub
        .as_ref()
        .map(|h| h.pull_only_on_change)
        .unwrap_or_else(crate::config::default_pull_only_on_change);
    let now = unix_now();
    // Сводка «сколько реально пулим» — иначе экономия невидима (debug-строки
    // в проде не пишутся).
    let mut started = 0usize;
    let mut unchanged = 0usize;
    let mut offline = 0usize;

    for project in &req.projects {
        let static_cfg = projects_cfg.and_then(|p| p.get(&project.name));

        // Контроль-режим ([auto_projects] sync = false): авто-проект анонсирован
        // (статус виден в status/TUI), но не разворачивается — pull пропускаем.
        // Явные [projects.*] разворачиваются всегда.
        if !should_pull_project(static_cfg.is_none(), auto_cfg.and_then(|a| a.sync)) {
            continue;
        }

        // Машины: из явного [projects.X] machines, иначе — [auto_projects]
        // machines, иначе — весь флот из [remote.*].
        let machines: Vec<String> = match static_cfg.and_then(|pc| pc.machines.clone()) {
            Some(m) if !m.is_empty() => m,
            _ => match auto_cfg.and_then(|a| a.machines.clone()) {
                Some(m) if !m.is_empty() => m,
                _ => remote_cfg.keys().cloned().collect(),
            },
        };
        if machines.is_empty() {
            continue;
        }

        // Ветка: статическая конфигурация важнее; для авто-проектов — ветка
        // машины-источника (приходит в состоянии проекта).
        let branch = static_cfg
            .and_then(|pc| pc.branch.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| {
                if project.branch.is_empty() {
                    auto_cfg
                        .and_then(|a| a.branch.clone())
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| "main".to_string())
                } else {
                    project.branch.clone()
                }
            });
        let path = crate::projects::status::expand_user_path(
            static_cfg
                .map(|pc| pc.path.as_path())
                .unwrap_or(std::path::Path::new(&project.path)),
        );
        let cmd = build_pull_command(
            &path,
            &branch,
            &project.url,
            static_cfg.and_then(|pc| pc.post_pull.as_deref()),
        );

        for machine_name in machines {
            if machine_name == req.machine {
                continue;
            }
            if let Some(target) = &req.target {
                if machine_name != *target {
                    continue;
                }
            }
            let Some(remote) = remote_cfg.get(&machine_name) else {
                error!("no remote config for machine {machine_name}");
                continue;
            };

            // Офлайн-машина: SSH-пулл всё равно упадёт, но с retry+backoff это
            // десятки секунд и запись «failed» каждый цикл. Пропускаем целиком.
            // Плюс change-detection: не пулим проект, чей commit уже развёрнут.
            let last_push = state.last_push(&machine_name).await;
            let deployed = state.deployed_commit(&machine_name, &project.name).await;
            if let Some(reason) = pull_skip_reason(
                only_on_change,
                offline_skip,
                now,
                last_push,
                deployed.as_deref(),
                &project.commit_hash,
            ) {
                if reason.starts_with("no push") {
                    offline += 1;
                } else {
                    unchanged += 1;
                }
                tracing::debug!("skipping pull {} → {machine_name}: {reason}", project.name);
                continue;
            }
            started += 1;

            let host = remote.host.clone();
            let port = remote.port;
            let user = remote.user.clone();
            let project_name = project.name.clone();
            let commit = project.commit_hash.clone();
            let machine_name = machine_name.clone();
            let cmd = cmd.clone();
            let state = state.clone();
            let store_path = store_path.clone();
            let ssh_key = ssh_key.clone();
            tokio::spawn(async move {
                info!("SSH pulling {project_name} on {machine_name} ({host})...");
                let outcome = retry_pull(pull_retries, || {
                    let store_path = store_path.clone();
                    let host = host.clone();
                    let user = user.clone();
                    let cmd = cmd.clone();
                    let ssh_key = ssh_key.clone();
                    async move {
                        match crate::ssh::client::exec_with_key_verifying_split(
                            &host,
                            port,
                            &user,
                            &cmd,
                            &ssh_key,
                            store_path,
                            crate::ssh::client::DEFAULT_SSH_TIMEOUT,
                            std::time::Duration::from_secs(pull_timeout),
                        )
                        .await
                        {
                            Ok(_) => Ok(()),
                            Err(e) => Err(format!("{e:#}")),
                        }
                    }
                })
                .await;
                state
                    .record_pull(&machine_name, &project_name, &outcome)
                    .await;
                match &outcome {
                    PullOutcome { ok: true, .. } => {
                        state
                            .record_deployed(&machine_name, &project_name, &commit)
                            .await;
                        info!("SSH pull {machine_name}/{project_name}: OK")
                    }
                    PullOutcome { ok: false, .. } => error!(
                        "SSH pull {machine_name}/{project_name} failed after {} attempts: {}",
                        outcome.attempts,
                        outcome.error.as_deref().unwrap_or("unknown error")
                    ),
                }
            });
        }
    }

    if started + unchanged + offline > 0 {
        info!(
            "pull plan from {}: {started} started, {unchanged} unchanged, {offline} offline",
            req.machine
        );
    }
}

/// Собирает remote-команду разворачивания проекта на целевой машине:
/// если каталога проекта ещё нет — `git clone` из origin (bootstrap), иначе —
/// обычный `git pull --rebase --autostash` (WIP не теряется); затем
/// `post_pull`, если задан. Если `url` пуст (у проекта нет remote) — только
/// pull по существующему каталогу.
fn build_pull_command(
    path: &std::path::Path,
    branch: &str,
    url: &str,
    post_pull: Option<&str>,
) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let dir = q(&path.display().to_string());
    let dot_git = q(&path.join(".git").display().to_string());
    let pull = format!(
        "cd {dir} && git pull --rebase --autostash origin {}",
        q(branch)
    );
    let mut cmd = if url.trim().is_empty() {
        pull.clone()
    } else {
        format!(
            "if [ -d {dot_git} ]; then {pull}; else git clone {} {dir} && cd {dir} && git checkout {}; fi",
            q(url.trim()),
            q(branch)
        )
    };
    if let Some(post) = post_pull {
        let post = post.trim();
        if !post.is_empty() {
            cmd.push_str(" && ");
            cmd.push_str(post);
        }
    }
    cmd
}

/// Retry-обёртка для SSH-пулла: первая попытка + `pull_retries` повторных,
/// backoff `5s·2^n` с потолком 60 с. Возвращает итоговый `PullOutcome`
/// (см. `hub-pull-orchestration`); тестируется с pause-time.
///
/// Ошибки уровня сети/аутентификации (`No route to host`, `Connection
/// refused`, `Permission denied`, …) повторять бессмысленно — хост оживёт не
/// от третьей попытки за 15 секунд.
async fn retry_pull<F, Fut>(pull_retries: u32, mut attempt: F) -> PullOutcome
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut attempts: u32 = 0;
    let mut backoff = Duration::from_secs(5);
    loop {
        attempts += 1;
        match attempt().await {
            Ok(()) => return PullOutcome::success(attempts),
            Err(err) => {
                if attempts > pull_retries || !is_transient_error(&err) {
                    return PullOutcome::failure(err, attempts);
                }
                let wait = backoff.min(Duration::from_secs(60));
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                backoff = backoff.saturating_mul(2);
            }
        }
    }
}

/// Стоит ли пропустить SSH-пулл (и почему). `None` — пулить.
///
/// Две экономии, без которых каждый push каждого клиента дёргал все проекты на
/// всех машинах (~1700 пуллов в сутки, из них ~1000 в офлайн-машину):
/// * машина давно не выходила на связь (`last_push` старше `offline_skip`);
/// * её `commit` уже развёрнут (change-driven pull).
fn pull_skip_reason(
    only_on_change: bool,
    offline_skip: u64,
    now: i64,
    last_push: Option<i64>,
    deployed: Option<&str>,
    source_commit: &str,
) -> Option<String> {
    if offline_skip > 0 {
        if let Some(last) = last_push {
            if last > 0 && now.saturating_sub(last) > offline_skip as i64 {
                return Some(format!("no push for {}s", now - last));
            }
        }
    }
    if only_on_change && !source_commit.is_empty() && deployed == Some(source_commit) {
        let short = &source_commit[..source_commit.len().min(8)];
        return Some(format!("already at {short}"));
    }
    None
}

/// Стоит ли повторять SSH-пулл после такой ошибки.
fn is_transient_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    !e.contains("no route to host")
        && !e.contains("connection refused")
        && !e.contains("network is unreachable")
        && !e.contains("nosuchhost")
        && !e.contains("name or service not known")
        && !e.contains("permission denied")
        && !e.contains("host key mismatch")
        && !e.contains("authentication failed")
}

async fn handle_pull(val: serde_json::Value, state: &HubState) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<PullRequest>(val) {
        state.prune_stale().await;
        let mut machines = state.all_machines().await;
        if let Some(machine) = req.filter.as_ref().and_then(|f| f.machine.as_ref()) {
            machines.retain(|name, _| name == machine);
        }
        info!("pull from {}: {} machines", req.machine, machines.len());
        serde_json::to_value(PullResponse { machines }).unwrap_or_default()
    } else {
        error_envelope("invalid pull request")
    }
}

async fn handle_state_push(val: serde_json::Value, state: &Arc<HubState>) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<StatePushRequest>(val) {
        let n = req.items.len();
        let seq = state.merge_state(req.items, &req.machine).await;
        info!("state push from {}: {n} item(s), seq={seq}", req.machine);
        serde_json::to_value(StatePushResponse {
            ok: true,
            error: None,
            state_seq: seq,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid state_push request")
    }
}

async fn handle_state_pull(
    val: serde_json::Value,
    state: &HubState,
    cfg: &Config,
) -> serde_json::Value {
    if let Ok(req) = serde_json::from_value::<StatePullRequest>(val) {
        // Старый клиент не шлёт max_bytes → 0 (без лимита): иначе он получил бы
        // одну страницу и, проигнорировав курсор, потерял остальное.
        // Новому клиенту дополнительно ограничиваем страницу конфигом хаба.
        let hub_page = cfg
            .hub
            .as_ref()
            .map(|h| h.state_pull_page_bytes)
            .unwrap_or(0);
        let max_bytes = if req.max_bytes == 0 {
            0
        } else if hub_page == 0 {
            req.max_bytes
        } else {
            req.max_bytes.min(hub_page)
        };
        let (items, seq, next_seq, has_more) = state
            .state_since_page(
                req.state_seq,
                &req.machine,
                max_bytes,
                req.channels.as_deref(),
            )
            .await;
        info!(
            "state pull from {}: {} item(s){}",
            req.machine,
            items.len(),
            if has_more { ", more pages" } else { "" }
        );
        serde_json::to_value(StatePullResponse {
            items,
            state_seq: seq,
            next_seq,
            has_more,
        })
        .unwrap_or_default()
    } else {
        error_envelope("invalid state_pull request")
    }
}

/// Read-only сводка state store (вкладка State в TUI). Старый хаб (без
/// `state_status`) отвечает error-обёрткой, клиент показывает "unavailable".
async fn handle_state_status(val: serde_json::Value, state: &HubState) -> serde_json::Value {
    if serde_json::from_value::<StateStatusRequest>(val).is_ok() {
        match state.state_status().await {
            Ok(resp) => serde_json::to_value(resp).unwrap_or_default(),
            Err(e) => error_envelope(format!("state_status: {e}")),
        }
    } else {
        error_envelope("invalid state_status request")
    }
}

async fn handle_status(state: &HubState) -> serde_json::Value {
    state.prune_stale().await;
    match state.status().await {
        Ok(mut resp) => {
            resp.hub_version = Some(crate::protocol::version_string());
            serde_json::to_value(resp).unwrap_or_default()
        }
        Err(_) => serde_json::Value::Null,
    }
}

fn make_server_config(
    cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    priv_key: rustls::pki_types::PrivateKeyDer<'static>,
) -> Result<ServerConfig> {
    let mut config =
        ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert_chain, priv_key)?,
        )?));
    config.transport = Arc::new(quinn::TransportConfig::default());
    Ok(config)
}

fn load_or_generate_certs(
    cfg: &Config,
    data_dir: &std::path::Path,
) -> Result<(
    Vec<rustls::pki_types::CertificateDer<'static>>,
    rustls::pki_types::PrivateKeyDer<'static>,
)> {
    // 1. Явно заданные через [hub] cert/key — приоритет.
    if let Some(hub) = &cfg.hub {
        if let (Some(cert_path), Some(key_path)) = (&hub.cert, &hub.key) {
            let cert = std::fs::read(cert_path)?;
            let key = std::fs::read(key_path)?;
            let certs =
                rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut key.as_slice())?.unwrap();
            return Ok((certs, key));
        }
    }

    // 2. Сертификат из data_dir (сохранён при прошлом запуске) — чтобы
    //    fingerprint был стабильным между рестартами и клиенты с TOFU
    //    не ломались после каждого перезапуска хаба.
    let cert_file = data_dir.join("server_cert.pem");
    let key_file = data_dir.join("server_key.pem");
    if cert_file.exists() && key_file.exists() {
        let cert = std::fs::read(&cert_file)?;
        let key = std::fs::read(&key_file)?;
        let certs = rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
        if let Some(key) = rustls_pemfile::private_key(&mut key.as_slice())? {
            return Ok((certs, key));
        }
        anyhow::bail!("server_key.pem in {data_dir:?} contains no private key");
    }

    // 3. Генерируем новый и сохраняем на диск.
    info!(
        "no certs found, generating self-signed (persisted in {})",
        data_dir.display()
    );
    let cert = rcgen::generate_simple_self_signed(vec!["dsync.local".into()])?;
    let cert_pem = cert.cert.pem();
    let key_pem = cert.key_pair.serialize_pem();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&cert_file, cert_pem)?;
    std::fs::write(&key_file, key_pem)?;

    let cert_der = std::fs::read(&cert_file)?;
    let key_der = std::fs::read(&key_file)?;
    let certs = rustls_pemfile::certs(&mut cert_der.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_der.as_slice())?.unwrap();
    Ok((certs, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use crate::config::{HubConfig, MachineConfig, ProjectConfig, RemoteMachine};

    #[test]
    fn hub_token_precedence_config_over_sidecar() {
        use std::collections::HashMap;
        // Конфиг непустой → победил конфиг.
        let cfg_toks = HashMap::from([("desktop".to_string(), "cfg-tok".to_string())]);
        let secret = SecretTokens {
            hub: Some(crate::config::SecretHub {
                tokens: HashMap::from([("desktop".to_string(), "file-tok".to_string())]),
            }),
            hub_connect: None,
        };
        let got = effective_hub_tokens(&cfg_toks, secret);
        assert_eq!(got["desktop"], "cfg-tok");
    }

    #[test]
    fn hub_token_precedence_sidecar_when_config_empty() {
        use std::collections::HashMap;
        let secret = SecretTokens {
            hub: Some(crate::config::SecretHub {
                tokens: HashMap::from([("notebook".to_string(), "file-tok".to_string())]),
            }),
            hub_connect: None,
        };
        let got = effective_hub_tokens(&HashMap::new(), secret);
        assert_eq!(got["notebook"], "file-tok");
    }

    #[test]
    fn hub_token_precedence_empty_when_both_empty() {
        use std::collections::HashMap;
        let got = effective_hub_tokens(&HashMap::new(), SecretTokens::default());
        assert!(got.is_empty());
    }

    #[test]
    fn should_pull_project_semantics() {
        // Явные [projects.*] пулятся всегда, даже при контроль-режиме.
        assert!(should_pull_project(false, Some(false)));
        assert!(should_pull_project(false, None));
        assert!(should_pull_project(false, Some(true)));
        // Авто-проекты: по умолчанию (None) и sync=true — разворачиваются.
        assert!(should_pull_project(true, None));
        assert!(should_pull_project(true, Some(true)));
        // Контроль-режим: авто-проекты анонсируются, но не пулятся.
        assert!(!should_pull_project(true, Some(false)));
    }

    /// Принимающий всё verifier — тестовый аналог TOFU-клиента.
    #[derive(Debug)]
    struct AcceptAll;

    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::RSA_PKCS1_SHA256,
                rustls::SignatureScheme::RSA_PKCS1_SHA384,
                rustls::SignatureScheme::RSA_PKCS1_SHA512,
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
                rustls::SignatureScheme::RSA_PSS_SHA256,
                rustls::SignatureScheme::RSA_PSS_SHA384,
                rustls::SignatureScheme::RSA_PSS_SHA512,
                rustls::SignatureScheme::ED25519,
            ]
        }
    }

    fn test_server_config() -> ServerConfig {
        let cert = rcgen::generate_simple_self_signed(vec!["dsync.local".into()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
        make_server_config(vec![cert_der], key_der).unwrap()
    }

    fn test_client_config() -> quinn::ClientConfig {
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAll))
            .with_no_client_auth();
        let quic = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
        quinn::ClientConfig::new(Arc::new(quic))
    }

    fn test_config(tokens: &[(&str, &str)], max_message_size: u64, max_concurrency: u32) -> Config {
        Config {
            config_version: 1,
            machine: MachineConfig {
                name: "hub-machine".into(),
                ssh_key: None,
            },
            hub: Some(HubConfig {
                bind: "127.0.0.1:0".into(),
                cert: None,
                key: None,
                data_dir: None,
                tokens: tokens
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
                max_message_size,
                max_concurrency,
                retention_days: 30,
                pull_retries: 2,
                pull_timeout_secs: crate::config::default_pull_timeout_secs(),
                pull_only_on_change: true,
                offline_skip_after_secs: 0,
                state_pull_page_bytes: crate::config::default_state_pull_page_bytes(),
            }),
            hub_connect: None,
            projects: Some(
                [(
                    "dotfiles".to_string(),
                    ProjectConfig {
                        path: "/tmp/nonexistent-dsync-test".into(),
                        branch: Some("main".into()),
                        machines: Some(vec!["notebook".into()]),
                        post_pull: None,
                    },
                )]
                .into_iter()
                .collect(),
            ),
            auto_projects: None,
            remote: Some(
                [(
                    "notebook".to_string(),
                    RemoteMachine {
                        host: "127.0.0.1".into(),
                        port: 1, // несуществующий порт — SSH сразу упадёт
                        user: "nobody".into(),
                    },
                )]
                .into_iter()
                .collect(),
            ),
            capture: None,
            state: None,
        }
    }

    /// Уникальный каталог данных для тестового хаба (иначе state не хранится).
    static HUB_TEST_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    async fn spawn_test_hub(
        tokens: &[(&str, &str)],
        max_message_size: u64,
        max_concurrency: u32,
    ) -> (SocketAddr, Arc<HubState>, Arc<Semaphore>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server_cfg = test_server_config();
        let endpoint = Endpoint::server(server_cfg, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = endpoint.local_addr().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "dsync-hub-test-{}-{}",
            std::process::id(),
            HUB_TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let state = Arc::new(HubState::new(Some(dir), 30));
        let cfg = Arc::new(test_config(tokens, max_message_size, max_concurrency));
        let semaphore = Arc::new(Semaphore::new(max_concurrency as usize));
        tokio::spawn(serve_loop(
            endpoint,
            state.clone(),
            cfg,
            max_message_size,
            semaphore.clone(),
        ));
        (addr, state, semaphore)
    }

    async fn connect(addr: SocketAddr) -> quinn::Connection {
        let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint
            .connect_with(test_client_config(), addr, "dsync.local")
            .unwrap()
            .await
            .unwrap()
    }

    async fn send_push(
        conn: &quinn::Connection,
        machine: &str,
        token: &str,
    ) -> anyhow::Result<crate::protocol::PushResponse> {
        let req = PushRequest {
            machine: machine.to_string(),
            token: token.to_string(),
            timestamp: unix_now(),
            projects: vec![],
            target: None,
        };
        crate::client::connect::send_push(conn, &req).await
    }

    #[tokio::test]
    async fn auth_accepts_valid_push_and_rejects_others() {
        let (addr, state, _sem) = spawn_test_hub(&[("desktop", "secret-a")], 1 << 20, 4).await;
        let conn = connect(addr).await;

        // Неизвестная машина — отказ, состояние не тронуто.
        let r = send_push(&conn, "desktop", "wrong-token").await;
        assert!(r.is_err(), "wrong token must be rejected");
        assert!(
            r.unwrap_err().to_string().contains("authentication failed"),
            "actionable error expected"
        );

        let r = send_push(&conn, "intruder", "secret-a").await;
        assert!(r.is_err(), "unknown machine must be rejected");
        assert!(
            state.all_machines().await.is_empty(),
            "no state changes on rejection"
        );

        // Валидный push — принят, состояние записано.
        let resp = send_push(&conn, "desktop", "secret-a").await.unwrap();
        assert!(resp.ok);
        let m = state.all_machines().await;
        assert_eq!(m.len(), 1);
        assert_eq!(m["desktop"].name, "desktop");
    }

    #[tokio::test]
    async fn oversized_request_rejected_and_small_accepted() {
        let (addr, state, _sem) = spawn_test_hub(&[("desktop", "t")], 1024, 4).await;
        let conn = connect(addr).await;

        // Невалидный JSON, но большой — должен упасть по размеру раньше разбора.
        let big = vec![b'x'; 4096];
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(&big).await.unwrap();
        send.finish().unwrap();
        let buf = recv.read_to_end(usize::MAX).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(val["type"], "error");
        assert!(
            val["error"].as_str().unwrap().contains("max_message_size"),
            "explicit size error"
        );

        // Малый запрос проходит.
        let resp = send_push(&conn, "desktop", "t").await.unwrap();
        assert!(resp.ok);
        assert_eq!(state.all_machines().await.len(), 1);
    }

    #[tokio::test]
    async fn concurrency_cap_yields_explicit_busy_error() {
        let max = 1;
        let (addr, _state, semaphore) = spawn_test_hub(&[("desktop", "t")], 1 << 20, max).await;
        let conn = connect(addr).await;

        // Захватываем единственный permit сервера вручную — второй запрос
        // должен получить «hub busy» через 5с таймаут ожидания, а не молча
        // пропасть (и не дождаться обработки).
        let _held = semaphore.acquire_owned().await.unwrap();

        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let req = PushRequest {
            machine: "desktop".into(),
            token: "t".into(),
            timestamp: unix_now(),
            projects: vec![],
            target: None,
        };
        let mut msg = serde_json::to_value(&req).unwrap();
        msg["type"] = serde_json::json!("push");
        send.write_all(&serde_json::to_vec(&msg).unwrap())
            .await
            .unwrap();
        send.finish().unwrap();
        let buf = recv.read_to_end(usize::MAX).await.unwrap();
        let val: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(val["type"], "error");
        assert!(val["error"].as_str().unwrap().contains("hub busy"));
    }

    #[tokio::test]
    async fn state_push_pull_routing_and_self_exclusion() {
        use crate::protocol::{StateItem, StatePullRequest, StatePushRequest};

        let (addr, _state, _sem) =
            spawn_test_hub(&[("desktop", "td"), ("notebook", "tn")], 1 << 20, 4).await;
        let conn = connect(addr).await;

        // desktop загружает элемент состояния.
        let push = StatePushRequest {
            machine: "desktop".into(),
            token: "td".into(),
            timestamp: unix_now(),
            items: vec![StateItem {
                channel: "opencode".into(),
                key: "ses_1".into(),
                updated: 5,
                data: r#"{"k":1}"#.into(),
                ..Default::default()
            }],
        };
        let resp = crate::client::connect::send_state_push(&conn, &push)
            .await
            .unwrap();
        assert!(resp.ok);
        assert_eq!(resp.state_seq, 1);

        // notebook видит элемент.
        let pull = StatePullRequest {
            machine: "notebook".into(),
            token: "tn".into(),
            state_seq: 0,
            max_bytes: 0,
            channels: None,
        };
        let r = crate::client::connect::send_state_pull(&conn, &pull)
            .await
            .unwrap();
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].key, "ses_1");
        assert_eq!(r.items[0].origin, "desktop");
        assert_eq!(r.items[0].data, r#"{"k":1}"#);
        assert_eq!(r.state_seq, 1);

        // desktop не получает свой же элемент обратно.
        let own = StatePullRequest {
            machine: "desktop".into(),
            token: "td".into(),
            state_seq: 0,
            max_bytes: 0,
            channels: None,
        };
        assert!(crate::client::connect::send_state_pull(&conn, &own)
            .await
            .unwrap()
            .items
            .is_empty());

        // Чужой токен — отказ (и не утечка).
        let bad = StatePullRequest {
            machine: "notebook".into(),
            token: "wrong".into(),
            state_seq: 0,
            max_bytes: 0,
            channels: None,
        };
        let err = crate::client::connect::send_state_pull(&conn, &bad).await;
        assert!(err.is_err(), "bad token must be rejected");
        assert!(err
            .unwrap_err()
            .to_string()
            .contains("authentication failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_first_attempt_fails_then_succeeds() {
        let mut calls: u32 = 0;
        let outcome = retry_pull(2, || {
            calls += 1;
            async move {
                if calls == 1 {
                    Err("ssh dead".to_string())
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert!(outcome.ok, "pulled after the first failure");
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.error, None);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_all_failures_store_last_error() {
        let outcome = retry_pull(2, || async { Err("boom".to_string()) }).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.attempts, 3, "initial attempt + 2 retries");
        assert_eq!(outcome.error.as_deref(), Some("boom"));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_zero_pull_retries_single_attempt() {
        let outcome = retry_pull(0, || async { Err("nope".to_string()) }).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.attempts, 1);
        assert_eq!(outcome.error.as_deref(), Some("nope"));
    }

    #[test]
    fn pull_skip_reason_offline_and_unchanged() {
        let now = 1_000_000;
        // Машина офлайн дольше порога — не пулим.
        assert!(pull_skip_reason(true, 2700, now, Some(now - 3600), None, "abc").is_some());
        // Недавний push и новый commit — пулим.
        assert!(pull_skip_reason(true, 2700, now, Some(now - 60), None, "abc").is_none());
        // Этот commit уже развёрнут — не пулим.
        assert!(pull_skip_reason(true, 2700, now, Some(now - 60), Some("abc"), "abc").is_some());
        // Change-detection выключен — пулим даже тот же commit.
        assert!(pull_skip_reason(false, 0, now, Some(now - 60), Some("abc"), "abc").is_none());
        // Репо без коммитов — определять нечего, пулим.
        assert!(pull_skip_reason(true, 2700, now, Some(now - 60), Some(""), "").is_none());
        // Машина ещё ни разу не пушила — не считаем офлайн.
        assert!(pull_skip_reason(true, 2700, now, None, None, "abc").is_none());
    }

    #[test]
    fn non_transient_errors_are_not_retried() {
        assert!(!is_transient_error("No route to host (os error 113)"));
        assert!(!is_transient_error("Connection refused"));
        assert!(!is_transient_error("Permission denied (publickey)"));
        assert!(is_transient_error("ssh handshake timed out"));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_stops_immediately_on_non_transient_error() {
        let mut calls = 0u32;
        let outcome = retry_pull(5, || {
            calls += 1;
            async move { Err("No route to host (os error 113)".to_string()) }
        })
        .await;
        assert!(!outcome.ok);
        assert_eq!(outcome.attempts, 1, "no retries for an unreachable host");
        assert_eq!(calls, 1);
    }

    #[test]
    fn pull_command_clones_when_dir_missing_else_pulls() {
        let path = std::path::Path::new("/home/mflkee/projects/newproj");
        // Есть url — bootstrap-клон для машин без каталога.
        let cmd = build_pull_command(path, "main", "git@github.com:mflkee/mycelium.git", None);
        assert!(
            cmd.contains("if [ -d '/home/mflkee/projects/newproj/.git' ]; then"),
            "clone-if-missing guard expected, got: {cmd}"
        );
        assert!(
            cmd.contains(
                "git clone 'git@github.com:mflkee/mycelium.git' '/home/mflkee/projects/newproj'"
            ),
            "clone with url expected, got: {cmd}"
        );
        assert!(cmd.contains("git checkout 'main'"));

        // Путь/имя с пробелом и кавычками в пути — экранирование одинарными.
        let weird = std::path::Path::new("/tmp/a b/'quote'");
        let cmd = build_pull_command(weird, "main", "git@h:r.git", None);
        assert!(cmd.contains("'/tmp/a b/'\\''quote'\\''/.git'"));
    }

    #[test]
    fn pull_command_pull_only_without_url_and_appends_post_pull() {
        let path = std::path::Path::new("/home/mflkee/projects/localonly");
        let cmd = build_pull_command(path, "dev", "", Some("bash ./deploy.sh"));
        assert!(cmd.starts_with(
            "cd '/home/mflkee/projects/localonly' && git pull --rebase --autostash origin 'dev'"
        ));
        assert!(cmd.ends_with("&& bash ./deploy.sh"));
        assert!(!cmd.contains("git clone"));
    }
}
