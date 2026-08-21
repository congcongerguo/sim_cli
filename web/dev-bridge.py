#!/usr/bin/env python3
"""开发用桥:浏览器 ⇄(SSE + POST)⇄ 本桥 ⇄(socket)⇄ sim_cli 无头后端。

纯标准库,零依赖。作用有二:
  1. 让你**立刻**在浏览器里试 web 调试终端,不用先改你的 tiny_http 服务;
  2. 作为**语言无关的参考实现** —— 你的 Rust/tiny_http 桥照着这个逻辑写即可
     (对应 Rust 版见 web/bridge.rs)。

数据流:
  GET  /          → 返回 web/index.html
  GET  /events    → 打开一条到 sim_cli 的 socket;把 sim_cli 推来的每行 JSON
                    (ServerMsg)转成一条 SSE `data:` 事件流给浏览器;首个
                    `session` 事件带回本连接的 sid。
  POST /command?sid=... → 按 sid 找到对应 socket,把请求体(ClientMsg JSON)写进去。

每个浏览器 SSE 连接 = 一条独立的 sim_cli 连接 = 一个独立 ViewSession
(各自的过滤/滚动);命令通过 sid 落到同一条连接上。

用法:
  # 终端 A:起后端(Unix socket,Linux 板子上的默认)
  cargo run --features serve --bin sim_cli -- --serve
  # 或 TCP:
  cargo run --features serve --bin sim_cli -- --serve --tcp 127.0.0.1:7899

  # 终端 B:起桥
  python3 web/dev-bridge.py                       # 默认桥 :8080,连默认 socket
  SIM_CLI_TCP=127.0.0.1:7899 python3 web/dev-bridge.py   # 连 TCP 后端

  # 浏览器打开 http://127.0.0.1:8080
"""
import json
import os
import socket
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
BIND = os.environ.get("SIM_CLI_WEB_BIND", "127.0.0.1:8080")

# sid -> 已连接 sim_cli 的 socket。SSE 连接建;POST 用。
SESSIONS: dict[str, socket.socket] = {}
LOCK = threading.Lock()


def backend_target() -> tuple[str, str]:
    """决定连 sim_cli 后端的方式,返回 ("tcp", "host:port") 或 ("unix", path)。

    - 显式设了 SIM_CLI_TCP → TCP;
    - 否则本机无 AF_UNIX(如 Windows)→ 默认 TCP 127.0.0.1:7899
      (与 `sim_cli --serve` 在 Windows 上自动回退的默认地址一致);
    - 否则(Linux/macOS)→ Unix socket。
    """
    tcp = os.environ.get("SIM_CLI_TCP")
    if not tcp and not hasattr(socket, "AF_UNIX"):
        tcp = "127.0.0.1:7899"
    if tcp:
        return ("tcp", tcp)
    return ("unix", os.environ.get("SIM_CLI_SOCK", "/tmp/sim_cli.sock"))


def backend_connect() -> socket.socket:
    kind, target = backend_target()
    if kind == "tcp":
        host, _, port = target.rpartition(":")
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.connect((host or "127.0.0.1", int(port)))
        return s
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(target)
    return s


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):  # 安静
        pass

    def handle(self):
        # 浏览器随时会断开 SSE/预连接;吞掉这类断连异常,别刷屏 traceback。
        try:
            super().handle()
        except (ConnectionAbortedError, ConnectionResetError, BrokenPipeError):
            pass

    # ── GET:首页 / SSE 事件流 ──────────────────────────────────────────
    def do_GET(self):
        path = urlparse(self.path).path
        if path in ("/", "/index.html"):
            self._serve_file(os.path.join(HERE, "index.html"), "text/html; charset=utf-8")
        elif path == "/events":
            self._serve_events()
        else:
            self.send_error(404)

    def _serve_file(self, fpath, ctype):
        try:
            with open(fpath, "rb") as f:
                data = f.read()
        except OSError:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _serve_events(self):
        try:
            sock = backend_connect()
        except OSError as e:
            self.send_error(502, f"backend connect failed: {e}")
            return
        sid = uuid.uuid4().hex
        with LOCK:
            SESSIONS[sid] = sock

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        # 首个事件:把 sid 交给浏览器(POST 时带回)。
        self._sse_raw(f"event: session\ndata: {sid}\n\n")
        # 声明一个"有界尾窗、跟随底部"的视口(不要全量 count:0,否则日志一多必卡)。
        # 浏览器连上后会用自己的视口高度再发一次更精确的 view 覆盖它。
        try:
            sock.sendall(b'{"type":"view","count":400,"follow":true}\n')
        except OSError:
            pass

        buf = b""
        try:
            while True:
                chunk = sock.recv(65536)
                if not chunk:
                    break
                buf += chunk
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    line = line.strip()
                    if line:
                        # sim_cli 的每行(ServerMsg JSON)原样转成一条 SSE 事件。
                        self._sse_raw("data: " + line.decode("utf-8", "replace") + "\n\n")
        except (OSError, BrokenPipeError):
            pass
        finally:
            with LOCK:
                SESSIONS.pop(sid, None)
            try:
                sock.close()
            except OSError:
                pass

    def _sse_raw(self, text: str):
        self.wfile.write(text.encode("utf-8"))
        self.wfile.flush()

    # ── POST:命令 → 对应 socket ────────────────────────────────────────
    def do_POST(self):
        parsed = urlparse(self.path)
        if parsed.path != "/command":
            self.send_error(404)
            return
        sid = (parse_qs(parsed.query).get("sid") or [None])[0]
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else b""
        with LOCK:
            sock = SESSIONS.get(sid)
        if sock is None:
            self.send_error(409, "unknown or expired session")
            return
        try:
            # 校验是合法 JSON(挡掉垃圾),再转发。
            json.loads(body or b"{}")
            sock.sendall(body.rstrip() + b"\n")
        except (OSError, ValueError) as e:
            self.send_error(502, f"forward failed: {e}")
            return
        self.send_response(204)
        self.end_headers()


def main():
    host, _, port = BIND.rpartition(":")
    srv = ThreadingHTTPServer((host or "127.0.0.1", int(port)), Handler)
    kind, target = backend_target()
    print(f"[web] http://{host or '127.0.0.1'}:{port}  → sim_cli ({kind} {target})")
    if kind == "tcp":
        print(f"[web] 提示:请确保后端在监听 TCP:  sim_cli --serve --tcp {target}")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
