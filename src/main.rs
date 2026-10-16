//! AGVPN — unofficial desktop client for AdGuard VPN on Linux.

#![allow(clippy::type_complexity)]

mod app;
mod cli;
mod consts;
mod controller;
mod data;
mod i18n;
mod i18n_ru;
mod logger;
mod parsers;
mod settings;
mod stats;
mod system;
mod tray;
mod ui;
mod updater;
mod util;

fn main() -> gtk::glib::ExitCode {
    logger::init();
    app::run()
}
