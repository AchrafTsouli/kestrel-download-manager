// No console window behind Flux in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    flux_app_lib::run();
}
