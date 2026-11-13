//! A real torrent through the built-in engine (librqbit). It needs the network, so it runs only on
//! request (and every night in CI):
//!
//!     cargo test -p kestrel-torrent --test real_torrent -- --ignored --nocapture

use kestrel_torrent::{Outcome, Selection, Tools, metadata_path};
use tokio_util::sync::CancellationToken;

const SINTEL: &str = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev";

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn only_the_chosen_files_of_a_magnet_arrive_and_a_pause_continues() {
    let root = std::env::temp_dir().join(format!("kestrel-torrent-test-{}", std::process::id()));
    let tools = Tools::new(root.join("cache"));
    let cancel = CancellationToken::new();
    let info = tokio::time::timeout(std::time::Duration::from_secs(180), tools.load(SINTEL, &cancel)).await.expect("file list in 3 minutes").unwrap();
    assert_eq!(info.info_hash, "08ada5a7a6183aae1e09d831df6748d566095a10");
    println!("{} files: {:#?}", info.files.len(), info.files);

    // The film and the smallest file; everything else must be left out.
    let mut by_size = info.files.clone();
    by_size.sort_by_key(|f| f.length);
    let chosen: Vec<_> = vec![&by_size[0], by_size.last().unwrap()];
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

    // Paused once on the way, then continued to the end.
    let pause = CancellationToken::new();
    let stopper = pause.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        stopper.cancel();
    });
    let first = tools.download(&selection, &folder, 2 << 20, |_| {}, pause).await.unwrap();
    assert_eq!(first, Outcome::Paused, "paused part way (2 MB/s for 4 s)");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(600), tools.download(&selection, &folder, 0, |_| {}, cancel)).await.expect("done in 10 minutes").unwrap();
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
            // A tiny file can lie entirely inside pieces of chosen ones: then it arrives whole. Never a
            // partial one, and never the big unchosen film.
            assert!(size.is_none() || size == Some(file.length), "a partial unchosen file was left: {}", on_disk.display());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
