# sim_cli 服务化设计（前后端分离 / 多前端接入）

> 目标读者：开发 + 评审。本文只做设计，不含实现。

## 1. 背景与目标

现在 `sim_cli` 是**单进程 TUI**：前端（ratatui/crossterm）和后端（Router + Tools）
在同一个进程里，通过内存 channel 通信，必须 shell 登录到板子上启动，关掉终端后端也没了。

目标：把它拆成**常驻后端 + 按需前端**的服务化结构。

- **后端常驻**：`sim_cli --serve` 起来就一直跑，Tool 持续工作、日志持续落盘，没人连也照跑。
- **前端按需打开**：用的时候才连；关掉前端不影响后端。
- **前端只是壳**：有哪些 tab、命令、子命令，全由后端下发；前端不写死业务。
- **多前端**：TUI / Web / 脚本 可同时接入，各看各的。
- **医疗器械约束**：核心不上网、信任边界清晰、可审计、依赖可控、体积可控。

非目标：本设计不改任何 Tool 的业务逻辑；不引入重型框架。

## 2. 架构总览

```
              [ 后端：常驻进程 (daemon) ]
               Router + 所有 Tool 一直在跑
               LogBuffer / msg_log 持续记录
               每连接维护一个 ViewSession
                        ▲
                        │  JSON 协议（NDJSON over socket）
        ┌───────────────┼─────────────────┬──────────────┐
     TUI 前端        Web 前端            脚本/自动化      其他原生客户端
   (ratatui)     (浏览器, 需桥接)     (nc/python)      (Qt/另一Rust程序)
   随连随断,关掉前端后端毫发无损;可多前端同时接入
```

本质：把现在**进程内的 `Command`/`ViewState` channel 边界，拉成进程间的 socket 边界**。
`backend::run`（Command 进 / ViewState 出）基本原样复用。

## 3. 进程模型 / 启动模式

| 命令 | 行为 |
|---|---|
| `sim_cli` | 现状：本地单进程 TUI（默认，保持不变） |
| `sim_cli --serve` | 无头后端常驻，监听 socket，不碰终端 |
| `sim_cli --connect <addr>` | TUI 作为客户端连远端后端 |

serve / client 相关代码放在 optional feature（如 `serve`）里，普通固件构建默认不含，
保住嵌入式体积。

## 4. 传输层

抽象一层 `Transport` trait，上层协议与传输解耦：

- **Unix domain socket**（默认，同板）：本机隔离、不占端口、零网络暴露。
- **TCP**（可选，远端）：`--serve --tcp <addr>`，对外必须前置 TLS。

换传输不改协议。传输实现全部 feature 门控。

## 5. 协议

**格式**：NDJSON —— 每行一个 JSON 对象。选它因为只依赖已有的 `serde_json`、跨语言、
可读、易调试（`nc`/`websocat` 可直接测）。

**握手**（连上后后端先发）：
```json
{"type":"hello","version":"0.1.0","protocol":1}
```

**共享状态帧**（广播，很小；所有客户端一致）：tab 列表、命令树、状态面板、mode、
streaming、modal。
```json
{"type":"shared","tools":[...],"active_cmds":[...],"state":{...},"mode":"normal","streaming":false,"modal":null}
```

**客户端 → 后端：命令**（对应现有 `Command`）：
```json
{"type":"input","text":"con zmq tcp://..."}
{"type":"tab_switch","name":"demo"}
{"type":"permission","choice":"always"}
```

**客户端 → 后端：视口请求**（按范围取窗，见第 7 节）：
```json
{"type":"view","tab":"conn","start":960,"count":44,"filter":"/error/","exclude":null}
```

**后端 → 客户端：视口内容**（只发这一屏，不发全量缓冲）：
```json
{"type":"window","tab":"conn","start":960,"lines":[...],"total":1000,"evicted":0,"follow":true}
```

**错误帧**（非法输入回错误，不崩服务）：
```json
{"type":"error","code":"bad_command","detail":"..."}
```

