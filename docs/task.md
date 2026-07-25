# 新增 Tool 指南

加一个新 tool（tab）只需三步。

---

## 第一步：实现 Tool trait

在 `src/tool/` 下新建文件，实现 [`Tool`] trait：

```rust
// src/tool/newtool.rs

use crate::message::{LogLevel, Message};
use super::{cmd, msg, Cmd, Tool, ToolState};

pub struct NewTool {
    // tool-specific state
}

impl NewTool {
    pub fn new(_def: &'static super::registry::ToolDef) -> Self {
        Self { }
    }
}

impl Tool for NewTool {
    fn commands(&self) -> Vec<Cmd> {
        vec![
            cmd("start", "begin work"),
            cmd("stop",  "stop work"),
        ]
    }

    fn handle(&mut self, cmd: &str, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "start" => vec![msg("started", LogLevel::Notice)],
            "stop"  => vec![msg("stopped", LogLevel::Info)],
            _       => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn snapshot(&self) -> ToolState { ToolState::default() }
}
```

---

## 第二步：注册到 register_tools!

在 `src/tool/mod.rs` 底部的 `register_tools!` 宏中加一行：

```rust
register_tools! {
    conn::ConnTool,
    demo::DemoTool,
    newtool::NewTool,   // ← 加这一行
}
```

宏自动生成模块声明和工厂函数，无需手动维护 `pub mod` 和 `create()`。

---

## 第三步：tasks.toml 加配置

```toml
[[tool]]
name = "newtool"
hint = "new tool     -  start / stop"
```

---

## Tool trait 接口

```rust
pub trait Tool: Send + 'static {
    fn commands(&self) -> Vec<Cmd>;                    // 命令列表（框架自动追加 help/clear/exit）
    fn handle(&mut self, cmd: &str, args: &[&str]) -> Vec<Message>;  // 处理用户命令
    fn tick(&mut self) -> Vec<Message> { vec![] }      // 可选：定时回调
    fn snapshot(&self) -> ToolState { default }         // 可选：状态栏快照
    fn tick_ms(&self) -> u64 { 500 }                    // 可选：tick 间隔（毫秒）
    fn push_ms(&self) -> u64 { 100 }                    // 可选：状态推送间隔
    fn take_transport_rx(&mut self) -> Receiver<TransportEvent>;  // 可选：transport 集成
    fn on_transport(&mut self, ev: TransportEvent) -> Vec<Message> { vec![] }
}
```

## Harness 运行时

`spawn()` 自动提供：

- 命令接收（mpsc channel）
- 消息日志（LogBuffer + 落盘）
- 状态推送（watch channel）
- 定时回调（tick）
- tokio 生命周期管理
