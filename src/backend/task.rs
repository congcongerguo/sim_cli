use std::time::Duration;

use tokio::sync::{mpsc, watch};

use super::chat::ChatState;
use super::conn::{ConnState, ConnSubsystem};

/// Lightweight task metadata for the tab bar (no message history).
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub name: String,
    pub demo_running: bool,
    pub conn: ConnState,
}

/// One task tab: owns its own conversation log and optional transport.
pub struct Task {
    pub name: String,
    pub chat: ChatState,
    pub conn: ConnSubsystem,
    pub demo_running: bool,
    pub demo_cancel: Option<watch::Sender<bool>>,
}

/// Collection of tasks with an active tab.
pub struct TaskManager {
    pub tasks: Vec<Task>,
    pub active: usize,
    demo_tick_tx: mpsc::Sender<usize>,
    pub demo_tick_rx: mpsc::Receiver<usize>,
}

impl TaskManager {
    pub fn new(model: String) -> Self {
        let (demo_tick_tx, demo_tick_rx) = mpsc::channel(64);
        let make_task = |name: &str, hint: &str| {
            let mut chat = ChatState::new(model.clone());
            // Replace the generic welcome with a tab-specific message.
            chat.messages.clear();
            chat.push_system(format!("[{name}] {hint}"));
            chat.push_system("type 'help' for commands, ←/→ to switch tabs");
            Task {
                name: name.into(),
                chat,
                conn: ConnSubsystem::new(),
                demo_running: false,
                demo_cancel: None,
            }
        };
        Self {
            tasks: vec![
                make_task("main", "general       —  model / plan / demo"),
                make_task("conn", "transport     —  con zmq|tcp / close / send"),
                make_task("demo", "log demo      —  start / stop"),
            ],
            active: 0,
            demo_tick_tx,
            demo_tick_rx,
        }
    }

    pub fn active(&self) -> &Task {
        &self.tasks[self.active]
    }

    pub fn active_mut(&mut self) -> &mut Task {
        &mut self.tasks[self.active]
    }

    pub fn active_name(&self) -> &str {
        &self.tasks[self.active].name
    }

    pub fn active_chat_mut(&mut self) -> &mut ChatState {
        &mut self.tasks[self.active].chat
    }

