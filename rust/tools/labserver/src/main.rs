//! `cargo run -p labserver` starts the lab server for manual tests and benchmarks.
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let server = labserver::LabServer::start().await?;
    println!("Lab server on http://{}", server.addr);
    println!("Example: {}", server.url("big.bin", "size=1073741824"));
    tokio::signal::ctrl_c().await?;
    Ok(())
}
