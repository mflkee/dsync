use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use teloxide::dispatching::{UpdateFilterExt, HandlerExt};
use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{BotCommand, CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode};
use tokio::sync::Mutex;

use crate::config::{Config, RemoteMachine};

#[derive(Default, Clone, Serialize, Deserialize)]
struct ChatSession {
    machine: Option<String>,
    #[serde(skip)]
    mode: Mode,
    model: Option<String>,
    #[serde(skip)]
    ctrl_msg: Option<MessageId>,
}

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
enum Mode {
    #[default]
    Exec,
    Shell,
    Oc,
}

type Sessions = Arc<Mutex<HashMap<ChatId, ChatSession>>>;

fn sessions_path() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("dsync").join("bot-sessions.json")
}

async fn load_sessions() -> HashMap<String, ChatSession> {
    let path = sessions_path();
    if !path.exists() { return HashMap::new(); }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn save_sessions(map: &HashMap<ChatId, ChatSession>) {
    let plain: HashMap<String, &ChatSession> = map.iter().map(|(k, v)| (k.0.to_string(), v)).collect();
    if let Some(dir) = sessions_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(data) = serde_json::to_string_pretty(&plain) {
        let _ = std::fs::write(sessions_path(), data);
    }
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    #[command(description = "выбрать машину")]
    Machine(String),
    #[command(description = "список машин")]
    List,
    #[command(description = "режим opencode (текст)")]
    Oc,
    #[command(description = "список моделей")]
    Models,
    #[command(description = "выбрать модель")]
    Model(String),
    #[command(description = "интерактивный терминал (tmux)")]
    Shell,
    #[command(description = "выйти из режима")]
    Stop,
    #[command(description = "помощь")]
    Help,
}

pub async fn run(cfg: Config) -> Result<()> {
    eprintln!("bot::run starting");
    let token = std::env::var("DSYNC_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("DSYNC_BOT_TOKEN not set"))?;

    let remotes: HashMap<String, RemoteMachine> = cfg.remote.clone().unwrap_or_default();
    if remotes.is_empty() {
        anyhow::bail!("no remote machines in config");
    }

    let bot = Bot::new(token);
    let remotes = Arc::new(remotes);

    // load persisted sessions
    let initial = load_sessions().await;
    let initial_map: HashMap<ChatId, ChatSession> = initial.into_iter()
        .filter_map(|(k, v)| {
            let id: i64 = k.parse().ok()?;
            Some((ChatId(id), v))
        })
        .collect();
    let sessions: Sessions = Arc::new(Mutex::new(initial_map));
    eprintln!("sessions loaded: {}", sessions.lock().await.len());

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(dptree::endpoint(handle_text)),
        )
        .branch(Update::filter_callback_query().endpoint(handle_callback));

    bot.set_my_commands([
        BotCommand::new("machine", "выбрать машину"),
        BotCommand::new("list", "список машин"),
        BotCommand::new("oc", "режим opencode (текст)"),
        BotCommand::new("models", "список моделей"),
        BotCommand::new("model", "выбрать модель"),
        BotCommand::new("shell", "интерактивный терминал"),
        BotCommand::new("stop", "выйти из режима"),
        BotCommand::new("help", "помощь"),
    ])
    .await?;

    let save_handle = sessions.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let map = save_handle.lock().await;
            save_sessions(&*map).await;
        }
    });

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![remotes, sessions])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

// ── helpers ──────────────────────────────────────────

fn machine_keyboard(remotes: &HashMap<String, RemoteMachine>) -> InlineKeyboardMarkup {
    let rows: Vec<Vec<InlineKeyboardButton>> = remotes
        .keys()
        .map(|n| vec![InlineKeyboardButton::callback(n.clone(), format!("machine:{n}"))])
        .collect();
    InlineKeyboardMarkup::new(rows)
}

fn shell_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⬆", "key:up"),
            InlineKeyboardButton::callback("⬇", "key:down"),
            InlineKeyboardButton::callback("↵", "key:enter"),
        ],
        vec![
            InlineKeyboardButton::callback("ESC", "key:esc"),
            InlineKeyboardButton::callback("⏹ stop", "mode:stop"),
        ],
    ])
}

fn rm(cfg: &HashMap<String, RemoteMachine>, name: &str) -> Option<RemoteMachine> {
    cfg.get(name).cloned()
}

