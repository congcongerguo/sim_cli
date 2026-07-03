use std::io::Result;

fn main() -> Result<()> {
    // prost-build shells out to the system `protoc` (on PATH — libprotoc 22.3).
    // Output lands in OUT_DIR/sim.rs, included by src/proto.rs.
    prost_build::Config::new()
        .compile_protos(&["proto/sim.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/sim.proto");
    Ok(())
}
