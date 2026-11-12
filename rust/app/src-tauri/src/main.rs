// No console window behind Kestrel in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kestrel_app_lib::run();
}
