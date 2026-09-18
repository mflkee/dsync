mod bot;
mod cli;
mod client;
mod config;
mod doctor;
mod hub;
mod projects;
mod protocol;
mod ssh;
mod tui;

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = cli::Cli::parse();
    let tui_mode = matches!(&args.command, cli::Commands::Tui);
    init_tracing(tui_mode);

    let cfg = config::Config::load()?;

    match args.command {
        cli::Commands::Hub => hub::run_server(cfg).await,
        cli::Commands::Watch { interval } => client::watch(cfg, interval).await,
        cli::Commands::Push { machine } => {
            print_lines(client::push(cfg, machine).await?);
            Ok(())
        }
        cli::Commands::Pull { machine } => {
            print_lines(client::pull(cfg, machine).await?);
            Ok(())
        }
        cli::Commands::Status => {
            print_lines(client::status(cfg).await?);
            Ok(())
        }
        cli::Commands::Doctor => doctor::run(cfg).await,
        cli::Commands::Bot => bot::run(cfg).await,
        cli::Commands::Tui => tui::run(cfg),
    }
}

/// В TUI лог tracing'а уходит в файл `~/.local/share/dsync/tui.log`, иначе
/// info-строки из фоновых задач (connect attempt, starting push…) писались
/// бы в stdout и затирали экран ratatui.
fn init_tracing(tui_mode: bool) {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "dsync=info".into())
    };

    if !tui_mode {
        tracing_subscriber::fmt().with_env_filter(filter()).init();
        return;
    }

    let dir = dirs::data_dir()
        .map(|d| d.join("dsync"))
        .unwrap_or_else(|| PathBuf::from("/tmp/dsync"));
    let open = || {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("tui.log"))
            .ok()
    };

    if let Some(file) = open() {
        tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_ansi(false)
            .with_writer(FileWriter(Mutex::new(file)))
            .init();
    } else {
        // Не смогли открыть файл — глушим вывод, чтобы не ломать экран.
        let sink = std::fs::OpenOptions::new()
            .append(true)
            .open("/dev/null")
            .expect("open /dev/null");
        tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_ansi(false)
            .with_writer(FileWriter(Mutex::new(sink)))
            .init();
    }
}

/// Тонкая обёртка: tracing-subscriber требует MakeWriter, файл им не является.
struct FileWriter(Mutex<std::fs::File>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FileWriter {
    type Writer = std::fs::File;

    fn make_writer(&'a self) -> Self::Writer {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_clone()
            .expect("try_clone on tui.log")
    }
}

/// Печатает строки результата (push/pull/status возвращают линии вместо
/// печати в stdout — так те же функции безопасно вызываются из TUI).
fn print_lines(lines: Vec<String>) {
    for l in lines {
        println!("{l}");
    }
}
