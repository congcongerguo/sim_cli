//! Generated protobuf bindings, compiled by `build.rs` via prost-build.
//! The `sim` module (from `package sim;` in `proto/sim.proto`) holds `Tick`,
//! the wire type carried on the ZMQ "pb" topic.

pub mod sim {
    include!(concat!(env!("OUT_DIR"), "/sim.rs"));
}
