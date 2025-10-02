use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

pub enum Status {
    Idle,
    Requesting,
    Error(String),
}

pub enum AppEvent {
    Status(Status),
    System(String),
}

pub struct App {
    pub input: String,
    pub messages: Vec<(String, String)>, // (role, content)
    pub model: String,
    pub status: Status,
    pub events_tx: UnboundedSender<AppEvent>,
    pub events_rx: UnboundedReceiver<AppEvent>,
}

pub fn mask_api_key(key: &str) -> String {
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
    pub fn new() -> Self {
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

pub fn ui(frame: &mut Frame, app: &App) {
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
