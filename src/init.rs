use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Result};
use dialoguer::{Confirm, Input, MultiSelect, Select};

use crate::config::{
    Config, HubConfig, HubConnectConfig, MachineConfig, ProjectConfig, RemoteMachine,
};

/// Интерактивный мастер первого запуска (`dsync init`).
///
/// Спрашивает роль машины (клиент/хаб/оба), имя, адрес хаба, SSH-ключ,
/// машины флота, проекты и планировщик, затем пишет config.toml
/// и (на Linux) systemd user unit для `dsync watch`.
pub fn run() -> Result<()> {
    // Не запускаемся без TTY (в CI / скриптах просто падаем с понятной ошибкой).
    if !std::io::stdin().is_terminal() {
        bail!("dsync init is interactive and needs a terminal; run it manually");
    }

    if let Some(path) = crate::config::find_config_path() {
        let overwrite = Confirm::new()
            .with_prompt(format!(
                "config already exists at {}, overwrite it?",
                path.display()
            ))
            .default(false)
            .interact()?;
        if !overwrite {
            println!("keeping existing config — nothing changed");
            return Ok(());
        }
    }

    println!("🧭 dsync init — walk through fleet setup\n");

    // 1. Роль машины.
    let roles = [
        "client (connects to a hub)",
        "hub + client (runs the hub AND syncs)",
        "hub only (server, no local projects)",
    ];
    let role_idx = Select::new()
        .with_prompt("What is this machine's role in the fleet?")
        .items(&roles)
        .default(0)
        .interact()?;
    let is_hub = role_idx > 0;
    let is_client = role_idx != 2;

    // 2. Имя машины.
    let hostname = hostname();
    let name: String = Input::new()
        .with_prompt("Machine name")
        .default(hostname)
        .interact_text()?;

    // 3. Hub адрес (строка — конфиг с токеном собираем после генерации токенов).
    let mut hub_addr = None;
    if is_client {
        let default_addr = if is_hub {
            "127.0.0.1:42069".to_string()
        } else {
            String::new()
        };
        let addr: String = Input::new()
            .with_prompt("Hub address (host:port, inside your private network)")
            .default(default_addr)
            .allow_empty(false)
            .interact_text()?;
        hub_addr = Some(addr);
    }

    // 4. SSH-ключ: существующий или генерируем ed25519.
    let ssh_key = ssh_key_setup()?;

    // 5. Машины флота (remote.*).
    let remotes = remote_setup()?;

    // 6. Токены хаба: по одному на каждую машину флота + на себя.
    let machine_tokens = generate_fleet_tokens(&remotes, &name, is_hub || is_client);

    // 7. [hub] — только если машина — хаб.
    let hub = if is_hub {
        let bind: String = Input::new()
            .with_prompt("Hub bind address")
            .default("0.0.0.0:42069".to_string())
            .interact_text()?;
        let data_dir: String = Input::new()
            .with_prompt("Hub data dir (state + persisted TLS cert)")
            .default(
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("./.dsync-data"))
                    .join("dsync-hub")
                    .display()
                    .to_string(),
            )
            .interact_text()?;
        Some(HubConfig {
            bind,
            cert: None,
            key: None,
            data_dir: Some(data_dir.into()),
            tokens: machine_tokens.clone(),
            max_message_size: crate::config::default_max_message_size(),
            max_concurrency: crate::config::default_max_concurrency(),
            retention_days: crate::config::default_retention_days(),
            pull_retries: crate::config::default_pull_retries(),
        })
    } else {
        None
    };

    if is_hub {
        println!("\n── machine tokens (hub auth) ──");
        for (m, t) in &machine_tokens {
            println!("{m}: {t}");
        }
        println!("add [hub_connect] token on each client machine (hub+client already has it)");
    }

    // 8. hub_connect: для hub+client токен берём из сгенерированных,
    //    для чистого клиента — спрашиваем явно.
    let hub_connect = match hub_addr {
        None => None,
        Some(addr) => {
            let token = match &hub {
                Some(h) => h.tokens.get(&name).cloned().unwrap_or_default(),
                None => Input::new()
                    .with_prompt(
                        "Hub token (from the hub's [hub] tokens; empty = set later, \
                         `dsync doctor` will remind you)",
                    )
                    .allow_empty(true)
                    .interact_text()?,
            };
            Some(HubConnectConfig { address: addr, token })
        }
    };

    // 9. Проекты.
    let projects = projects_setup(&remotes)?;

    // 10. Планировщик.
    let scheduler = scheduler_setup()?;

    // Собираем конфиг.
    let cfg = Config {
        config_version: crate::config::CONFIG_VERSION,
        machine: MachineConfig {
            name,
            ssh_key: Some(ssh_key),
        },
        hub,
        hub_connect,
        projects: if projects.is_empty() {
            None
        } else {
            Some(projects)
        },
        remote: if remotes.is_empty() {
            None
        } else {
            Some(remotes)
        },
        capture: None,
    };

    // Показываем итог и пишем.
    let target = default_config_path();
    println!("\n───── generated config ─────\n");
    let rendered = toml::to_string_pretty(&cfg)?;
    println!("{rendered}");
    println!("────────────────────────────");

    let write = Confirm::new()
        .with_prompt(format!("Write to {}?", target.display()))
        .default(true)
        .interact()?;
    if !write {
        println!("aborted — nothing written");
        return Ok(());
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        &target,
        format!("# dsync config — generated by `dsync init`\n{rendered}"),
    )?;
    println!("✓ wrote {}", target.display());

    if is_hub && scheduler.as_deref() == Some("systemd") {
        println!("\nRun the hub now:  dsync hub");
        println!("(or restart dsync-hub.service if you used an existing one)");
    }
    println!("\nNext steps:\n\
              1. Make sure the hub can SSH into each machine:\n      ssh-copy-id <user>@<host>   # on the hub\n\
              2. From any client:  dsync push   → hub pulls + applies everywhere\n\
              3. Check health:     dsync doctor,  dsync status,  dsync tui");
    Ok(())
}

