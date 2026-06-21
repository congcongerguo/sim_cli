//! Single source of truth for command-to-effect mapping.
//!
//! When you add a new [`Action`] variant the match below stops compiling
//! until you handle it — by design.

use crate::commands::Action;
use crate::transport::TransportEvent;

use super::Backend;
use super::conn;

pub fn run_action(b: &mut Backend, action: Action) {
    match action {
        Action::Help => b.chat.push_system(crate::help::full_help()),
        Action::Clear => b.chat.clear(),
        Action::Exit => b.should_quit = true,
        Action::Model(m) => {
            b.chat.set_model(m);
            b.chat.push_system(format!("model -> {}", b.chat.model));
        }
        Action::Plan(t) => {
            b.chat.set_plan(t);
            b.chat.push_system(format!("mode -> {:?}", b.chat.mode));
        }
        Action::Demo(s) => b.llm.start_demo(s, &mut b.chat),
        Action::Connect(p) => {
            let outs = b.conn.connect(p);
            emit_conn(b, outs);
        }
        Action::Disconnect => {
            let outs = b.conn.disconnect();
            emit_conn(b, outs);
        }
        Action::Send => {
            let outs = b.conn.send_ping();
            emit_conn(b, outs);
        }
    }
}

pub fn apply_transport_event(b: &mut Backend, ev: TransportEvent) {
    let outs = b.conn.handle_event(ev);
    emit_conn(b, outs);
}

fn emit_conn(b: &mut Backend, outs: Vec<conn::ConnOutcome>) {
    for o in outs {
        b.chat.push_system(conn::format(&o));
    }
}
