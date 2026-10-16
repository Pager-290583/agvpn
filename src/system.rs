//! Desktop integration: autostart and opening links.

use adw::prelude::*;
use gtk::{gio, glib};

use crate::consts::APP_ID;
use crate::util::IN_FLATPAK;

pub fn open_uri(uri: &str, parent: Option<&gtk::Window>) {
    let launcher = gtk::UriLauncher::new(uri);
    let uri = uri.to_string();
    launcher.launch(parent, gio::Cancellable::NONE, move |res| {
        if let Err(err) = res {
            log::warn!("cannot open {uri}: {}", err.message());
        }
    });
}

pub fn set_autostart(enabled: bool) {
    if *IN_FLATPAK {
        request_background(enabled);
    } else {
        xdg_autostart(enabled);
    }
}

/// org.freedesktop.portal.Background — the sandbox-friendly way to autostart.
fn request_background(autostart: bool) {
    let bus = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(bus) => bus,
        Err(err) => {
            log::warn!("Background portal failed: {}", err.message());
            return;
        }
    };
    let options = glib::VariantDict::new(None);
    options.insert("reason", "Keep the VPN connection managed in the background");
    options.insert("autostart", autostart);
    options.insert("commandline", vec!["agvpn".to_string(), "--background".to_string()]);
    options.insert("dbus-activatable", false);
    let params = glib::Variant::tuple_from_iter(["".to_variant(), options.end()]);
    bus.call(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Background",
        "RequestBackground",
        Some(&params),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        |res| {
            if let Err(err) = res {
                log::warn!("Background portal failed: {}", err.message());
            }
        },
    );
}

fn xdg_autostart(enabled: bool) {
    let path = glib::user_config_dir().join("autostart").join(format!("{APP_ID}.desktop"));
    if !enabled {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "agvpn".into());
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=AGVPN\nExec={exe} --background\nIcon={APP_ID}\nX-GNOME-Autostart-enabled=true\n"
    );
    if let Err(err) = std::fs::write(&path, text) {
        log::warn!("cannot write {}: {err}", path.display());
    }
}
