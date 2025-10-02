use std::{error::Error, io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{execute, terminal::EnterAlternateScreen, terminal::LeaveAlternateScreen};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use openai::chat::{ChatCompletion, ChatCompletionMessage, ChatCompletionMessageRole};

struct App {
    input: String,
    messages: Vec<(String, String)>, // (role, content)
    model: String,
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

        Self {
            input: String::new(),
            messages,
            model,
        }
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)].as_ref())
        .split(frame.area());

    // Render messages
    let history_text = app
        .messages
        .iter()
        .map(|(role, content)| format!("{}: {}", role, content))
        .collect::<Vec<_>>()
        .join("\n");
    let history =
        Paragraph::new(history_text).block(Block::default().title("对话").borders(Borders::ALL));
    frame.render_widget(history, chunks[0]);

    // Render input（底部）
    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .title("输入（Enter 发送，Esc 退出）")
            .borders(Borders::ALL),
    );
    frame.render_widget(input, chunks[1]);
}

async fn send_to_openai(app: &mut App) -> Result<String, Box<dyn Error>> {
    // Build messages in OpenAI format
    let mut msgs: Vec<ChatCompletionMessage> = app
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

    // Append current user input as the latest message
    msgs.push(ChatCompletionMessage {
        role: ChatCompletionMessageRole::User,
        content: Some(app.input.clone()),
        name: None,
        function_call: None,
        tool_calls: None,
        tool_call_id: None,
    });

    // Build request and call API
    let req = ChatCompletion::builder(&app.model, msgs).build()?;
    let res = ChatCompletion::create(req).await?;

    let Some(first) = res
        .choices
        .get(0)
        .and_then(|c| c.message.content.as_ref())
        .cloned()
    else {
        return Ok("(无内容)".to_string());
    };

    Ok(first)
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

                app.messages.push(("user".to_string(), app.input.clone()));
                match send_to_openai(&mut app).await {
                    Ok(reply) => {
                        app.messages.push(("assistant".to_string(), reply));
                    }
                    Err(e) => {
                        app.messages
                            .push(("system".to_string(), format!("请求失败: {}", e)));
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
