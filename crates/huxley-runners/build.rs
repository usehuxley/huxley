//! Compiles the runner protocol with protox (no `protoc` needed).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/runner.proto");
    let files = protox::compile(["proto/runner.proto"], ["proto"])?;
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_fds(files)?;
    Ok(())
}
