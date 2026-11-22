// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same binary doubles as the update helper: with `--kadr-swap` it
    // replaces the exe of a process that just exited and starts it again. This
    // check has to happen before anything else touches the app state.
    kensaku_lib::self_update::run_helper_if_asked();
    kensaku_lib::run()
}
