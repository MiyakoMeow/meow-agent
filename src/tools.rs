use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::UnboundedSender;

use crate::tui::{AppEvent, Status};

pub type CmdResult = Result<String, Box<dyn Error>>;
pub type CmdFuture = Pin<Box<dyn Future<Output = CmdResult> + Send>>;

pub trait ToolCommand: Send {
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
                    } else if let Some(name) = p.file_name().and_then(|s| s.to_str())
                        && name.contains(&pattern)
                    {
                        found.push(p.display().to_string());
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

pub trait CommandSpec {
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

pub fn parse_command(input: &str) -> Option<Box<dyn ToolCommand + Send>> {
    let trimmed = input.trim();
    if !trimmed.starts_with(':') {
        return None;
    }
    let rest = &trimmed[1..];
    let mut it = rest.splitn(2, ' ');
    let cmd = it.next()?;
    let args = it.next().unwrap_or("");
    for spec in command_specs() {
        if spec.name() == cmd
            && let Some(c) = spec.parse(args)
        {
            return Some(c);
        }
    }
    None
}

pub async fn run_command(cmd: Box<dyn ToolCommand + Send>, tx: UnboundedSender<AppEvent>) {
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
