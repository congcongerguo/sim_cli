/// 应用模式。
///
/// 目前仅区分普通模式和 Plan 模式，用于将来控制 LLM 的行为差异。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 普通模式：LLM 正常执行工具调用
    Normal,
    /// Plan 模式：LLM 只输出计划，不执行实际操作
    Plan,
}
