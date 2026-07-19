/// 静态 tool 定义，由 build.rs 从 tasks.toml 生成。
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub hint: &'static str,
    #[allow(dead_code)]
    pub zmq_sub_addr: Option<&'static str>,
    #[allow(dead_code)]
    pub zmq_pub_addr: Option<&'static str>,
    pub tcp_addr: Option<&'static str>,
}

impl ToolDef {
    pub fn tcp_addr(&self) -> &str {
        self.tcp_addr.unwrap_or("127.0.0.1:7878")
    }

    #[allow(dead_code)]
    pub fn zmq_sub_addr(&self) -> &str {
        self.zmq_sub_addr.unwrap_or("tcp://127.0.0.1:5555")
    }

    #[allow(dead_code)]
    pub fn zmq_pub_addr(&self) -> &str {
        self.zmq_pub_addr.unwrap_or("tcp://127.0.0.1:5556")
    }
}

// 由 build.rs 生成
include!(concat!(env!("OUT_DIR"), "/tool_defs.rs"));
