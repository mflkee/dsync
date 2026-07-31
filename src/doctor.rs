use std::path::Path;

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

    check("ssh key", || {
        let key = dirs::home_dir()
            .map(|h| h.join(".ssh/id_ed25519"))
            .unwrap_or_default();
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

    if let Some(ref zen) = cfg.zen {
        check("zen profile", || {
            let path = crate::projects::status::expand_user_path(&zen.profile_path);
            if path.exists() {
                Status::Ok
            } else {
                Status::Warn(format!("profile_path not found: {}", path.display()))
            }
        });
    }

    println!("\nchecking hub connectivity...");
    match try_ping_hub(&cfg).await {
        Ok(_) => {}
        Err(e) => println!("  hub: {e}"),
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
    };
    let resp = crate::client::connect::send_status(&conn, &req).await?;
    for (name, s) in &resp.machines {
        println!("    {name}: online={}", s.online);
    }
    Ok(())
}
