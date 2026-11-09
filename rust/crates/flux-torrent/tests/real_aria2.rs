//! A real torrent through aria2 (the check Flux 1.0 was released with). It needs the network and
//! aria2c.exe, so it runs only on request:
//!
//!     $env:FLUX_ARIA2="…\aria2c.exe"; cargo test -p flux-torrent --test real_aria2 -- --ignored --nocapture

use flux_torrent::{Outcome, Selection, Tools, metadata_path};
use tokio_util::sync::CancellationToken;

const SINTEL: &str = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev";

#[tokio::test]
#[ignore]
async fn only_the_chosen_files_of_a_magnet_arrive() {
    let root = std::env::temp_dir().join(format!("flux-torrent-test-{}", std::process::id()));
    let tools = Tools::new(std::env::var("FLUX_ARIA2").expect("set FLUX_ARIA2 to aria2c.exe"), root.join("cache"));
    let cancel = CancellationToken::new();
    let info = tokio::time::timeout(std::time::Duration::from_secs(180), tools.load(SINTEL, &cancel)).await.expect("file list in 3 minutes").unwrap();
    assert_eq!(info.info_hash, "08ada5a7a6183aae1e09d831df6748d566095a10");
    println!("{} files: {:#?}", info.files.len(), info.files);

    // The two smallest files; everything else must be left out.
    let mut by_size = info.files.clone();
    by_size.sort_by_key(|f| f.length);
    let chosen: Vec<_> = by_size.iter().take(2).collect();
    let selection = Selection {
        info_hash: info.info_hash.clone(),
        name: info.name.clone(),
        metadata_file: metadata_path(&tools.cache, &info.info_hash),
        files: chosen.iter().map(|f| f.index).collect(),
        selected_bytes: chosen.iter().map(|f| f.length).sum(),
        seed_minutes: 0,
    };
    selection.validate(SINTEL).unwrap();
    let folder = root.join("out");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(600), tools.download(&selection, &folder, 0, |s| println!("{s:?}"), cancel)).await.expect("done in 10 minutes").unwrap();
    assert_eq!(outcome, Outcome::Completed);

    // Paths are relative to the torrent's own folder (a single-file torrent is just the file).
    let path = |p: &str| if info.files.len() == 1 { folder.join(p) } else { folder.join(&info.name).join(p) };
    for file in &info.files {
        let on_disk = path(&file.path);
        let wanted = chosen.iter().any(|c| c.index == file.index);
        let size = std::fs::metadata(&on_disk).map(|m| m.len()).ok();
        if wanted {
            assert_eq!(size, Some(file.length), "{}", on_disk.display());
        } else {
            assert_eq!(size, None, "an unchosen file was kept: {}", on_disk.display());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
