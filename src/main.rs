use std::{error::Error, io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{execute, terminal::EnterAlternateScreen, terminal::LeaveAlternateScreen};

mod api;
mod tools;
mod tui;

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

    let mut app = tui::App::new();

    loop {
        terminal.draw(|f| tui::ui(f, &app))?;

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
                if let Some(cmd) = tools::parse_command(&app.input) {
                    let tx = app.events_tx.clone();
                    tokio::spawn(async move { tools::run_command(cmd, tx).await });
                } else {
                    app.messages.push(("user".to_string(), app.input.clone()));
                    app.status = tui::Status::Requesting;
                    terminal.draw(|f| tui::ui(f, &app))?;
                    if let Err(e) = api::stream_to_openai(&mut app, &mut terminal).await {
                        app.messages
                            .push(("system".to_string(), format!("请求失败: {}", e)));
                        app.status = tui::Status::Error(e.to_string());
                    } else {
                        app.status = tui::Status::Idle;
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