设计要点：`ViewState` 拆成**「共享状态（广播）」+「视口内容（每连接）」**两部分，
避免每帧把整个缓冲序列化过网络。

## 6. 后端职责变化：每连接会话 + 过滤/窗口下沉

现在过滤（`filter.rs` 346 行）和滚动（`scroll.rs` 227 行）在前端。服务化后**下沉到后端**，
因为过滤要作用于全量数据，而全量在后端；grep 搜的是板子上落盘的历史，客户端根本碰不到。

后端为**每个连接**维护一个 `ViewSession`：
```
ViewSession {
    tab: String,            // 当前看哪个 tab
    scroll: ScrollPos,      // 滚动位置 / 是否跟随底部
    viewport_rows: u16,     // 客户端申报的一屏行数
    filter: Option<Filter>, // include 过滤
    exclude: Option<Filter>,// exclude 过滤
}
```
Router 里加 `HashMap<ConnId, ViewSession>`。`scroll.rs` 的"跟随底部/滚动偏移/驱逐补偿"
逻辑搬过来做成"每连接一份"，不是重写。

**局部性**：多数事件只影响一个连接，不惊动别人。

### 何时重算某连接的窗口（触发表）

| 事件 | 重算谁 | 说明 |
|---|---|---|
| 缓冲来了新行 | 该 tab 上所有连接 | 跟随底部的滚动显示新行；停在历史的只更新"未读 N / 滚动条" |
| 某连接滚动 | 只它自己 | 位置变了 |
| 某连接改 filter/exclude | 只它自己 | 匹配集合变了 |
| 某连接切 tab | 只它自己 | 换缓冲了 |
| 某连接改视口大小 | 只它自己 | 一屏行数变了 |

**易错点**（需单元测试覆盖）：该重算却漏算（新行来了不刷）、滚动位置与过滤后行号不对齐、
缓冲满驱逐旧行导致行号偏移（`evicted_lines` 补偿）。建议对 filter 重算做 debounce + 缓存上一窗口。

## 7. 视口："一页"怎么确定（TUI vs Web）

原则：**永远是客户端申报自己的视口，后端不猜。** 协议做成**按范围取（start + count）**，
一套后端逻辑同时服务两端：

| 客户端 | start 来源 | count 来源 |
|---|---|---|
| TUI | 滚动偏移 | crossterm 给的终端行数（resize 事件更新） |
| Web | `scrollTop / 每行高度` | `容器高度 / 每行高度`（+ overscan 余量） |

### Web 换算规则

```
一页行数 = floor( 容器 clientHeight / 每行像素高度 )
```
- 用 `ResizeObserver` 监听日志容器高度；窗口 resize / 字号变化 / 页面缩放都会触发。
- 换算后通过 `{"type":"view",...}` 上报，**debounce ~100ms**。
- **约定固定行高**（等宽字体、固定 line-height），像素↔行数换算才精确；日志视图不做变高行。

### Web 顺滑滚动（可选）

用列表虚拟化：多取上下 overscan 行（如取 100 行），本地滚动丝滑；滚近缓冲边缘再请求下一段。
后端不变，只是 web 把 count 要大一点。TUI 不需要（本就一行一行）。

## 8. 前端支持矩阵

| 前端 | 传输 | 需桥接 | 说明 |
|---|---|---|---|
| TUI（现有） | 进程内 / Unix socket / TCP | 否 | 界面基本不动，数据源改为 socket |
| Web（浏览器） | WebSocket / HTTP | **是** | 浏览器不能开裸 socket，中间需一层桥 |
| 脚本 / 自动化 | Unix socket / TCP | 否 | nc/socat/python 直接发 JSON 行，利于回归测试 |
| 其他原生客户端 | socket / TCP | 否 | Qt/Electron/另一 Rust 程序（可复用类型） |

### 浏览器桥接（两种，推荐方式1）

