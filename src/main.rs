use std::{error::Error, io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{execute, terminal::EnterAlternateScreen, terminal::LeaveAlternateScreen};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use openai::chat::{
    ChatCompletion, ChatCompletionDelta, ChatCompletionMessage, ChatCompletionMessageRole,
};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

enum Status {
    Idle,
    Requesting,
    Error(String),
}

enum AppEvent {
    Status(Status),
    System(String),
}

type CmdResult = Result<String, Box<dyn Error>>;
type CmdFuture = Pin<Box<dyn Future<Output = CmdResult> + Send>>;

trait ToolCommand: Send {
    fn execute(&self) -> CmdFuture;
}

struct TouchCommand {
    path: String,
}

impl ToolCommand for TouchCommand {
    fn execute(&self) -> CmdFuture {
        let path = self.path.clone();
        Box::pin(async move {
            use tokio::fs;
            fs::write(path, "").await?;
            Ok("文件已创建".to_string())
        })
    }
}

struct RmCommand {
    path: String,
}

impl ToolCommand for RmCommand {
    fn execute(&self) -> CmdFuture {
        let path = self.path.clone();
        Box::pin(async move {
            use tokio::fs;
            fs::remove_file(path).await?;
            Ok("文件已删除".to_string())
        })
    }
}

struct WriteCommand {
    path: String,
    content: String,
}

impl ToolCommand for WriteCommand {
    fn execute(&self) -> CmdFuture {
        let path = self.path.clone();
        let content = self.content.clone();
        Box::pin(async move {
            use tokio::fs;
            fs::write(path, content).await?;
            Ok("文件已写入".to_string())
        })
    }
}

struct FindCommand {
    pattern: String,
}

impl ToolCommand for FindCommand {
    fn execute(&self) -> CmdFuture {
        let pattern = self.pattern.clone();
        Box::pin(async move {
            use tokio::fs;
            let cwd = std::env::current_dir()?;
            let mut found: Vec<String> = Vec::new();
            let mut stack = vec![cwd];
            while let Some(dir) = stack.pop() {
                let mut rd = fs::read_dir(&dir).await?;
                while let Some(ent) = rd.next_entry().await? {
                    let p = ent.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                        if name.contains(&pattern) {
                            found.push(p.display().to_string());
                        }
                    }
                }
            }
            Ok(format!(
                "匹配到 {} 个文件:\n{}",
                found.len(),
                found.join("\n")
            ))
        })
    }
}

struct EditAtCommand {
    path: String,
    line: usize,
    col: usize,
    content: String,
}

impl ToolCommand for EditAtCommand {
    fn execute(&self) -> CmdFuture {
        let path = self.path.clone();
        let line = self.line;
        let col = self.col;
        let content = self.content.clone();
        Box::pin(async move {
            use tokio::fs;
            let mut text = fs::read_to_string(&path).await?;
            let mut offset = 0usize;
            for (i, l) in text.lines().enumerate() {
                if i + 1 == line {
                    offset += col.min(l.len());
                    break;
                }
                offset += l.len() + 1; // +\n
            }
            text.insert_str(offset, &content);
            fs::write(path, text).await?;
            Ok("已定点插入内容".to_string())
        })
    }
}

struct MoveContentCommand {
    src: String,
    start_line: usize,
    end_line: usize,
    dst: String,
    dst_line: usize,
}

impl ToolCommand for MoveContentCommand {
    fn execute(&self) -> CmdFuture {
        let src = self.src.clone();
        let start_line = self.start_line;
        let end_line = self.end_line;
        let dst = self.dst.clone();
        let dst_line = self.dst_line;
        Box::pin(async move {
            use tokio::fs;
            let mut src_text = fs::read_to_string(&src).await?;
            let dst_text = fs::read_to_string(&dst).await?;

            let lines: Vec<&str> = src_text.lines().collect();
            let start = start_line.saturating_sub(1);
            let end = end_line.min(lines.len());
            let moving = lines[start..end].join("\n");

            // remove from src
            let mut new_src = String::new();
            for (i, l) in lines.iter().enumerate() {
                if i < start || i >= end {
                    new_src.push_str(l);
                    new_src.push('\n');
                }
            }
            src_text = new_src;

            // insert into dst
            let mut offset = 0usize;
            for (i, l) in dst_text.lines().enumerate() {
                if i + 1 == dst_line {
                    break;
                }
                offset += l.len() + 1;
            }
            let mut dst_new = dst_text.clone();
            dst_new.insert_str(offset, &format!("{}\n", moving));

            fs::write(&src, src_text).await?;
            fs::write(&dst, dst_new).await?;
            Ok("已移动内容".to_string())
        })
    }
}

// 命令匹配与解析规格（将字符串匹配逻辑移动到 trait 中）
trait CommandSpec {
    fn name(&self) -> &'static str;
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>>;
}

