//! Single source of truth for command-to-effect mapping.
//!
//! When you add a new [`Action`] variant the match below stops compiling
//! until you handle it — by design.

use crate::commands::Action;

use super::Backend;
use super::conn;

pub fn run_action(b: &mut Backend, action: Action) {
    match action {
        Action::Help => {
            let task_name = b.tasks.active_name().to_string();
            b.tasks.active_mut().chat.push_system(crate::help::full_help(&task_name));
        }
        Action::Clear => {
            b.tasks.active_mut().chat.clear();
        }
        Action::Exit => b.should_quit = true,
        Action::Model(m) => {
            b.tasks.active_mut().chat.set_model(m);
            let model = b.tasks.active().chat.model.clone();
            b.tasks.active_mut().chat.push_system(format!("model -> {model}"));
        }
        Action::Plan(t) => {
            b.tasks.active_mut().chat.set_plan(t);
            let mode = format!("{:?}", b.tasks.active().chat.mode);
            b.tasks.active_mut().chat.push_system(format!("mode -> {mode}"));
        }
        Action::Demo(s) => {
            let chat = &mut b.tasks.active_mut().chat;
            b.llm.start_demo(s, chat);
        }
        Action::Connect(p) => {
            let outs = b.tasks.active_mut().conn.connect(p);
            for o in outs {
                b.tasks.active_mut().chat.push_system(conn::format(&o));
            }
        }
        Action::Disconnect => {
            let outs = b.tasks.active_mut().conn.disconnect();
            for o in outs {
                b.tasks.active_mut().chat.push_system(conn::format(&o));
            }
        }
        Action::Send => {
            let outs = b.tasks.active_mut().conn.send_ping();
            for o in outs {
                b.tasks.active_mut().chat.push_system(conn::format(&o));
            }
        }
        Action::TaskSwitch(name) => {
            if name.trim().is_empty() {
                return;
            }
            let from = b.tasks.active_name().to_string();
            match b.tasks.switch_to(&name) {
                Ok(()) => {
                    let to = b.tasks.active_name().to_string();
                    b.tasks.active_mut()
                        .chat
                        .push_system(format!("── switched: {from} → {to} ──"));
                }
                Err(e) => {
                    b.tasks.active_mut().chat.push_system(format!("error: {e}"));
                }
            }
        }
        Action::Start => match b.tasks.start_demo() {
            Ok(()) => {
                let name = b.tasks.active_name().to_string();
                b.tasks.active_mut()
                    .chat
                    .push_system(format!("demo started on '{name}' — logging every 1s"));
            }
            Err(e) => {
                b.tasks.active_mut().chat.push_system(format!("error: {e}"));
            }
        },
        Action::Stop => match b.tasks.stop_demo() {
            Ok(()) => {
                let name = b.tasks.active_name().to_string();
                b.tasks.active_mut()
                    .chat
                    .push_system(format!("demo stopped on '{name}'"));
            }
            Err(e) => {
                b.tasks.active_mut().chat.push_system(format!("error: {e}"));
            }
        },
    }
}