/// Генерирует по токену на каждую машину флота + на саму машину (если она
/// клиент-роль). Возвращает map machine → token.
fn generate_fleet_tokens(
    remotes: &std::collections::HashMap<String, RemoteMachine>,
    self_name: &str,
    include_self: bool,
) -> std::collections::HashMap<String, String> {
    let mut members: Vec<String> = remotes.keys().cloned().collect();
    if include_self {
        members.push(self_name.to_string());
    }
    members.sort();
    members.dedup();
    members
        .into_iter()
        .map(|m| (m, generate_token()))
        .collect()
}

/// Случайный 32-hex токен (uuid v4, без дефисов).
fn generate_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Имя машины: hostname через `hostname` (UNIX) / COMPUTERNAME (Windows), fallback "localhost".
fn hostname() -> String {
    #[cfg(unix)]
    {
        if let Ok(out) = std::process::Command::new("hostname").output() {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    let t = s.trim();
                    if !t.is_empty() {
                        return t.to_string();
                    }
                }
            }
        }
    }
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// SSH-ключ: используем существующий или генерируем ed25519.
fn ssh_key_setup() -> Result<PathBuf> {
    let default_key = dirs::home_dir()
        .unwrap_or_default()
        .join(".ssh")
        .join("id_ed25519");
    let exists = default_key.exists();

    if exists {
        let use_default = Confirm::new()
            .with_prompt(format!(
                "SSH key found at {} — use it?",
                default_key.display()
            ))
            .default(true)
            .interact()?;
        if use_default {
            return Ok(default_key);
        }
    }

    let path: String = Input::new()
        .with_prompt("Path to SSH private key")
        .default(default_key.display().to_string())
        .interact_text()?;
    let path = PathBuf::from(path);

    if path.exists() {
        println!("✓ using existing key {}", path.display());
        return Ok(path);
    }

    let gen = Confirm::new()
        .with_prompt(format!("Generate new ed25519 key at {}?", path.display()))
        .default(true)
        .interact()?;
    if !gen {
        bail!("no SSH key available — dsync needs one for hub→machine pulls");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let status = std::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-f"])
        .arg(&path)
        .status()?;
    if !status.success() {
        bail!("ssh-keygen failed; check that OpenSSH tools are installed");
    }
    println!("✓ generated key {}", path.display());
    // chmod 600 как надёжная защита (не у всех umask ок).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn remote_setup() -> Result<std::collections::HashMap<String, RemoteMachine>> {
    let mut remotes = std::collections::HashMap::new();
    println!("\n── machines of the fleet ──");
    println!("Add each machine the hub will pull to (clients AND the hub itself).");

    loop {
        let name: String = Input::new()
            .with_prompt("Machine name (e.g. desktop)")
            .allow_empty(false)
            .interact_text()?;
        let host: String = Input::new()
            .with_prompt(format!("{name}: SSH host/IP"))
            .allow_empty(false)
            .interact_text()?;
        let port: u16 = Input::new()
            .with_prompt(format!("{name}: SSH port"))
            .default(22)
            .interact_text()?;
        let default_user = std::env::var("USER").unwrap_or_else(|_| "root".into());
        let user: String = Input::new()
            .with_prompt(format!("{name}: SSH user"))
            .default(default_user)
            .interact_text()?;
        remotes.insert(name, RemoteMachine { host, port, user });
        let more = Confirm::new()
            .with_prompt("Add another machine?")
            .default(false)
            .interact()?;
        if !more {
            break;
        }
    }
    if remotes.is_empty() {
        println!("⚠ no machines added — dsync can still push local state, but nothing will be pulled remotely");
    }
    Ok(remotes)
}

fn projects_setup(
    remotes: &std::collections::HashMap<String, RemoteMachine>,
) -> Result<std::collections::HashMap<String, ProjectConfig>> {
    let mut projects = std::collections::HashMap::new();
    println!("\n── projects ──");
    println!("Git repos to keep in sync (e.g. your dotfiles). Each needs its own git origin.");

    loop {
        let name: String = Input::new()
            .with_prompt("Project name (e.g. dotfiles)")
            .default(match projects.is_empty() {
                true => "dotfiles".to_string(),
                false => "project".to_string(),
            })
            .allow_empty(false)
            .interact_text()?;
        let path: String = Input::new()
            .with_prompt(format!("{name}: local path"))
            .allow_empty(false)
            .interact_text()?;
        let branch: String = Input::new()
            .with_prompt(format!("{name}: git branch"))
            .default("main".to_string())
            .interact_text()?;
        let machines = if remotes.is_empty() {
            Vec::new()
        } else {
            let keys: Vec<String> = remotes.keys().cloned().collect();
            println!("{name}: machines to sync on? (space=select, enter=continue)");
            let sel: Vec<usize> = MultiSelect::new()
                .with_prompt("  sync machines")
                .items(&keys)
                .defaults(&vec![true; keys.len()])
                .interact()?;
            sel.iter().map(|i| keys[*i].clone()).collect()
        };
        let post_pull: String = Input::new()
            .with_prompt(format!("{name}: post_pull command (optional)"))
            .allow_empty(true)
            .interact_text()?;
        let mut project = ProjectConfig {
            path: path.into(),
            branch: Some(branch),
            machines: None,
            post_pull: None,
        };
        if !machines.is_empty() {
            project.machines = Some(machines);
        }
        if !post_pull.is_empty() {
            project.post_pull = Some(post_pull);
        }
        projects.insert(name, project);

        let more = Confirm::new()
            .with_prompt("Add another project?")
            .default(false)
            .interact()?;
        if !more {
            break;
        }
    }
    if projects.is_empty() {
        bail!("at least one project is required");
    }
    Ok(projects)
}

fn scheduler_setup() -> Result<Option<String>> {
    println!("\n── scheduler ──");
    let choices = if cfg!(target_os = "linux") {
        vec![
            "systemd user timer (Linux, recommended)",
            "dsync watch (background loop, any OS)",
            "none (run dsync manually)",
        ]
    } else {
        vec![
            "dsync watch (background loop, any OS)",
            "none (run dsync manually)",
        ]
    };
    let idx = Select::new()
        .with_prompt("How should dsync run periodically?")
        .items(&choices)
        .default(0)
        .interact()?;

    let choice = choices[idx];
    if choice.starts_with("systemd") {
        install_systemd_timer()?;
        Ok(Some("systemd".into()))
    } else if choice.starts_with("dsync watch") {
        println!("▶ Run in the background:  dsync watch --interval 900");
        Ok(Some("watch".into()))
    } else {
        println!("▶ Run manually when you need it:  dsync push / dsync pull");
        Ok(None)
    }
}

/// Устанавливает systemd user unit `dsync-watch.service` + `.timer`
/// (каждые 15 минут). Возвращает true при успехе.
#[cfg(target_os = "linux")]
fn install_systemd_timer() -> Result<bool> {
    let unit_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("systemd")
        .join("user");
    let service = unit_dir.join("dsync-watch.service");
    let timer = unit_dir.join("dsync-watch.timer");

    let data_dir = dirs::data_dir().unwrap_or_default().join("dsync");
    let binary = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("dsync"))
        .display()
        .to_string();

    let service_unit = format!(
        "[Unit]\n\
         Description=dsync watch — periodic fleet sync\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart={binary} watch --interval 900\n\
         Restart=on-failure\n\
         RestartSec=30\n"
    );
    let timer_unit = "[Unit]\n\
         Description=dsync watch timer\n\n\
         [Timer]\n\
         OnBootSec=1min\n\
         OnUnitActiveSec=15min\n\
         Persistent=true\n\n\
         [Install]\n\
         WantedBy=timers.target\n";

    std::fs::create_dir_all(&unit_dir)?;
    let _ = std::fs::create_dir_all(&data_dir);
    std::fs::write(&service, service_unit)?;
    std::fs::write(&timer, timer_unit)?;
    println!(
        "✓ wrote systemd units: {} / {}",
        service.display(),
        timer.display()
    );
    println!("  enable:  systemctl --user daemon-reload && systemctl --user enable --now dsync-watch.timer");
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
fn install_systemd_timer() -> Result<bool> {
    Ok(true)
}

/// Целевой путь конфига: `~/.config/dsync/dsync/config.toml` (или XDG).
fn default_config_path() -> PathBuf {
    directories::ProjectDirs::from("com", "mflkee", "dsync")
        .map(|d| d.config_dir().join("dsync/config.toml"))
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("dsync/dsync/config.toml")
        })
}

