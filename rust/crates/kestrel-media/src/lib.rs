//! Kestrel's video support: which pages hold videos, their qualities and playlists, and downloads
//! (with clips) through the bundled yt-dlp and FFmpeg. A port of Flux 1.x's video code.

pub mod formats;
pub mod pages;
pub mod tools;
pub mod update;

pub use formats::{Catalog, Playlist, PlaylistEntry, Probe, Selection};
pub use tools::{MediaEvent, Outcome, Tools, VideoJob};
