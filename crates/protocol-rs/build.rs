use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir.join("..").join("..").join("proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);

    tonic_prost_build::configure().compile_protos(
        &[
            proto_root.join("common.proto"),
            proto_root.join("handshake.proto"),
            proto_root.join("system.proto"),
            proto_root.join("workspace.proto"),
            proto_root.join("task.proto"),
        ],
        &[proto_root],
    )?;

    Ok(())
}
