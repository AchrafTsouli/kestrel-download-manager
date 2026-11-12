//! Kestrel from the command line. It talks to the running Kestrel the way the browser bridge does
//! (the per-user pipe or socket) and starts Kestrel in the tray when it is not running.
//!
//!     kestrel-cli add <url> [<url> …]     magnet links too; Kestrel's start dialog shows when enabled
//!     kestrel-cli list                    the download list
//!     kestrel-cli pause-all | resume-all
//!     kestrel-cli show | quit

use std::path::PathBuf;
use std::time::Duration;

use kestrel_ipc::{Request, Response, read_frame, write_frame};

const USAGE: &str = "Kestrel from the command line

  kestrel-cli add <url> [<url> ...]   download addresses or magnet links
  kestrel-cli list                    show the download list
  kestrel-cli pause-all               pause every download
  kestrel-cli resume-all              resume every download
  kestrel-cli show                    open Kestrel's window
  kestrel-cli quit                    close Kestrel";

fn app_path() -> PathBuf {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    ["Kestrel.exe", "kestrel.exe", "kestrel"].iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join("kestrel"))
}

/// A random request ID (the bridge's GUID form), so a repeated command never adds twice.
fn request_id() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("system random numbers");
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

async fn send(request: &Request, start: bool) -> Result<Response, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut started = false;
    loop {
        match kestrel_ipc::pipe::connect(Duration::from_millis(500)).await {
            Ok(mut link) => {
                write_frame(&mut link, request).await.map_err(|e| e.to_string())?;
                return match tokio::time::timeout(Duration::from_secs(150), read_frame::<Response>(&mut link)).await {
                    Ok(Ok(Some(response))) => Ok(response),
                    Ok(_) => Err("Kestrel closed the connection.".into()),
                    Err(_) => Err("Kestrel did not answer in time.".into()),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Err(e.to_string()),
            Err(_) if !start => return Err("Kestrel is not running.".into()),
            Err(_) => {
                if !started {
                    started = true;
                    kestrel_ipc::start_app(&app_path()).map_err(|_| "Kestrel was not found beside kestrel-cli.".to_string())?;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err("Kestrel did not start in time.".into());
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} B"),
    }
}

fn print_list(list: &serde_json::Value) {
    let Some(items) = list.as_array().filter(|a| !a.is_empty()) else {
        println!("No downloads.");
        return;
    };
    for d in items {
        let (written, total) = (d["written"].as_u64().unwrap_or(0), d["total"].as_u64());
        let progress = match total {
            Some(t) if t > 0 => format!("{:>5.1}%  {} / {}", written as f64 * 100.0 / t as f64, size(written), size(t)),
            _ => size(written),
        };
        let speed = d["bytesPerSecond"].as_f64().filter(|s| *s > 0.0).map(|s| format!("  {}/s", size(s as u64))).unwrap_or_default();
        println!("{:<12} {:<44} {progress}{speed}", d["status"].as_str().unwrap_or(""), d["name"].as_str().unwrap_or(""));
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("");
    let requests: Vec<Request> = match command {
        "add" if args.len() > 1 => args[1..].iter().map(|url| Request { action: "download".into(), url: Some(url.clone()), request_id: Some(request_id()), ..Default::default() }).collect(),
        "list" | "pause-all" | "resume-all" | "show" | "quit" => vec![Request { action: command.into(), ..Default::default() }],
        _ => {
            println!("{USAGE}");
            std::process::exit(if command.is_empty() || command == "help" || command == "--help" { 0 } else { 2 });
        }
    };
    let mut failed = false;
    for request in &requests {
        if let Err(error) = kestrel_ipc::validate(request) {
            eprintln!("{}: {error}", request.url.as_deref().unwrap_or(command));
            failed = true;
            continue;
        }
        // Closing or listing a Kestrel that is not running should not start it.
        let start = !matches!(command, "quit" | "list");
        match send(request, start).await {
            Ok(response) if response.ok => match command {
                "list" => print_list(&response.downloads.unwrap_or_default()),
                "add" => println!("Added {}", request.url.as_deref().unwrap_or("")),
                _ => println!("Done."),
            },
            Ok(response) => {
                eprintln!("{}: {}", request.url.as_deref().unwrap_or(command), response.error.unwrap_or_else(|| "Kestrel refused it.".into()));
                failed = true;
            }
            Err(error) => {
                eprintln!("{error}");
                failed = true;
            }
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    #[test]
    fn request_ids_are_accepted_by_the_bridge() {
        let request = kestrel_ipc::Request { action: "download".into(), url: Some("https://example.test/a.zip".into()), request_id: Some(super::request_id()), ..Default::default() };
        assert!(kestrel_ipc::validate(&request).is_ok());
        assert_ne!(super::request_id(), super::request_id());
    }
}
