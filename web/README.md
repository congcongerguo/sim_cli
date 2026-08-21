# sim_cli Web 调试终端(桥接方案)

**浏览器零安装**访问板子上的 sim_cli:不用 SSH、不用装任何客户端工具,打开浏览器即用。

## 为什么是这个形态

- 只有 **web** 真正做到"客户端零安装"(浏览器人人都有);SSH、远端 TUI 都还要在客户端装/编工具。
- **sim_cli 本体一行 web 代码都不加 → 固件体积零增长。** web 相关的东西全在你**同板的 tiny_http 服务**里(它本来就付了 tiny_http 的代价)。
- 浏览器 ⇄ 后端用 **SSE + POST**,纯 HTTP:tiny_http 原生支持,**不需要 SHA-1 / md-5 / WebSocket 帧**。

## 架构

```
浏览器 ──HTTP: GET / , GET /events(SSE) , POST /command──▶ tiny_http 桥 ──Unix socket(NDJSON)──▶ sim_cli --serve
```

- 每个浏览器的 SSE 连接 = 一条独立的 sim_cli 连接 = 一个独立 ViewSession(各自过滤/滚动);
  命令按 `sid` 落到同一条连接上。
- tab 列表、命令树、日志、状态**全部由 sim_cli 后端下发**,前端只是壳。

## 文件

| 文件 | 用途 |
|---|---|
| `index.html` | 单文件浏览器前端(SSE 收状态、POST 发命令、渲染日志/tab)。零外部资源,满足严格 CSP。 |
| `dev-bridge.py` | **纯标准库**的桥,可立刻本地跑起来验证整条链;也是语言无关的参考实现。 |
| `bridge.rs` | 给你 tiny_http 服务的 **Rust 参考桥**,可整段拷入适配。 |

## 立刻试(用 Python 桥,无需改你的服务)

```bash
# 终端 A:起无头后端(Linux 板子默认 Unix socket /tmp/sim_cli.sock)
cargo run --features serve --bin sim_cli -- --serve

# 终端 B:起桥
python3 web/dev-bridge.py          # 监听 127.0.0.1:8080

# 浏览器打开 http://127.0.0.1:8080  → 输入 help 回车
```
后端用 TCP 时:`SIM_CLI_TCP=127.0.0.1:7899 python3 web/dev-bridge.py`。
换监听地址:`SIM_CLI_WEB_BIND=0.0.0.0:8080`。

### Windows

Windows 无 Unix socket(Python 也没有 `AF_UNIX`),**必须走 TCP**。后端和桥都会
自动默认 TCP `127.0.0.1:7899`,直接跑即可:

```bat
:: 终端 A(sim_cli 在 Windows 上无 --tcp 也会自动回退到 127.0.0.1:7899)
cargo run --features serve --bin sim_cli -- --serve --tcp 127.0.0.1:7899

:: 终端 B(桥在 Windows 上自动用 TCP 127.0.0.1:7899)
python web\dev-bridge.py

:: 浏览器打开 http://127.0.0.1:8080
```
换地址:后端 `--tcp <host:port>`,桥 `set SIM_CLI_TCP=<host:port>`。

## 接进你的 tiny_http 服务(生产)

1. 你的 `Cargo.toml` 里有 `tiny_http = "0.12"` 即可(**SSE 不需要 md-5 / sha1**)。
2. 参照 `bridge.rs`:加三个路由 `GET /`、`GET /events`、`POST /command`;把 `index.html`
   用 `include_str!` 内嵌进你的二进制。
3. 常量 `SIM_CLI_SOCK` / `BIND` 按你的部署改。协议行为以 `dev-bridge.py` 为准
   (它经过端到端验证)。

## 医疗器械:上生产前必须补的(桥这一层负责)

sim_cli 的 socket 默认仅本机可达;**对外网络的安全项全部落在 tiny_http 这一层**:

- **认证**:`/events` 与 `/command` 加登录/令牌校验(至少 token,理想账号+角色)。
- **TLS**:对外用 HTTPS(tiny_http 的 `Server::https`,或前置反向代理)。
- **审计**:记录"谁、何时、下了什么命令"(命令都经过 `POST /command`,天然可记账)。
- **网络边界**:默认只绑内网/回环 + 反代,不要 `0.0.0.0` 裸奔。
- **会话回收**:SSE 断开时清理 `sid`(参考实现已在 `Drop` 里做)。

## 已知边界(与 sim_cli 现状一致)

- 当前用**全量模式**(`view.count=0`):每次更新推当前 tab 的全部消息,前端整段替换渲染。
  日志极大时可改用**按范围取窗**(sim_cli 协议已支持 `start+count`),前端做虚拟滚动。
- 活跃 tab / 权限弹窗为所有连接共享;过滤/滚动每连接独立(见 `docs/service-design.md`)。
