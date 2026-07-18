# 新增 Task 指南

加一个新 task 只需三步，且每个 task 有自己的 feature flag——关掉 feature 就完全不编译。

---

## 第一步：tasks.toml 加一行

```toml
[[task]]
name = "myserial"
hint = "serial port  —  connect / send"
border_color = [255, 128, 0]     # RGB
tcp_addr = "192.168.1.100:9000"  # 可选
```

---

## 第二步：写 actor + create 回调

在 `src/backend/task/` 下新建文件，实现 `TaskActor` trait，并导出 `pub fn create`：

```rust
// src/backend/task/myserial_actor.rs

use crate::message::{LogLevel, Message};
use super::super::chat::ChatState;
use super::registry::TaskDef;
use super::{cmd, CommandDef, TaskActor, TaskRuntime, TaskSnapshot};
use std::sync::Arc;

pub struct MySerialTask { chat: ChatState, def: &'static TaskDef }

impl MySerialTask {
    pub fn new(model: String, def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(model), def }
    }
}

impl TaskActor for MySerialTask {
    fn commands(&self) -> Vec<CommandDef> {
        vec![cmd("help", "show commands"), cmd("clear", "clear log"),
             cmd("exit", "quit"), cmd("connect", "open port"), cmd("send", "send data")]
    }

    fn handle_own(&mut self, cmd: &str, _sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "connect" => vec![msg("connected", LogLevel::Notice)],
            "send"    => vec![msg("data sent", LogLevel::Info)],
            _         => vec![msg("unknown", LogLevel::Error)],
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(), messages: self.chat.messages.clone(),
            model: self.chat.model.clone(), conn: crate::backend::ConnState::Disconnected,
            demo_running: false, latest_recv: None, latest_recv_at: None,
        }
    }

    fn chat(&self) -> &ChatState { &self.chat }
    fn chat_mut(&mut self) -> &mut ChatState { &mut self.chat }
}

/// 回调：创建并 spawn 这个 actor
pub fn create(model: String, def: &'static TaskDef) -> TaskRuntime {
    let actor = MySerialTask::new(model, def);
    let cmds = Arc::new(actor.commands());
    let handle = super::spawn_actor(actor);
    TaskRuntime { handle, commands: cmds }
}

fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}
```

---

## 第三步：注册 feature + 模块 + 回调

**Cargo.toml** 加 feature：

```toml
[features]
default = ["zmq", "conn-task", "demo-task", "myserial-task"]
myserial-task = []
```

**`src/backend/task/mod.rs`** 顶部加模块声明：

```rust
#[cfg(feature = "myserial-task")]
pub mod myserial_actor;
```

**`src/backend/task/mod.rs`** 的 `create_actor` 加分支：

```rust
pub fn create_actor(name: &str, model: String, def: &'static TaskDef) -> Option<TaskRuntime> {
    #[cfg(feature = "conn-task")]
    if name == "conn" { return Some(conn_actor::create(model, def)); }
    #[cfg(feature = "demo-task")]
    if name == "demo" { return Some(demo_actor::create(model, def)); }
    #[cfg(feature = "myserial-task")]
    if name == "myserial" { return Some(myserial_actor::create(model, def)); }
    None
}
```

---

## Feature 隔离

每个 task 有自己的 feature flag。关闭 feature 后：

- 模块不编译 → 代码完全不进二进制
- `create_actor` 返回 `None` → Router 静默跳过
- `tasks.toml` 里对应条目也跳过

```bash
# 只要 conn task，demo 代码完全不编译
cargo build --no-default-features --features conn-task
```

---

## TaskActor trait 接口

```rust
pub trait TaskActor: Send + 'static {
    fn commands(&self) -> Vec<CommandDef>;           // 必须：命令列表
    fn handle_own(&mut self, cmd, sub, args) -> Vec<Message>;  // 必须：自己独有的命令
    fn snapshot(&self) -> TaskSnapshot;               // 必须：状态快照
    fn chat(&self) -> &ChatState;                     // 必须
    fn chat_mut(&mut self) -> &mut ChatState;         // 必须

    fn handle_command(&mut self, cmd, sub, args) -> Vec<Message>;  // 默认：匹配 help/clear
    fn tick(&mut self) -> Vec<Message> { vec![] }     // 可选：每秒回调
}
```

---

## Harness 运行时

`spawn_actor()` 自动提供：

- 命令接收（mpsc channel）
- 状态推送（每 100ms 调 `snapshot()`）
- 定时回调（每秒调 `tick()`）
- tokio 生命周期管理