/// Проверка, что путь (после ~-раскрытия) попадает в ожидаемую директорию — help для тестов.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_target_is_under_config_dir() {
        let p = default_config_path();
        assert!(p.to_string_lossy().ends_with("dsync/config.toml"));
    }

    #[test]
    fn hostname_falls_back() {
        let h = hostname();
        assert!(!h.is_empty());
    }

    #[test]
    fn config_serializes_roundtrip() {
        let cfg = Config {
            config_version: 1,
            machine: MachineConfig {
                name: "desktop".into(),
                ssh_key: Some(PathBuf::from("~/.ssh/id_ed25519")),
            },
            hub: None,
            hub_connect: Some(HubConnectConfig {
                address: "10.0.0.5:42069".into(),
                token: "tok-local".into(),
            }),
            projects: Some(
                [(
                    "dotfiles".to_string(),
                    ProjectConfig {
                        path: "~/dotfiles".into(),
                        branch: Some("main".into()),
                        machines: Some(vec!["desktop".into(), "notebook".into()]),
                        post_pull: Some("chezmoi apply".into()),
                    },
                )]
                .into_iter()
                .collect(),
            ),
            remote: Some(
                [(
                    "notebook".to_string(),
                    RemoteMachine {
                        host: "10.0.0.6".into(),
                        port: 22,
                        user: "me".into(),
                    },
                )]
                .into_iter()
                .collect(),
            ),
            capture: None,
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.machine.name, "desktop");
        assert_eq!(back.hub_connect.unwrap().address, "10.0.0.5:42069");
        let projects = back.projects.as_ref().unwrap();
        assert_eq!(
            projects["dotfiles"].machines.as_ref().unwrap()[1],
            "notebook"
        );
    }

    #[test]
    fn fleet_tokens_cover_members_and_self() {
        let mut remotes = std::collections::HashMap::new();
        remotes.insert(
            "notebook".to_string(),
            RemoteMachine {
                host: "10.0.0.6".into(),
                port: 22,
                user: "me".into(),
            },
        );
        let toks = generate_fleet_tokens(&remotes, "desktop", true);
        assert_eq!(toks.len(), 2, "notebook + self");
        assert!(toks.contains_key("desktop"));
        assert!(toks.contains_key("notebook"));
        assert_ne!(toks["desktop"], toks["notebook"], "tokens are per-machine");
        // 32 hex chars (uuid v4 simple).
        for t in toks.values() {
            assert_eq!(t.len(), 32);
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn hub_roundtrip_keeps_tokens() {
        let hub = HubConfig {
            bind: "0.0.0.0:42069".into(),
            cert: None,
            key: None,
            data_dir: None,
            tokens: [("desktop".to_string(), "tok-a".to_string())].into_iter().collect(),
            max_message_size: 1024,
            max_concurrency: 2,
            retention_days: 7,
            pull_retries: 1,
        };
        let cfg = Config {
            config_version: 1,
            machine: MachineConfig {
                name: "desktop".into(),
                ssh_key: None,
            },
            hub: Some(hub),
            hub_connect: Some(HubConnectConfig {
                address: "127.0.0.1:42069".into(),
                token: "tok-a".into(),
            }),
            projects: None,
            remote: None,
            capture: None,
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.hub.unwrap().tokens["desktop"], "tok-a");
        assert_eq!(back.hub_connect.unwrap().token, "tok-a");
    }
}
