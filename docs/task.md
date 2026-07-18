# 新增 Task 指南

加一个新 task 只需两步：定义一个 toml 配置 + 写一个 actor 实现。

---

## 第一步：tasks.toml 加一行

```toml
[[task]]
name = "myserial"
hint = "serial port  —  connect / send"
border_color = [255, 128, 0]     # RGB
tcp_addr = "192.168.1.100:9000"  # 可选，没配用默认值
```

字段说明：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `name` | string | 是 | 标签页名称，唯一 |
| `hint` | string | 是 | 标签页描述，显示在欢迎消息和 help 里 |
| `border_color` | [u8;3] | 是 | 对话边框 RGB 颜色 |
| `commands` | string[] | 否 | 保留字段，actor 内部定义命令 |
| `tcp_addr` | string | 否 | TCP 传输地址，默认 127.0.0.1:7878 |
| `zmq_sub_addr` | string | 否 | ZMQ SUB 地址（需 zmq feature） |
| `zmq_pub_addr` | string | 否 | ZMQ PUB 地址（需 zmq feature） |

---

## 第二步：写 actor

在 `src/backend/task/` 下新建文件，实现 `TaskActor` trait：

```rust
// src/backend/task/myserial_actor.rs

use crate::message::{LogLevel, Message};
use super::super::chat::ChatState;
use super::registry::TaskDef;
use super::{cmd, CommandDef, TaskActor, TaskSnapshot};

pub struct MySerialTask {
    chat: ChatState,
    def: &'static TaskDef,
}

impl MySerialTask {
    pub fn new(model: String, def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(model), def }
    }
}

impl TaskActor for MySerialTask {
    // 1. 声明这个 task 支持的命令
    fn commands(&self) -> Vec<CommandDef> {
        vec![
            cmd("help", "show commands"),    // 共享
            cmd("clear", "clear log"),       // 共享
            cmd("exit", "quit"),             // 共享
            cmd("connect", "open serial port"),
            cmd("send", "send data"),
        ]
    }

    // 2. 处理自己独有的命令（help/clear 由 trait 默认实现兜底）
    fn handle_own(&mut self, cmd: &str, _sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "connect" => vec![msg("connected", LogLevel::Notice)],
            "send"    => vec![msg("data sent", LogLevel::Info)],
            _         => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    // 3. 定时回调（每秒一次，默认空实现）
    fn tick(&mut self) -> Vec<Message> {
        vec![]  // 不需要就返回空
    }

    // 4. 状态快照
    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.clone(),
            model: self.chat.model.clone(),
            conn: crate::backend::ConnState::Disconnected,
            demo_running: false,
            latest_recv: None,
            latest_recv_at: None,
        }
    }

    // 5. 聊天记录
    fn chat(&self) -> &ChatState { &self.chat }
    fn chat_mut(&mut self) -> &mut ChatState { &mut self.chat }
}

fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}
```

---

## 第三步：注册到 Router

在 `src/backend/mod.rs` 的 `Router::new()` 里加一行：

```rust
"myserial" => {
    let actor = task::myserial_actor::MySerialTask::new(model.clone(), def);
    let cmds = Arc::new(actor.commands());
    (task::spawn_actor(actor), cmds)
}
```

然后在 `src/backend/task/mod.rs` 顶部加模块声明：

```rust
pub mod myserial_actor;
```

---

## TaskActor trait 完整接口

```rust
pub trait TaskActor: Send + 'static {
    fn commands(&self) -> Vec<CommandDef>;       // 必须实现：命令列表
    fn handle_command(&mut self, ...) -> Vec<Message>;  // 有默认实现：匹配 help/clear
    fn handle_own(&mut self, ...) -> Vec<Message>;      // 必须实现：处理自己独有的命令
    fn snapshot(&self) -> TaskSnapshot;           // 必须实现：状态快照

    fn tick(&mut self) -> Vec<Message> { vec![] }       // 可选：每秒回调
    fn chat(&self) -> &ChatState;                 // 必须实现
    fn chat_mut(&mut self) -> &mut ChatState;     // 必须实现
}
```

---

## Harness 提供的运行时

`spawn_actor()` 自动提供：

- **命令接收**：mpsc channel，前端输入文本自动到达
- **状态推送**：每 100ms 调 `snapshot()`，通过 watch channel 发给 Router
- **定时回调**：每秒调 `tick()`
- **tokio 生命周期**：自动 spawn + select! 事件循环

Actor 只管实现业务逻辑，不用管 tokio。

---

## 命令格式

前端传来的原始文本会被拆成三段：

```
"con tcp 192.168.1.1"  →  cmd="con", sub=Some("tcp"), args=["192.168.1.1"]
"help"                 →  cmd="help", sub=None, args=[]
"start"                →  cmd="start", sub=None, args=[]
```

`handle_command` 的默认实现只处理 `help` 和 `clear`，其余转到 `handle_own`。
