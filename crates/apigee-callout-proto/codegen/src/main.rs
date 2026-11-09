//! Regenerates `src/apigee.rs` from `proto/external_callout.proto`.

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let proto = root.join("proto");
    let files = protox::compile([proto.join("external_callout.proto")], [&proto])
        .expect("the .proto compiles");
    tonic_prost_build::configure()
        // rail-apigee serves it; its tests call it.
        .build_server(true)
        .build_client(true)
        // The client's `connect` would need tonic's transport.
        .build_transport(false)
        .out_dir(root.join("src"))
        .compile_fds(files)
        .expect("the code generates");
}
