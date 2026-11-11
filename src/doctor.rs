use anyhow::Result;

use crate::config::Config;
use crate::protocol::{StateStatusResponse, StatusResponse};

enum Status {
    Ok,
    Skip(&'static str),
    Warn(String),
}

pub async fn run(cfg: Config) -> Result<()> {
    println!("dsync doctor\n");

    check("config", || Status::Ok);

    check("hub_connect", || {
        let addr = match &cfg.hub_connect {
            Some(h) => &h.address,
            None => return Status::Skip("no hub_connect in config"),
        };
        if addr.is_empty() {
            return Status::Warn("hub_connect.address is empty".into());
        }
        Status::Ok
    });

    check("machine name", || {
        let name = &cfg.machine.name;
        if name.is_empty() {
            return Status::Warn("machine name is empty".into());
        }
        Status::Ok
    });

    check("hub token", || {
        if cfg.hub_connect.is_none() {
            return Status::Skip("no hub_connect in config");
        }
        // Резолв через sidecar tokens.toml: секреты не живут в главном конфиге
        // (chezmoi-managed), см. config::read_secret_tokens.
        let token = crate::client::connect::hub_token(&cfg);
        if token.is_empty() {
            return Status::Warn(
                "hub token is empty — hub will reject requests; set it in the config or in the sidecar ~/.config/dsync/dsync/tokens.toml ([hub_connect] token)"
                    .into(),
            );
        }
        Status::Ok
    });

    check("hub trust (TOFU)", || {
        let addr = match &cfg.hub_connect {
            Some(h) => &h.address,
            None => return Status::Skip("no hub_connect in config"),
        };
        let store = crate::trust::TrustStore::load();
        match store.get(addr) {
            Some(_) => Status::Ok,
            None => {
                Status::Warn("not trusted yet — first connect will trust the hub (TOFU)".into())
            }
        }
    });

    check("ssh key", || {
        let key = cfg.machine.ssh_key_path();
        if key.exists() {
            Status::Ok
        } else {
            Status::Warn(format!("not found at {}", key.display()))
        }
    });

    check("netbird route", || {
        let has_wt = std::net::UdpSocket::bind("0.0.0.0:0")
            .ok()
            .and_then(|s| {
                s.connect("100.89.0.1:1").ok()?;
                s.local_addr().ok()
            })
            .is_some();
        if has_wt {
            Status::Ok
        } else {
            Status::Warn("no 100.89.x.x route (netbird down?)".into())
        }
    });

    if let Some(ref projects) = cfg.projects {
        println!("\nprojects:");
        for (name, p) in projects {
            let path = crate::projects::status::expand_user_path(&p.path);
            check(name, || {
                if !path.exists() {
                    return Status::Warn(format!("path not found: {}", path.display()));
                }
                if !path.join(".git").exists() {
                    return Status::Warn(format!("not a git repo at {}", path.display()));
                }
                Status::Ok
            });
        }
    } else {
        println!("\n  (no projects configured)");
    }

    if let Some(ref ap) = cfg.auto_projects {
        let root = ap
            .root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("~/projects"));
        println!("\nauto_projects ({:?}):", root);
        println!("  ✓ watching root, new git projects auto-deploy to fleet");
    }

    println!("\nchecking hub connectivity...");
    match hub_snapshot(&cfg).await {
        Ok((resp, state_resp)) => {
            let hub_v = resp
                .hub_version
                .as_deref()
                .unwrap_or("unknown (old hub — обновите dsync-hub)");
            let local_v = crate::protocol::version_string();
            let mut names: Vec<&String> = resp.machines.keys().collect();
            names.sort();
            for name in names {
                println!("    {name}: online={}", resp.machines[name].online);
            }
            println!("\nhub version:");
            if resp.hub_version.as_deref() == Some(local_v.as_str()) {
                println!("  ✓ hub {hub_v} matches this client");
            } else {
                println!("  ✗ hub {hub_v} ≠ client {local_v}");
                println!("      обновите хаб: git pull && cargo build --release && systemctl --user restart dsync-hub");
            }
            if let Some(state) = state_resp {
                println!("\nstate store (hub):");
                if state.channels.is_empty() {
                    println!("  ∼ пусто (каналы ещё не синхронизировались)");
                }
                for ch in &state.channels {
                    let err = ch
                        .error
                        .as_deref()
                        .map(|e| format!(" — last error: {e}"))
                        .unwrap_or_default();
                    println!(
                        "  ✓ {}: {} item(s), {}, last {}{err}",
                        ch.channel,
                        ch.item_count,
                        human_bytes(ch.bytes),
                        ch.last_origin
                    );
                }
            }
        }
        Err(e) => println!("  hub: {e}"),
    }

