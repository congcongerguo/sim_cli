#[path = "../json_framer.rs"]
mod json_framer;

use std::env;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use json_framer::JsonFramer;

const MAX_PENDING: usize = 16 * 1024 * 1024;
const READ_CHUNK: usize = 4096;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7878".into());
    let listener = TcpListener::bind(&addr).await?;
    println!("echo server listening on {addr} (JSON brace-framed)");

    loop {
        let (sock, peer) = listener.accept().await?;
        println!("[+] {peer} connected");
        tokio::spawn(async move {
            match handle(sock).await {
                Ok(_) => println!("[-] {peer} disconnected"),
                Err(e) => eprintln!("[-] {peer} error: {e}"),
            }
        });
    }
}

async fn handle(sock: TcpStream) -> std::io::Result<()> {
    let (mut read, mut write) = sock.into_split();
    let mut framer = JsonFramer::new();
    let mut chunk = [0u8; READ_CHUNK];

    loop {
        let n = read.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        framer.feed(&chunk[..n]);
        if framer.pending_bytes() > MAX_PENDING {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("buffer exceeded {MAX_PENDING} bytes without a complete message"),
            ));
        }
        while let Some(msg) = framer.next_message() {
            write.write_all(&msg).await?;
            write.flush().await?;
        }
    }
}