async fn ssh(host: &str, port: u16, user: &str, cmd: &str) -> Result<String> {
    crate::ssh::client::exec(host, port, user, cmd).await
}

fn sh_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn strip_ansi(s: &str) -> String {
    // Quick ANSI escape sequence remover
    let mut out = String::with_capacity(s.len());
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' || c.is_ascii_alphabetic() { esc = false; }
            continue;
        }
        if c == '\x1b' { esc = true; continue; }
        out.push(c);
    }
    out
}

// ── command handler ──────────────────────────────────

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    remotes: Arc<HashMap<String, RemoteMachine>>,
    sessions: Sessions,
) -> Result<()> {
    eprintln!("handle_command: text={:?} chat={}", msg.text(), msg.chat.id.0);
    let mut map = sessions.lock().await;
    let s = map.entry(msg.chat.id).or_default();

    match cmd {
        Command::Help => {
            bot.send_message(msg.chat.id,
                "dsync bot — машины через Telegram\n\n\
                /machine — выбрать машину\n/machine <имя> — напрямую\n\
                /list — список машин\n\
                /oc — режим opencode (промт → текст)\n\
                /models — список моделей\n/model <имя> — выбрать модель\n\
                /shell — интерактивный терминал (tmux, кнопки)\n\
                /stop — выйти из режима\n\
                /help — помощь").await?;
        }
        Command::List => {
            let mut text = String::from("машины:\n");
            for name in remotes.keys() {
                let cur = s.machine.as_deref() == Some(name.as_str());
                text.push_str(&format!("  • {name}{}\n", if cur { " ◀" } else { "" }));
            }
            bot.send_message(msg.chat.id, text).await?;
        }
        Command::Machine(name) => {
            if name.trim().is_empty() {
                bot.send_message(msg.chat.id, "выбери машину:")
                    .reply_markup(machine_keyboard(&remotes)).await?;
            } else if remotes.contains_key(name.trim()) {
                s.machine = Some(name.trim().to_string());
                bot.send_message(msg.chat.id, format!("✓ {name}")).await?;
            } else {
                bot.send_message(msg.chat.id, format!("«{name}» не найдена")).await?;
            }
        }
        Command::Models => {
            let m = match &s.machine {
                Some(m) => m.clone(),
                None => { bot.send_message(msg.chat.id, "сначала /machine").await?; return Ok(()); }
            };
            let r = match rm(&remotes, &m) {
                Some(r) => r,
                None => { bot.send_message(msg.chat.id, "машина не найдена").await?; return Ok(()); }
            };
            let out = ssh(&r.host, r.port, &r.user, "opencode models 2>&1").await?;
            let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("opencode/")).collect();
            let mut rows = Vec::new();
            for line in &lines {
                let name = line.trim();
                rows.push(vec![InlineKeyboardButton::callback(name.to_string(), format!("model:{name}"))]);
            }
            let kb = InlineKeyboardMarkup::new(rows);
            bot.send_message(msg.chat.id, format!("модели opencode ({}):", lines.len()))
                .reply_markup(kb).await?;
        }
        Command::Model(name) => {
            if name.trim().is_empty() {
                bot.send_message(msg.chat.id, "/model anthropic/claude-opus-4-8").await?;
            } else {
                s.model = Some(name.trim().to_string());
                bot.send_message(msg.chat.id, format!("✓ модель: {}", name.trim())).await?;
            }
        }
        Command::Oc => {
            if s.machine.is_none() { bot.send_message(msg.chat.id, "сначала /machine").await?; return Ok(()); }
            s.mode = Mode::Oc;
            bot.send_message(msg.chat.id, "✓ opencode режим\nпиши промт → ответ текстом\n/stop — выход").await?;
        }
        Command::Shell => {
            let m = match &s.machine {
                Some(m) => m.clone(),
                None => { bot.send_message(msg.chat.id, "сначала /machine").await?; return Ok(()); }
            };
            let r = match rm(&remotes, &m) {
                Some(r) => r,
                None => { bot.send_message(msg.chat.id, "машина не найдена").await?; return Ok(()); }
            };
            let session = format!("dsync-bot-{}", msg.chat.id.0);
            let _ = ssh(&r.host, r.port, &r.user,
                &format!("TERM=xterm-256color tmux new-session -d -x 100 -y 30 -s {session} 2>&1 || true")).await;
            s.mode = Mode::Shell;
            let ctrl = bot.send_message(msg.chat.id, "✓ shell режим (кнопки внизу)")
                .reply_markup(shell_keyboard()).await?;
            s.ctrl_msg = Some(ctrl.id);
        }
        Command::Stop => {
            let old = s.mode;
            s.mode = Mode::Exec;
            if old == Mode::Shell {
                if let (Some(m), Some(ctrl)) = (&s.machine, s.ctrl_msg) {
                    if let Some(r) = rm(&remotes, m) {
                        let session = format!("dsync-bot-{}", msg.chat.id.0);
                        let _ = ssh(&r.host, r.port, &r.user,
                            &format!("tmux kill-session -t {session} 2>/dev/null || true")).await;
                    }
                    let _ = bot.edit_message_reply_markup(msg.chat.id, ctrl).reply_markup(InlineKeyboardMarkup::default()).await;
                }
            }
            s.ctrl_msg = None;
            bot.send_message(msg.chat.id, "exec режим").await?;
        }
    }
    Ok(())
}