    // Локальный state-индекс: сбойные и dead-letter элементы.
    let health = crate::client::state::health();
    if cfg.state.is_some() {
        println!("\nstate (local):");
        println!(
            "  seq={}, applied={}, failing={}, dead-letter={}",
            health.seq,
            health.applied,
            health.failures.len(),
            health.dead.len()
        );
        for (key, d) in &health.dead {
            println!("  ✗ {key}: {}", d.error);
        }
        for (key, f) in &health.failures {
            println!(
                "  ∼ {key}: {} неудачн. попыт(ки), будет повторено",
                f.attempts
            );
        }
    }

    // Мусор атомарных записей хаба (`tmp` без rename).
    let data_dir = cfg.hub_data_dir();
    if data_dir.is_dir() {
        let orphans = count_tmp_files(&data_dir);
        if orphans > 0 {
            println!(
                "\n  ✗ {} осиротевш. tmp-файл(ов) в {} — удалит сам хаб при старте (старше 1 ч)",
                orphans,
                data_dir.display()
            );
        }
    }

    // opencode: без `session import` (V1) канал состояния не может применяться.
    if cfg
        .state
        .as_ref()
        .and_then(|s| s.opencode.as_ref())
        .map(|oc| oc.enabled())
        .unwrap_or(false)
    {
        check("opencode state", || {
            let Some(bin) = crate::client::state::opencode_binary() else {
                return Status::Warn(
                    "opencode не найден — сессии не будут ни экспортироваться, ни импортироваться"
                        .into(),
                );
            };
            match std::process::Command::new(&bin)
                .args(["session", "import", "--help"])
                .output()
            {
                Ok(o) => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    if text.contains("import") {
                        Status::Ok
                    } else {
                        Status::Warn(format!(
                            "{} не поддерживает `session import` (opencode V1?) — сессии не применяются; поставьте V2 в ~/.opencode/bin/opencode",
                            bin.display()
                        ))
                    }
                }
                Err(e) => Status::Warn(format!("не удалось запустить {}: {e}", bin.display())),
            }
        });
    }

    // SSH host-key trust: TOFU-якорь хаба, по одной машине флота.
    if let Some(ref remotes) = cfg.remote {
        println!("\nssh host trust:");
        let store_path = crate::ssh::trust::SshHostTrustStore::path(&cfg.hub_data_dir());
        for (name, r) in remotes {
            let hostport = format!("{}:{}", r.host, r.port);
            let state =
                crate::ssh::client::probe_host_trust(&r.host, r.port, store_path.clone()).await;
            let line = match &state {
                crate::ssh::trust::SshTrustState::Trusted => {
                    let fp = {
                        let store = crate::ssh::trust::SshHostTrustStore::load(&store_path);
                        store.get(&hostport).unwrap_or("").to_string()
                    };
                    format!("  ✓ {name} ({hostport}): trusted ({fp})")
                }
                crate::ssh::trust::SshTrustState::Untrusted => {
                    format!("  ∼ {name} ({hostport}): untrusted — first contact will be recorded (TOFU)")
                }
                crate::ssh::trust::SshTrustState::Mismatch { expected, observed } => format!(
                    "  ✗ {name} ({hostport}): MISMATCH\n      stored:     {expected}\n      presented:  {observed}\n      to accept the new key: dsync trust ssh rm {hostport}"
                ),
                crate::ssh::trust::SshTrustState::Unreachable(reason) => {
                    format!("  ✗ {name} ({hostport}): unreachable ({reason})")
                }
            };
            println!("{line}");
        }
    } else {
        println!("\n  (no remotes configured)");
    }

    Ok(())
}

fn check(label: &str, f: impl FnOnce() -> Status) {
    let status = f();
    let sym = match &status {
        Status::Ok => "  ✓",
        Status::Skip(_) => "  ∼",
        Status::Warn(_) => "  ✗",
    };
    let suffix = match &status {
        Status::Ok => String::new(),
        Status::Skip(r) => format!(" ({r})"),
        Status::Warn(r) => format!(" ({r})"),
    };
    println!("{sym} {label}{suffix}");
}

async fn hub_snapshot(cfg: &Config) -> Result<(StatusResponse, Option<StateStatusResponse>)> {
    let conn = crate::client::connect::connect_with_retry(cfg).await?;
    let req = crate::protocol::StatusRequest {
        machine: cfg.machine.name.clone(),
        token: crate::client::connect::hub_token(cfg),
    };
    let resp = crate::client::connect::send_status(&conn, &req).await?;
    // Старый хаб не знает `state_status` — не ошибка, просто пропускаем.
    let state = crate::client::connect::send_state_status(
        &conn,
        &crate::protocol::StateStatusRequest {
            machine: cfg.machine.name.clone(),
            token: crate::client::connect::hub_token(cfg),
        },
    )
    .await
    .ok();
    crate::client::connect::close_conn(&conn);
    Ok((resp, state))
}

fn count_tmp_files(dir: &std::path::Path) -> usize {
    let mut n = 0;
    for d in [dir.to_path_buf(), dir.join("state")] {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        n += entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .count();
    }
    n
}

fn human_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if b as f64 >= MB {
        format!("{:.1} MB", b as f64 / MB)
    } else {
        format!("{:.1} KB", b as f64 / 1024.0)
    }
}
