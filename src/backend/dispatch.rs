//! Single source of truth for command-to-effect mapping.

use crate::commands::Action;
use crate::message::LogLevel;

use super::Backend;
use super::conn;

fn emit_conn(chat: &mut super::chat::ChatState, outs: &[super::conn::ConnOutcome]) {
    for o in outs {
        let (text, level) = conn::format(o);
        chat.push_system(text, level);
    }
}

pub fn run_action(b: &mut Backend, action: Action) {
    match action {
        Action::Help => {
            let task_name = b.tasks.active_name().to_string();
            b.tasks.active_mut()
                .chat
                .push_system(crate::help::full_help(&task_name), LogLevel::Info);
        }
        Action::Clear => {
            b.tasks.active_mut().chat.clear();
        }
        Action::Exit => b.should_quit = true,
        Action::Model(m) => {
            b.tasks.active_mut().chat.set_model(m);
            let model = b.tasks.active().chat.model.clone();
            b.tasks.active_mut()
                .chat
                .push_system(format!("model -> {model}"), LogLevel::Notice);
        }
        Action::Plan(t) => {
            b.tasks.active_mut().chat.set_plan(t);
            let mode = format!("{:?}", b.tasks.active().chat.mode);
            b.tasks.active_mut()
                .chat
                .push_system(format!("mode -> {mode}"), LogLevel::Notice);
        }
        Action::Demo(s) => {
            let chat = &mut b.tasks.active_mut().chat;
            b.llm.start_demo(s, chat);
        }
        Action::Connect(p) => {
            let outs = b.tasks.active_mut().conn.connect(p);
            emit_conn(&mut b.tasks.active_mut().chat, &outs);
        }
        Action::Disconnect => {
            let outs = b.tasks.active_mut().conn.disconnect();
            emit_conn(&mut b.tasks.active_mut().chat, &outs);
        }
        Action::Send => {
            let outs = b.tasks.active_mut().conn.send_ping();
            emit_conn(&mut b.tasks.active_mut().chat, &outs);
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
                        .push_system(
                            format!("── switched: {from} → {to} ──"),
                            LogLevel::Notice,
                        );
                }
                Err(e) => {
                    b.tasks.active_mut()
                        .chat
                        .push_system(format!("error: {e}"), LogLevel::Error);
                }
            }
        }
        Action::Start => match b.tasks.start_demo() {
            Ok(()) => {
                let name = b.tasks.active_name().to_string();
                b.tasks.active_mut()
                    .chat
                    .push_system(
                        format!("demo started on '{name}' — logging every 1s"),
                        LogLevel::Notice,
                    );
            }
            Err(e) => {
                b.tasks.active_mut()
                    .chat
                    .push_system(format!("error: {e}"), LogLevel::Error);
            }
        },
        Action::Stop => match b.tasks.stop_demo() {
            Ok(()) => {
                let name = b.tasks.active_name().to_string();
                b.tasks.active_mut()
                    .chat
                    .push_system(format!("demo stopped on '{name}'"), LogLevel::Notice);
            }
            Err(e) => {
                b.tasks.active_mut()
                    .chat
                    .push_system(format!("error: {e}"), LogLevel::Error);
            }
        },
    }
}