async fn persist_sessions(sessions: &Sessions) {
    let map = sessions.lock().await;
    save_sessions(&map).await;
}

// ── callback handler ─────────────────────────────────

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    remotes: Arc<HashMap<String, RemoteMachine>>,
    sessions: Sessions,
) -> Result<()> {
    eprintln!("handle_callback: data={:?}", q.data);
    let data = match &q.data {
        Some(d) => d,
        None => return Ok(()),
    };
    let msg = match &q.message {
        Some(m) => m,
        None => return Ok(()),
    };

    if let Some(name) = data.strip_prefix("machine:") {
        sessions.lock().await.entry(msg.chat.id).or_default().machine = Some(name.to_string());
        bot.edit_message_text(msg.chat.id, msg.id, format!("✓ {name}")).await?;
    } else if let Some(name) = data.strip_prefix("model:") {
        sessions.lock().await.entry(msg.chat.id).or_default().model = Some(name.to_string());
        bot.edit_message_text(msg.chat.id, msg.id, format!("✓ модель: {name}")).await?;
    } else if let Some(key) = data.strip_prefix("key:") {
        let chat_id = msg.chat.id;
        let m = sessions.lock().await.get(&chat_id).and_then(|s| s.machine.clone());
        let r = m.as_ref().and_then(|m| rm(&remotes, m));
        let ctrl_id = sessions.lock().await.get(&chat_id).and_then(|s| s.ctrl_msg);
        if let (Some(machine), Some(remote)) = (m, r) {
            let session = format!("dsync-bot-{chat_id}");
            let _ = ssh(&remote.host, remote.port, &remote.user,
                &format!("tmux send-keys -t {session} {key}")).await;
            let out = ssh(&remote.host, remote.port, &remote.user,
                &format!("sleep 0.3 && tmux capture-pane -t {session} -p -S -50")).await.unwrap_or_default();
            if let Some(ctrl) = ctrl_id {
                let cleaned = out.trim();
                let reply = if cleaned.is_empty() { "⏳".to_string() }
                    else if cleaned.len() > 3500 { format!("```\n{}...\n```", &cleaned[..3500]) }
                    else { format!("```\n{cleaned}\n```") };
                let _ = bot.edit_message_text(chat_id, ctrl, reply)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(shell_keyboard()).await;
            }
        }
    } else if data == "mode:stop" {
        let _ = handle_command(bot.clone(), msg.clone(), Command::Stop,
            remotes.clone(), sessions.clone()).await;
    }

    bot.answer_callback_query(q.id).await?;
    Ok(())
}

// ── text handler ─────────────────────────────────────

