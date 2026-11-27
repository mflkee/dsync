//! Планировщик SSH-пуллов: какие проекты/машины пулить, retry и команда.

use super::*;

/// Решает, разворачивать ли проект на флоте при получении push:
/// явные `[projects.*]` — всегда; авто-проекты — только если не включён
/// контроль-режим (`[auto_projects] sync = false` → анонс без разворота).
pub(crate) fn should_pull_project(is_auto: bool, auto_sync: Option<bool>) -> bool {
    !is_auto || auto_sync.unwrap_or(true)
}

pub(super) async fn trigger_remote_pulls(req: &PushRequest, cfg: &Config, state: &Arc<HubState>) {
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
    let now = now_secs();
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
pub(crate) fn build_pull_command(
    path: &std::path::Path,
    branch: &str,
    url: &str,
    post_pull: Option<&str>,
) -> String {
    let path_str = path.display().to_string();
    let dir = crate::infra::shell::quote(&path_str);
    // Команда исполняется в POSIX-шелле на удалённой машине, поэтому путь к
    // `.git` склеиваем через `/`, а не `Path::join` — на Windows-хосте (CI)
    // он дал бы `...\.git` и сломал бы guard.
    let dot_git = crate::infra::shell::quote(&crate::infra::shell::join_posix(&path_str, ".git"));
    let pull = format!(
        "cd {dir} && git pull --rebase --autostash origin {}",
        crate::infra::shell::quote(branch)
    );
    let mut cmd = if url.trim().is_empty() {
        pull.clone()
    } else {
        format!(
            "if [ -d {dot_git} ]; then {pull}; else git clone {} {dir} && cd {dir} && git checkout {}; fi",
            crate::infra::shell::quote(url.trim()),
            crate::infra::shell::quote(branch)
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
pub(crate) async fn retry_pull<F, Fut>(pull_retries: u32, mut attempt: F) -> PullOutcome
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let out = crate::infra::retry::retry(
        pull_retries.saturating_add(1),
        Duration::from_secs(5),
        Duration::from_secs(60),
        |_| attempt(),
        |e: &String| is_transient_error(e),
    )
    .await;
    match out.value {
        Ok(()) => PullOutcome::success(out.attempts),
        Err(err) => PullOutcome::failure(err, out.attempts),
    }
}

/// Стоит ли пропустить SSH-пулл (и почему). `None` — пулить.
///
/// Две экономии, без которых каждый push каждого клиента дёргал все проекты на
/// всех машинах (~1700 пуллов в сутки, из них ~1000 в офлайн-машину):
/// * машина давно не выходила на связь (`last_push` старше `offline_skip`);
/// * её `commit` уже развёрнут (change-driven pull).
pub(crate) fn pull_skip_reason(
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
pub(crate) fn is_transient_error(err: &str) -> bool {
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
