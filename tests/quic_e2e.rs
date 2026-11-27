//! End-to-end: реальный dsync-hub на loopback + клиентские команды по QUIC.

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_dsync")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn tmp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dsync-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

fn toml_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

struct Hub(Child);

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_hub(cfg: &Path, cwd: &Path) -> Hub {
    let child = Command::new(bin())
        .arg("hub")
        .current_dir(cwd)
        .env("DSYNC_CONFIG", cfg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dsync hub");
    Hub(child)
}

fn write_config(path: &Path, data_dir: &Path, port: u16) {
    let mut f = std::fs::File::create(path).expect("config");
    write!(
        f,
        r#"config_version = 1

[machine]
name = "e2e"

[hub]
bind = "127.0.0.1:{port}"
data_dir = "{}"
tokens = {{ e2e = "e2e-token" }}

[hub_connect]
address = "127.0.0.1:{port}"
token = "e2e-token"
"#,
        toml_path(data_dir)
    )
    .expect("write config");
}

#[test]
fn hub_client_roundtrip_over_quic() {
    let dir = tmp_dir("quic");
    std::fs::create_dir_all(&dir).expect("work dir");
    let cfg = dir.join("dsync.toml");
    let data = dir.join("data");
    let port = free_port();
    write_config(&cfg, &data, port);

    let _hub = spawn_hub(&cfg, &dir);
    // Хаб поднимает QUIC-endpoint и сертификаты за доли секунды; на медленном
    // CI даём запас. Клиентские команды ниже всё равно ретраят connect.
    std::thread::sleep(Duration::from_secs(2));

    let run = |args: &[&str]| {
        Command::new(bin())
            .args(args)
            .current_dir(&dir)
            .env("DSYNC_CONFIG", &cfg)
            .output()
            .expect("run dsync")
    };

    let push = run(&["push"]);
    assert!(
        push.status.success(),
        "push failed: {}\n{}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );

    let status = run(&["status", "--json"]);
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("status --json is valid JSON");
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(
        doc["machines"]["e2e"]["online"], true,
        "pushed machine is online: {doc}"
    );

    let pull = run(&["pull"]);
    assert!(
        pull.status.success(),
        "pull failed: {}",
        String::from_utf8_lossy(&pull.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
