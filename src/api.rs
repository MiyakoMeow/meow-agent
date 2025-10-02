use std::{error::Error, time::Duration};
use tokio::sync::mpsc::error::TryRecvError;

use openai::chat::{
    ChatCompletion, ChatCompletionDelta, ChatCompletionMessage, ChatCompletionMessageRole,
};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::Stdout;

use crate::tui::{App, AppEvent, ui};

pub async fn stream_to_openai(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
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
