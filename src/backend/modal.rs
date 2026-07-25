use std::collections::HashSet;

use crate::log_buffer::LogBuffer;
use crate::message::{Message, ToolCall, ToolStatus};

/// 权限弹窗中用户的选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalChoice {
    /// 允许本次执行
    Yes,
    /// 拒绝本次执行
    No,
    /// 始终允许（加入白名单，以后不再弹窗）
    Always,
}

/// 权限弹窗的展示内容。
///
/// 当 LLM 请求执行一个需要用户确认的工具时，
/// 前端会显示此弹窗，展示工具名称和参数预览。
#[derive(Debug, Clone)]
pub struct ModalRequest {
    /// 对应 [`LogBuffer`] 中 Tool 消息的索引
    #[allow(dead_code)]
    pub tool_index: usize,
    /// 工具名称
    pub tool_name: String,
    /// 参数预览文本
    pub args_preview: String,
}

/// 权限弹窗子系统的生命周期管理。
///
/// 负责：
/// - 维护"始终允许"白名单
/// - 跟踪当前弹窗请求
/// - 在 LLM 请求工具时创建待审批的 Tool 卡片
/// - 处理用户的批准/拒绝决定
///
/// 仅在 `mock-llm` feature 启用时活跃。
#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
pub struct ModalSubsystem {
    /// 当前待处理的权限请求，`None` 表示无弹窗
    pub request: Option<ModalRequest>,
    /// "始终允许"工具名称白名单
    #[allow(dead_code)]
    allow_always: HashSet<String>,
}

#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
impl ModalSubsystem {
    pub fn new() -> Self {
        Self {
            request: None,
            allow_always: HashSet::new(),
        }
    }

    /// LLM 请求执行工具。
    ///
    /// 将 ToolCall 卡片推入聊天日志。如果该工具在白名单中则自动批准，
    /// 否则设置为等待权限状态并弹出确认弹窗。
    pub fn start_tool(
        &mut self,
        tool_name: String,
        args_preview: String,
        chat: &mut LogBuffer,
    ) {
        let auto = self.allow_always.contains(&tool_name);
        let status = if auto {
            ToolStatus::Running
        } else {
            ToolStatus::AwaitingPermission
        };
        chat.push(Message::Tool(ToolCall {
            name: tool_name.clone(),
            args_preview: args_preview.clone(),
            status,
            output: String::new(),
        }));
        if !auto {
            let idx = chat.len() - 1;
            self.request = Some(ModalRequest {
                tool_index: idx,
                tool_name,
                args_preview,
            });
        }
    }

    /// 工具执行完毕，将结果写入 Tool 卡片。
    ///
    /// 更新最后一个 Tool 消息的状态为 Done 并填入输出，
    /// 然后追加一条空的 Assistant 消息作为 LLM 回复的占位。
    pub fn finish_tool(&mut self, output: String, chat: &mut LogBuffer) {
        if let Some(Message::Tool(t)) = chat.last_mut()
            && t.status != ToolStatus::Denied
        {
            t.status = ToolStatus::Done;
            t.output = output;
        }
        chat.push(Message::Assistant {
            text: String::new(),
            streaming: true,
        });
    }

    /// 处理用户在权限弹窗中的选择。
    ///
    /// - Yes：批准本次执行
    /// - Always：批准并加入白名单
    /// - No：拒绝执行，标记为 Denied
    pub fn resolve(&mut self, choice: ModalChoice, chat: &mut LogBuffer) {
        let req = match self.request.take() {
            Some(r) => r,
            None => return,
        };
        if let Some(Message::Tool(t)) = chat.get_mut(req.tool_index) {
            match choice {
                ModalChoice::Yes => t.status = ToolStatus::Running,
                ModalChoice::Always => {
                    t.status = ToolStatus::Running;
                    self.allow_always.insert(t.name.clone());
                }
                ModalChoice::No => {
                    t.status = ToolStatus::Denied;
                    t.output = "(permission denied)".into();
                }
            }
        }
    }
}