- **方式1（推荐）**：后端只说 NDJSON socket，**你的 web 服务做桥**：
  `浏览器 ⇄ WebSocket ⇄ web服务 ⇄ socket ⇄ 后端`。
  后端极小、不上网、无 web 依赖；认证/TLS/审计都在 web 服务那层。信任边界干净，体积不涨。
- **方式2**：后端自带 WebSocket 端点（feature 门控）。少一个进程，但后端背 WS/HTTP 依赖、
  网络暴露进核心。医疗器械不推荐。

## 9. 医疗器械约束（设计基线）

- 核心后端**默认只绑 Unix socket / 回环**，不直接对外网络。
- 对外网络的**认证 + TLS + 审计**放在唯一对外那层（你的 web 服务）。
- 每条 `Command` 落审计日志：来源客户端 + 时间 + 内容（复用现有 `msg_log`）。
- 协议带 `version`/`protocol` 字段，便于版本追溯与兼容管理。
- serve/TCP/WS 依赖全部 optional，常规固件构建保持现状，新增 SOUP 降到最低。
- 过滤/窗口逻辑**全后端只有一份**（所有前端走同一 view-session 协议），减少要验证的路径。

## 10. 代码改动清单与体积影响

1. 给 `Command`/`ViewState`（拆分后）/`Cmd`/`ToolState`/`Message` 加 serde 派生（feature 门控）。
   - 注意 `Cmd` 现为 `&'static str`/`&'static [Cmd]`，序列化时转 owned DTO（String + Vec）。
2. 新增 `src/serve.rs`：accept 连接 → 桥接 `cmd_tx`/视口 到 socket；每连接一个 `ViewSession`。
3. 把 `filter.rs`/`scroll.rs` 从前端职责下沉到后端会话（搬迁为主）。
4. `main.rs` 按参数分派：`--serve` / `--connect` / 默认本地 TUI。
5. 前端：删除自身过滤/滚动所有权，改为"发视口参数 + 渲染给定窗口"。

体积：默认构建不变（feature 关）。开 `serve` 仅增 socket + serde 逻辑；开 web 直连才引入 WS 依赖
（故不推荐，交给外部 web 服务）。

## 11. 分阶段落地

1. **阶段一**：serde 派生 + `--serve`（Unix socket + NDJSON）+ 命令行脚本客户端跑通
   "发命令 / 收共享状态" 最小闭环。
2. **阶段二**：视口协议（按范围取窗）+ 过滤/滚动下沉到 ViewSession + 每连接窗口重算。
3. **阶段三**：TUI 改造为 `--connect` 客户端，与本地模式共用一套协议。
4. **阶段四**：Web 前端 + 浏览器桥（由外部 web 服务承担），认证/TLS/审计接入。

## 12. 待测用例清单（重点在窗口重算）

- 新行到达：跟随底部的连接刷新；停在历史的连接位置不动、未读计数 +1。
- 过滤开启时滚动：行号对齐，无跳行/闪烁。
- 缓冲驱逐旧行：`evicted_lines` 补偿后滚动位置正确。
- 多连接互不干扰：A 滚动不影响 B 的窗口。
- 视口 resize：行数变化后窗口正确重算。
- 非法命令 / 非法过滤表达式：回 `error` 帧，服务不崩。
- 客户端断开重连：重发 hello + 共享状态 + 当前窗口。

---

## 13. 实现状态（阶段一~三已落地，阶段四 Web 未做）

全部新增代码在 `serve` feature 门控下，**默认固件构建（不带 `serve`）完全不变**。
默认 `zmq` feature 需要 `protoc`；开发校验用 `--no-default-features --features serve`。

### 已实现

- **协议** `src/protocol.rs`：NDJSON 线协议 + owned DTO（`ClientMsg` / `ServerMsg`：
  `Hello` / `Shared` / `Window` / `Error`）与内部类型互转。`ViewState` 已拆为
  **共享状态 `Shared`（广播）** + **视口内容 `Window`（每连接）**。