async fn handle_text(
    bot: Bot,
    msg: Message,
    remotes: Arc<HashMap<String, RemoteMachine>>,
    sessions: Sessions,
) -> Result<()> {
    eprintln!("handle_text: text={:?}", msg.text());
    let text = match msg.text() {
        Some(t) => t,
        None => return Ok(()),
    };

    let (machine, mode, model) = {
        let map = sessions.lock().await;
        let s = map.get(&msg.chat.id);
        match s {
            Some(s) => (s.machine.clone(), s.mode, s.model.clone()),
            None => { bot.send_message(msg.chat.id, "сначала /machine").await?; return Ok(()); }
        }
    };
    let machine = match machine {
        Some(m) => m,
        None => { bot.send_message(msg.chat.id, "сначала /machine").await?; return Ok(()); }
    };
    let r = match rm(&remotes, &machine) {
        Some(r) => r,
        None => { bot.send_message(msg.chat.id, "машина не найдена").await?; return Ok(()); }
    };

    match mode {
        Mode::Oc => {
            let model_flag = model.as_ref()
                .map(|m| format!(" -m {}", sh_escape(m)))
                .unwrap_or_default();
            let sent = bot.send_message(msg.chat.id, "⏳ opencode думает...").await?;
            match tokio::time::timeout(
                std::time::Duration::from_secs(90),
                ssh(&r.host, r.port, &r.user,
                    &format!("timeout 60 opencode run{model_flag} {} 2>&1", sh_escape(text))),
            ).await
            {
                Ok(Ok(out)) => {
                    eprintln!("oc: ssh ok, len={}", out.len());
                    let cleaned: String = out.lines()
                        .filter(|l| !l.trim().starts_with('>'))
                        .map(|l| strip_ansi(l))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        .to_string();
                    let reply = if cleaned.is_empty() { "(пустой ответ)".to_string() }
                        else if cleaned.len() > 4000 {
                            cleaned.chars().take(4000).collect::<String>()
                        } else { cleaned };
                    eprintln!("oc: editing msg {} with reply len={}", sent.id.0, reply.len());
                    bot.edit_message_text(msg.chat.id, sent.id, reply).await?;
                    eprintln!("oc: edit ok");
                }
                Ok(Err(e)) => {
                    eprintln!("oc: ssh error: {e}");
                    bot.edit_message_text(msg.chat.id, sent.id, format!("✗ {e}")).await.ok();
                }
                Err(_) => {
                    eprintln!("oc: tokio timeout");
                    bot.edit_message_text(msg.chat.id, sent.id, "✗ таймаут (90с)".to_string()).await.ok();
                }
            }
        }
        Mode::Shell => {
            let session = format!("dsync-bot-{}", msg.chat.id.0);
            let escaped = sh_escape(text);
            let cmd = format!(
                "tmux send-keys -t {session} {escaped} Enter && sleep 0.5 && tmux capture-pane -t {session} -p -S -50"
            );
            let ctrl_id = sessions.lock().await.get(&msg.chat.id).and_then(|s| s.ctrl_msg);
            let result = ssh(&r.host, r.port, &r.user, &cmd).await;
            let reply = match &result {
                Ok(out) => {
                    let cleaned = out.trim();
                    if cleaned.is_empty() { "(пусто)".to_string() }
                    else if cleaned.len() > 3000 {
                        cleaned.chars().take(3000).collect::<String>()
                    } else { cleaned.to_string() }
                }
                Err(e) => format!("✗ {e}"),
            };
            if let Some(ctrl) = ctrl_id {
                bot.edit_message_text(msg.chat.id, ctrl, reply)
                    .reply_markup(shell_keyboard()).await?;
            } else {
                let sent = bot.send_message(msg.chat.id, reply)
                    .reply_markup(shell_keyboard()).await?;
                sessions.lock().await.entry(msg.chat.id).or_default().ctrl_msg = Some(sent.id);
            }
        }
        Mode::Exec => {
            let sent = bot.send_message(msg.chat.id, "⏳").await?;
            match ssh(&r.host, r.port, &r.user, text).await {
                Ok(out) => {
                    let cleaned = out.trim();
                    let reply = if cleaned.is_empty() { "✓ (пусто)".to_string() }
                        else if cleaned.len() > 3500 {
                            format!("```\n{}...\n```", &cleaned[..3500])
                        } else { format!("```\n{cleaned}\n```") };
                    bot.edit_message_text(msg.chat.id, sent.id, reply)
                        .parse_mode(ParseMode::MarkdownV2).await?;
                }
                Err(e) => {
                    bot.edit_message_text(msg.chat.id, sent.id, format!("✗\n```\n{e}\n```"))
                        .parse_mode(ParseMode::MarkdownV2).await?;
                }
            }
        }
    }

    Ok(())
}
