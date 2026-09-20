use anyhow::Result;

use crate::config::Config;

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

    println!("\nchecking hub connectivity...");
    match try_ping_hub(&cfg).await {
        Ok(_) => {}
        Err(e) => println!("  hub: {e}"),
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

async fn try_ping_hub(cfg: &Config) -> Result<()> {
    let conn = crate::client::connect::connect_with_retry(cfg).await?;
    let req = crate::protocol::StatusRequest {
        machine: cfg.machine.name.clone(),
        token: crate::client::connect::hub_token(cfg),
    };
    let resp = crate::client::connect::send_status(&conn, &req).await?;
    for (name, s) in &resp.machines {
        println!("    {name}: online={}", s.online);
    }
    Ok(())
}