    /// Add a new task tab. Returns an error if the name already exists.
    #[allow(dead_code)]
    pub fn add(&mut self, name: String, model: String) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("task name must not be empty".into());
        }
        if self.tasks.iter().any(|t| t.name == name) {
            return Err(format!("task '{name}' already exists"));
        }
        let task = Task {
            name,
            chat: ChatState::new(model),
            conn: ConnSubsystem::new(),
            demo_running: false,
            demo_cancel: None,
        };
        self.tasks.push(task);
        Ok(())
    }

    /// Switch to the named task. Returns an error if not found.
    pub fn switch_to(&mut self, name: &str) -> Result<(), String> {
        let pos = self
            .tasks
            .iter()
            .position(|t| t.name == name)
            .ok_or_else(|| format!("task '{name}' not found"))?;
        self.active = pos;
        Ok(())
    }

    /// Close the named task. The active task cannot be closed unless it is the
    /// last remaining task (in which case the operation is refused).
    #[allow(dead_code)]
    pub fn close(&mut self, name: &str) -> Result<(), String> {
        if self.tasks.len() <= 1 {
            return Err("cannot close the last task".into());
        }
        let pos = self
            .tasks
            .iter()
            .position(|t| t.name == name)
            .ok_or_else(|| format!("task '{name}' not found"))?;

        // Stop demo if running on the task being closed.
        if let Some(tx) = self.tasks[pos].demo_cancel.take() {
            let _ = tx.send(true);
        }

        self.tasks.remove(pos);
        if self.active >= self.tasks.len() {
            self.active = self.tasks.len().saturating_sub(1);
        }
        // If we removed a task before the active one, shift the index back.
        if pos < self.active {
            self.active = self.active.saturating_sub(1);
        }
        Ok(())
    }

    /// Snapshot task metadata for the tab bar.
    pub fn list(&self) -> Vec<TaskInfo> {
        self.tasks
            .iter()
            .map(|t| TaskInfo {
                name: t.name.clone(),
                demo_running: t.demo_running,
                conn: t.conn.conn.clone(),
            })
            .collect()
    }

    /// Start the demo logger on the active task.
    pub fn start_demo(&mut self) -> Result<(), String> {
        let task = &mut self.tasks[self.active];
        if task.demo_running {
            return Err("demo already running on this task".into());
        }

        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let tick_tx = self.demo_tick_tx.clone();
        let task_idx = self.active;

        task.demo_running = true;
        task.demo_cancel = Some(cancel_tx);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if tick_tx.send(task_idx).await.is_err() {
                            break;
                        }
                    }
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop the demo logger on the active task.
    pub fn stop_demo(&mut self) -> Result<(), String> {
        let task = &mut self.tasks[self.active];
        if !task.demo_running {
            return Err("demo is not running on this task".into());
        }
        if let Some(tx) = task.demo_cancel.take() {
            let _ = tx.send(true);
        }
        task.demo_running = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mgr() -> TaskManager {
        TaskManager::new("mock-claude".into())
    }

    #[test]
    fn starts_with_three_fixed_tasks() {
        let mgr = make_mgr();
        assert_eq!(mgr.tasks.len(), 3);
        assert_eq!(mgr.active_name(), "main");
        let names: Vec<&str> = mgr.tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["main", "conn", "demo"]);
    }

    #[test]
    fn cannot_add_duplicate_name() {
        let mut mgr = make_mgr();
        assert!(mgr.add("main".into(), "mock-claude".into()).is_err());
    }

    #[test]
    fn cannot_add_empty_name() {
        let mut mgr = make_mgr();
        assert!(mgr.add("  ".into(), "mock-claude".into()).is_err());
    }

    #[test]
    fn add_and_switch() {
        let mut mgr = make_mgr();
        mgr.add("extra".into(), "mock-claude".into()).unwrap();
        assert_eq!(mgr.tasks.len(), 4);
        mgr.switch_to("extra").unwrap();
        assert_eq!(mgr.active_name(), "extra");
        // Switch back to a fixed task
        mgr.switch_to("conn").unwrap();
        assert_eq!(mgr.active_name(), "conn");
    }

    #[test]
    fn switch_to_missing_errors() {
        let mut mgr = make_mgr();
        assert!(mgr.switch_to("nope").is_err());
    }

    #[test]
    fn switch_between_fixed_tasks() {
        let mut mgr = make_mgr();
        mgr.switch_to("demo").unwrap();
        assert_eq!(mgr.active_name(), "demo");
        mgr.switch_to("main").unwrap();
        assert_eq!(mgr.active_name(), "main");
    }

    #[test]
    fn close_last_task_refused() {
        let mut mgr = make_mgr();
        // Close two, then try to close the last
        mgr.close("conn").unwrap();
        mgr.close("demo").unwrap();
        assert_eq!(mgr.tasks.len(), 1);
        assert!(mgr.close("main").is_err());
    }

    #[test]
    fn close_task_adjusts_active() {
        let mut mgr = make_mgr();
        // active=0 ("main"), close zmq (index 1), active stays at 0
        mgr.close("conn").unwrap();
        assert_eq!(mgr.active_name(), "main");
        assert_eq!(mgr.tasks.len(), 2);
        // Switch to tcp (index 1), then close main (index 0)
        mgr.switch_to("demo").unwrap();
        mgr.close("main").unwrap();
        // active should shift back to 0 (tcp)
        assert_eq!(mgr.active_name(), "demo");
    }

    #[test]
    fn close_active_task_switches() {
        let mut mgr = make_mgr();
        // active=0 ("main"), close it → active becomes "conn" (index 0 now)
        mgr.close("main").unwrap();
        assert_eq!(mgr.active_name(), "conn");
        assert_eq!(mgr.tasks.len(), 2);
    }

    #[tokio::test]
    async fn demo_start_stop() {
        let mut mgr = make_mgr();
        assert!(mgr.start_demo().is_ok());
        assert!(mgr.tasks[0].demo_running);
        assert!(mgr.start_demo().is_err()); // already running
        assert!(mgr.stop_demo().is_ok());
        assert!(!mgr.tasks[0].demo_running);
        assert!(mgr.stop_demo().is_err()); // not running
    }

    #[tokio::test]
    async fn closing_demo_task_cancels_demo() {
        let mut mgr = make_mgr();
        mgr.start_demo().unwrap();
        assert!(mgr.tasks[0].demo_running);
        mgr.close("main").unwrap();
        // "main" was removed, zmq is now active
        assert_eq!(mgr.active_name(), "conn");
        assert_eq!(mgr.tasks.len(), 2);
    }
}