- **无头后端** `src/serve.rs`：`sim_cli --serve` 起 Unix socket daemon，复用现有
  `backend::run`（Tool / 路由 / 后端逻辑零改动）。每连接一个 `ViewSession`，
  含 include/exclude 过滤、grep、按范围取窗（`start + count`，`count==0` 为全量模式）。
  过滤复用 `filter.rs`，grep 复用 `msg_log::scan`（搜**服务端**归档）。窗口/共享帧
  按 JSON 去重，抑制空闲期重复推送。
- **远端 TUI** `src/client.rs`：`sim_cli --connect [sock]` 连上后端，**复用现有
  `Frontend` 零改动**，把 socket 帧桥接成前端消费的 `ViewState`。
- **入口** `src/main.rs`：`--serve` / `--connect` / 默认本地 TUI 三态分派;
  无 `serve` feature 时给出友好报错。
- **脚本客户端** `scripts/sim-client.py`：跨语言示例(连 socket、按行读写 JSON)。
- **测试**：`protocol` 往返、`ViewSession` 过滤/窗口/grep 单测、端到端 socket 闭环
  (`UnixStream::pair` + 真后端,握手→发命令→收窗口)。`--features serve` 下 99 项全绿。

### 运行方式

两种传输,协议一致(见 `Endpoint`):
- **Unix socket**(unix 平台默认):仅本机、零网络暴露,推荐。
- **TCP**(跨平台;**Windows 只能用它**,因 tokio 的 Unix socket 不支持 Windows):
  对外时须前置 TLS。

```bash
# —— Unix socket(Linux / macOS)——
cargo run --features serve --bin sim_cli -- --serve            # 默认 /tmp/sim_cli.sock
cargo run --features serve --bin sim_cli -- --connect          # 远端 TUI(需真终端)
python3 scripts/sim-client.py                                  # 脚本客户端

# —— TCP(任意平台,含 Windows)——
cargo run --features serve --bin sim_cli -- --serve --tcp 127.0.0.1:7899
cargo run --features serve --bin sim_cli -- --connect 127.0.0.1:7899
python3 scripts/sim-client.py 127.0.0.1:7899
```
覆盖端点:`--socket <path>`(Unix)、`--tcp <host:port>`(TCP);或环境变量
`SIM_CLI_SOCK` / `SIM_CLI_TCP`。`--connect` 的位置参数含 `:` 视为 TCP,否则视为
Unix 路径。**Windows** 上不带 `--tcp` 会自动回退到默认 TCP 回环并给出提示。

### 本实现的已知边界（后续可迭代）

1. **共享会话语义**：当前**活跃 tab 与权限弹窗为所有连接共享**（一个连接切 tab，
   大家一起切）；**过滤 / exclude / grep / 滚动为每连接独立**。多连接“各自独立 tab”
   属未来增强（需把 Router 的单一 `active` 改为每连接）。
2. **远端 TUI 用全量模式**（`count==0`，收当前 tab 全部消息，本地做滚动/过滤 —— 复用
   久经测试的前端逻辑，零风险）。**按范围取窗（分页）** 已实现并经单测/脚本客户端验证，
   主要供 Web / 瘦客户端（阶段四）使用。
3. **远端 TUI 的 `grep`** 仍走前端本地 `msg_log::scan`（搜的是客户端本机归档，通常为空）；
   **服务端 grep 已实现**，脚本 / Web 客户端可经 `{"type":"grep",...}` 直接使用。把远端
   TUI 的 grep 接到 `ClientMsg::Grep` 需前端一个小钩子，留待后续。
4. **空闲推送**约 10Hz（后端 watch 每 100ms 一帧），已按 JSON 去重；如需进一步降噪可加
   debounce。
5. **医疗器械网络安全项**（认证 / TLS / 审计 / 网络边界）随阶段四对外那层（Web 服务或 TCP+TLS）
   落地；当前 Unix socket 默认仅本机可达，未对外。