struct TouchSpec;
impl CommandSpec for TouchSpec {
    fn name(&self) -> &'static str {
        "touch"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let mut parts = args.split_whitespace();
        let path = parts.next()?;
        Some(Box::new(TouchCommand {
            path: path.to_string(),
        }))
    }
}

struct RmSpec;
impl CommandSpec for RmSpec {
    fn name(&self) -> &'static str {
        "rm"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let mut parts = args.split_whitespace();
        let path = parts.next()?;
        Some(Box::new(RmCommand {
            path: path.to_string(),
        }))
    }
}

struct WriteSpec;
impl CommandSpec for WriteSpec {
    fn name(&self) -> &'static str {
        "write"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let mut it = args.splitn(2, ' ');
        let path = it.next()?;
        let content = it.next().unwrap_or("");
        Some(Box::new(WriteCommand {
            path: path.to_string(),
            content: content.to_string(),
        }))
    }
}

struct FindSpec;
impl CommandSpec for FindSpec {
    fn name(&self) -> &'static str {
        "find"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let pattern = args.split_whitespace().next().unwrap_or("");
        Some(Box::new(FindCommand {
            pattern: pattern.to_string(),
        }))
    }
}

struct EditAtSpec;
impl CommandSpec for EditAtSpec {
    fn name(&self) -> &'static str {
        "edit-at"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let mut parts = args.split_whitespace();
        let path = parts.next()?.to_string();
        let line: usize = parts.next()?.parse().ok()?;
        let col: usize = parts.next()?.parse().ok()?;
        // 剩余内容作为待插入文本
        let consumed = format!("{} {} {}", path, line, col);
        let content = args
            .strip_prefix(&consumed)
            .and_then(|s| s.strip_prefix(' '))
            .unwrap_or("")
            .to_string();
        Some(Box::new(EditAtCommand {
            path,
            line,
            col,
            content,
        }))
    }
}

struct MoveContentSpec;
impl CommandSpec for MoveContentSpec {
    fn name(&self) -> &'static str {
        "move-content"
    }
    fn parse(&self, args: &str) -> Option<Box<dyn ToolCommand + Send>> {
        let mut parts = args.split_whitespace();
        let src = parts.next()?.to_string();
        let start_line: usize = parts.next()?.parse().ok()?;
        let end_line: usize = parts.next()?.parse().ok()?;
        let dst = parts.next()?.to_string();
        let dst_line: usize = parts.next()?.parse().ok()?;
        Some(Box::new(MoveContentCommand {
            src,
            start_line,
            end_line,
            dst,
            dst_line,
        }))
    }
}

fn command_specs() -> Vec<Box<dyn CommandSpec>> {
    vec![
        Box::new(TouchSpec),
        Box::new(RmSpec),
        Box::new(WriteSpec),
        Box::new(FindSpec),
        Box::new(EditAtSpec),
        Box::new(MoveContentSpec),
    ]
}

struct App {
    input: String,
    messages: Vec<(String, String)>, // (role, content)
    model: String,
    status: Status,
    events_tx: UnboundedSender<AppEvent>,
    events_rx: UnboundedReceiver<AppEvent>,
}

fn mask_api_key(key: &str) -> String {
    if key.is_empty() {
        return "(未设置)".to_string();
    }
    let len = key.len();
    if len <= 6 {
        return "*".repeat(len);
    }
    let prefix = &key[..3];
    let suffix = &key[len - 3..];
    format!("{}{}{}", prefix, "*".repeat(len - 6), suffix)
}

impl App {
    fn new() -> Self {
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let api_base = std::env::var("OPENAI_API_BASE")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        let masked_key = mask_api_key(&api_key);

        let messages = vec![
            (
                "system".to_string(),
                "你是一个助理，帮助进行AI编码。".to_string(),
            ),
            (
                "system".to_string(),
                format!(
                    "当前配置：api_base={}, model={}, api_key={}",
                    api_base, model, masked_key
                ),
            ),
        ];

        let (events_tx, events_rx) = unbounded_channel();

        Self {
            input: String::new(),
            messages,
            model,
            status: Status::Idle,
            events_tx,
            events_rx,
        }
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(5),
                Constraint::Length(1),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(frame.area());

    // Render messages
    let history_text = app
        .messages
        .iter()
        .map(|(role, content)| format!("{}: {}", role, content))
        .collect::<Vec<_>>()
        .join("\n");
    let history = Paragraph::new(history_text);
    frame.render_widget(history, chunks[0]);

    // Render status（输入框上方，无边框）
    let status_text = match &app.status {
        Status::Idle => "按 Enter 发送，Esc 退出".to_string(),
        Status::Requesting => "正在请求OpenAI...".to_string(),
        Status::Error(e) => format!("请求失败: {}", e),
    };
    let status = Paragraph::new(status_text);
    frame.render_widget(status, chunks[1]);

    // Render input（底部）
    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .title("输入（Enter 发送，Esc 退出）")
            .borders(Borders::ALL),
    );
    frame.render_widget(input, chunks[2]);
}

