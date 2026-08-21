#!/usr/bin/env python3
"""极简脚本客户端 —— 演示 sim_cli 无头后端的线协议(NDJSON over Unix socket)。

任何语言只要能连 socket、按行读写 JSON,就能当 sim_cli 的前端。本脚本即示例:
连上 `--serve` 后端,把服务端推来的每一帧打印出来,并把你在终端输入的每一行
当作 `input` 命令发过去。

用法:
    # 终端 A:起无头后端(Unix socket,unix 平台)
    cargo run --features serve -- --serve
    # 或 TCP(跨平台,Windows 用这个):
    cargo run --features serve -- --serve --tcp 127.0.0.1:7899

    # 终端 B:连上它
    python3 scripts/sim-client.py                    # Unix,用默认 socket
    python3 scripts/sim-client.py /tmp/sim_cli.sock  # Unix,指定 socket 路径
    python3 scripts/sim-client.py 127.0.0.1:7899     # TCP(含 ':' 即按 host:port)

在终端 B 里键入(回车发送):
    help                 → 发一条命令,输出会随 window 帧回来
    {"type":"view","count":10,"follow":true}   → 直接发原始 JSON(以 { 开头即原样发送)
    {"type":"grep","expr":"/error/"}           → 服务端 grep 板子上的归档
    exit                 → 断开(只断本连接,后端继续常驻)
"""
import json
import os
import socket
import sys
import threading


def reader(sock: socket.socket) -> None:
    """打印服务端推来的每一帧(共享状态 / 视口 / 握手 / 错误)。"""
    buf = b""
    while True:
        try:
            chunk = sock.recv(65536)
        except OSError:
            break
        if not chunk:
            print("\n[server closed]")
            os._exit(0)
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                print("  <non-json>", line[:200])
                continue
            render(msg)


def render(msg: dict) -> None:
    """把一帧渲染成人类可读的一行(仅示例,真前端会画得更漂亮)。"""
    t = msg.get("type")
    if t == "hello":
        print(f"[hello] version={msg.get('version')} protocol={msg.get('protocol')}")
    elif t == "shared":
        tabs = " ".join(
            ("*" if i == msg.get("active_index") else " ") + tool["name"]
            for i, tool in enumerate(msg.get("tools", []))
        )
        print(f"[shared] tabs: {tabs}   mode={msg.get('mode')}")
    elif t == "window":
        lines = []
        for m in msg.get("messages", []):
            body = m.get("body", {})
            kind = body.get("kind")
            if kind == "system":
                lines.append(body.get("text", ""))
            elif kind == "assistant":
                lines.append(body.get("text", ""))
            elif kind == "tool":
                lines.append(f"<tool {body.get('name')} {body.get('status')}>")
        tail = lines[-6:]
        print(f"[window] tab={msg.get('tab')} total_lines={msg.get('total')} "
              f"shown={len(msg.get('messages', []))}")
        for ln in tail:
            for sub in ln.splitlines() or [""]:
                print("   |", sub)
    elif t == "error":
        print(f"[error] {msg.get('code')}: {msg.get('detail')}")
    else:
        print("[?]", msg)


def connect(target: str) -> socket.socket:
    """按目标形态选择传输:含 ':' → TCP host:port;否则 → Unix socket 路径。"""
    if ":" in target:
        host, _, port = target.rpartition(":")
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.connect((host or "127.0.0.1", int(port)))
    else:
        # AF_UNIX 在部分平台(如老 Windows)不可用。
        if not hasattr(socket, "AF_UNIX"):
            raise SystemExit(
                "this platform has no Unix sockets; connect over TCP, e.g. "
                "python3 scripts/sim-client.py 127.0.0.1:7899"
            )
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(target)
    return sock


def main() -> None:
    target = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
        "SIM_CLI_TCP"
    ) or os.environ.get("SIM_CLI_SOCK", "/tmp/sim_cli.sock")
    sock = connect(target)
    print(f"[connected] {target}")

    threading.Thread(target=reader, args=(sock,), daemon=True).start()

    # 连上先声明一个视口(全量、跟随底部),这样服务端会持续推 window。
    sock.sendall((json.dumps({"type": "view", "count": 0, "follow": True}) + "\n").encode())

    for line in sys.stdin:
        line = line.rstrip("\n")
        if not line:
            continue
        if line.lstrip().startswith("{"):
            # 直接发原始 JSON(方便手测各种帧)。
            payload = line
        else:
            # 普通文本当作 input 命令。
            payload = json.dumps({"type": "input", "text": line})
        try:
            sock.sendall((payload + "\n").encode())
        except OSError:
            break
        if line.strip() == "exit":
            break


if __name__ == "__main__":
    main()
