//! Downloads one address with the engine's defaults (8 connections) and prints how long it took.
//! Used by scripts/Bench-Engine.ps1:  fetch <url> <file>

use kestrel_engine::{Engine, HostLimits, Job, Options};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(url), Some(file)) = (args.next(), args.next()) else {
        eprintln!("usage: fetch <url> <file>");
        std::process::exit(2);
    };
    let started = std::time::Instant::now();
    let engine = Engine::new(Options::default(), HostLimits::default());
    let mut job = Job::new(url.parse().expect("address"), &file);
    match engine.download(&mut job, |_| {}, CancellationToken::new()).await {
        Ok(_) => println!("{}", started.elapsed().as_millis()),
        Err(error) => {
            eprintln!("failed: {error}");
            std::process::exit(1);
        }
    }
}