fn parse_command(input: &str) -> Option<Box<dyn ToolCommand + Send>> {
    let trimmed = input.trim();
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let mut it = rest.splitn(2, ' ');
    let cmd = it.next()?;
    let args = it.next().unwrap_or("");
    for spec in command_specs() {
        if spec.name() == cmd {
            if let Some(c) = spec.parse(args) {
                return Some(c);
            }
        }
    }
    None
}

async fn run_command(cmd: Box<dyn ToolCommand + Send>, tx: UnboundedSender<AppEvent>) {
    tx.send(AppEvent::Status(Status::Requesting)).ok();
    let result = cmd.execute().await;

    match result {
        Ok(msg) => {
            tx.send(AppEvent::System(msg)).ok();
            tx.send(AppEvent::Status(Status::Idle)).ok();
        }
        Err(e) => {
            tx.send(AppEvent::System(format!("操作失败: {}", e))).ok();
            tx.send(AppEvent::Status(Status::Error(e.to_string()))).ok();
        }
    }
}

// 已移除非流式响应函数，统一使用流式响应

async fn stream_to_openai(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> Result<(), Box<dyn Error>> {
    // Build messages in OpenAI format（包含最新的 user 输入）
    let msgs: Vec<ChatCompletionMessage> = app
        .messages
        .iter()
        .map(|(role, content)| {
            let role_enum = match role.as_str() {
                "system" => ChatCompletionMessageRole::System,
                "assistant" => ChatCompletionMessageRole::Assistant,
                _ => ChatCompletionMessageRole::User,
            };
            ChatCompletionMessage {
                role: role_enum,
                content: Some(content.clone()),
                name: None,
                function_call: None,
                tool_calls: None,
                tool_call_id: None,
            }
        })
        .collect();

    // 创建流
    let mut chat_stream = ChatCompletionDelta::builder(&app.model, msgs.clone())
        .create_stream()
        .await?;

    // 追加占位的 assistant 消息，用于边收边显示
    app.messages.push(("assistant".to_string(), String::new()));
    let idx = app.messages.len() - 1;

    let mut merged: Option<ChatCompletionDelta> = None;
    loop {
        // 消费后台事件（文件工具等），保持 UI 响应与动画
        while let Ok(ev) = app.events_rx.try_recv() {
            match ev {
                AppEvent::Status(s) => app.status = s,
                AppEvent::System(m) => app.messages.push(("system".to_string(), m)),
            }
            terminal.draw(|f| ui(f, app))?;
        }
        match chat_stream.try_recv() {
            Ok(delta) => {
                if let Some(content) = &delta.choices[0].delta.content {
                    app.messages[idx].1.push_str(content);
                    // 每收到一段内容就重绘
                    terminal.draw(|f| ui(f, app))?;
                }

                if let Some(m) = merged.as_mut() {
                    // 合并增量
                    m.merge(delta)?;
                } else {
                    merged = Some(delta);
                }
            }
            Err(TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(TryRecvError::Disconnected) => break,
        }
    }

    // 可选：将最终合并结果转为完整 ChatCompletion（当前未使用）
    let _final_completion: ChatCompletion = merged.unwrap().into();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Ensure API key is present (let-else)
    let Ok(_) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("未检测到环境变量 `OPENAI_API_KEY`，请先设置后再运行。");
        eprintln!(
            "PowerShell 示例：$Env:OPENAI_API_KEY='sk-...'\n可选：$Env:OPENAI_MODEL='gpt-4o-mini'"
        );
        return Ok(());
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = App::new();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Esc => break,
            KeyCode::Enter => {
                if app.input.trim().is_empty() {
                    continue;
                }
                if let Some(cmd) = parse_command(&app.input) {
                    let tx = app.events_tx.clone();
                    tokio::spawn(async move { run_command(cmd, tx).await });
                    // 保持动画与重绘由事件驱动
                } else {
                    app.messages.push(("user".to_string(), app.input.clone()));
                    app.status = Status::Requesting;
                    terminal.draw(|f| ui(f, &app))?;
                    if let Err(e) = stream_to_openai(&mut app, &mut terminal).await {
                        app.messages
                            .push(("system".to_string(), format!("请求失败: {}", e)));
                        app.status = Status::Error(e.to_string());
                    } else {
                        app.status = Status::Idle;
                    }
                }
                app.input.clear();
            }
            KeyCode::Char(c) => {
                app.input.push(c);
            }
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Tab => {
                app.input.push('\t');
            }
            _ => {}
        }
    }

    // restore terminal
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)?;

    Ok(())
}
