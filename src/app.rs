//! Application object: lifecycle, tray, notifications, quitting.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::consts::{APP_ID, APP_NAME};
use crate::controller::Controller;
use crate::parsers::{version_tuple, State};
use crate::settings::Settings;
use crate::tr;
use crate::tray::{MenuItem, Tray};
use crate::ui::window::MainWindow;
use crate::updater;
use crate::util::{resource_path, run_in_thread};

pub fn apply_theme(settings: &Settings) {
    let scheme = match settings.string("theme").as_str() {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

pub fn notify(app: &impl IsA<gio::Application>, settings: &Settings, tag: &str, title: &str, body: &str) {
    if !settings.bool("notifications") {
        return;
    }
    let notification = gio::Notification::new(title);
    notification.set_body(Some(body));
    notification.set_icon(&gio::ThemedIcon::new(APP_ID));
    notification.set_default_action("app.show");
    app.send_notification(Some(tag), &notification);
}

struct App {
    app: adw::Application,
    settings: Rc<Settings>,
    ctl: Rc<Controller>,
    window: Rc<MainWindow>,
    tray: RefCell<Option<Tray>>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    quitting: Cell<bool>,
    first_status: Cell<bool>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn instance() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.add_main_option("background", glib::Char::from(b'b'), glib::OptionFlags::NONE, glib::OptionArg::None,
                        "Start minimized to the tray", None);
    app.add_main_option("connect", glib::Char::from(0), glib::OptionFlags::NONE, glib::OptionArg::None, "Connect to VPN", None);
    app.add_main_option("disconnect", glib::Char::from(0), glib::OptionFlags::NONE, glib::OptionArg::None, "Disconnect", None);

    app.connect_startup(startup);
    app.connect_command_line(|_app, cmdline| {
        if let Some(this) = instance() {
            this.command_line(&cmdline.options_dict());
        }
        glib::ExitCode::SUCCESS
    });
    app.connect_activate(|_| {
        if let Some(this) = instance() {
            this.show_window();
        }
    });
    let code = app.run();
    APP.with(|a| a.borrow_mut().take());
    code
}

fn startup(app: &adw::Application) {
    let settings = Rc::new(Settings::new());
    crate::i18n::setup(&settings.string("language"));

    if let Some(display) = gdk::Display::default() {
        let icon_theme = gtk::IconTheme::for_display(&display);
        for sub in ["symbolic", "scalable"] {
            icon_theme.add_search_path(resource_path(&format!("data/icons/hicolor/{sub}/apps")));
        }
        let css = gtk::CssProvider::new();
        css.load_from_path(resource_path("data/style.css"));
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    apply_theme(&settings);

    let ctl = Controller::new(settings.clone());
    let window = MainWindow::new(app, ctl.clone());
    let this = Rc::new(App {
        app: app.clone(),
        settings,
        ctl,
        window,
        tray: RefCell::new(None),
        hold: RefCell::new(None),
        quitting: Cell::new(false),
        first_status: Cell::new(true),
    });
    APP.with(|a| *a.borrow_mut() = Some(this.clone()));

    this.ctl.signals.state_transition.connect(|(prev, new)| {
        if let Some(this) = instance() {
            this.on_transition(*prev, *new);
        }
    });
    for signal in [&this.ctl.signals.status_changed, &this.ctl.signals.locations_changed, &this.ctl.signals.license_changed] {
        signal.connect(|_| {
            if let Some(this) = instance() {
                this.update_tray();
            }
        });
    }
    this.settings.changed.connect(|key| {
        if key == "hide_tray_icon" || key == "*" {
            if let Some(this) = instance() {
                this.setup_tray();
            }
        }
    });

    this.add_actions();
    this.window.ui.window.connect_close_request(|_| match instance() {
        Some(this) => this.on_close_request(),
        None => glib::Propagation::Proceed,
    });
    this.setup_tray();
    // Keep running in the background while the window is closed (like the macOS menu bar app).
    *this.hold.borrow_mut() = Some(app.hold());
    this.ctl.ensure_cli_defaults();
    this.ctl.start();
    glib::timeout_add_seconds_local_once(20, || {
        if let Some(this) = instance() {
            this.auto_update_check();
        }
    });
    this.check_helper_version();
}

impl App {
    fn add_actions(self: &Rc<Self>) {
        let actions: [(&str, fn(&Rc<App>), &[&str]); 7] = [
            ("quit", |a| a.request_quit(), &["<Control>q"]),
            ("show", |a| a.show_window(), &[]),
            ("toggle", |a| a.ctl.toggle(), &["<Control>Return"]),
            ("connect", |a| a.ctl.connect_vpn(None, false), &[]),
            ("connect-fastest", |a| a.ctl.connect_vpn(None, true), &[]),
            ("disconnect", |a| a.ctl.disconnect_vpn(None), &[]),
            ("preferences", |a| a.show_tab("settings"), &["<Control>comma"]),
        ];
        for (name, callback, accels) in actions {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| {
                if let Some(this) = instance() {
                    callback(&this);
                }
            });
            self.app.add_action(&action);
            if !accels.is_empty() {
                self.app.set_accels_for_action(&format!("app.{name}"), accels);
            }
        }
    }

    fn command_line(self: &Rc<Self>, options: &glib::VariantDict) {
        let connect = options.contains("connect");
        let disconnect = options.contains("disconnect");
        if connect {
            self.ctl.connect_vpn(None, false);
        } else if disconnect {
            self.ctl.disconnect_vpn(None);
        }
        let background = options.contains("background") && !self.settings.bool("show_window_on_start");
        if !background && !connect && !disconnect {
            self.show_window();
        }
    }

    fn show_window(&self) {
        self.window.show();
    }

    fn show_tab(&self, name: &str) {
        self.show_window();
        self.window.show_tab(name);
    }

    fn toggle_window(&self) {
        let window = &self.window.ui.window;
        if window.is_visible() && window.is_active() {
            window.set_visible(false);
        } else {
            self.show_window();
        }
    }

    // ================================================================== tray

    fn setup_tray(self: &Rc<Self>) {
        if self.settings.bool("hide_tray_icon") {
            if let Some(tray) = self.tray.borrow_mut().take() {
                tray.hide();
            }
            return;
        }
        if self.tray.borrow().is_none() {
            let tray = Tray::new(APP_ID, APP_NAME, || {
                if let Some(this) = instance() {
                    this.toggle_window();
                }
            });
            tray.show();
            *self.tray.borrow_mut() = Some(tray);
        }
        self.update_tray();
    }

    fn update_tray(&self) {
        let Some(tray) = self.tray.borrow().clone() else { return };
        let ctl = &self.ctl;
        let state = ctl.state();
        let connected = matches!(state, State::Connected | State::Reconnecting);
        let status_text = match state {
            State::Connected => tr!("Connected"),
            State::Connecting => tr!("Connecting…"),
            State::Reconnecting => tr!("Waiting for recovery"),
            State::Disconnecting => tr!("Disconnecting…"),
            State::NotLoggedIn => tr!("Not logged in"),
            _ => tr!("Disconnected"),
        };
        let loc = ctl.current_location();
        let loc_text = match &loc {
            Some(l) => format!("{}, {}", l.city, l.country),
            None => ctl.location_title(&ctl.status.borrow().location),
        };

        let suffix = if connected {
            "on"
        } else if matches!(state, State::Connecting | State::Disconnecting) {
            "busy"
        } else {
            "off"
        };
        let icon_name = format!("{APP_ID}-tray-{suffix}");
        tray.set_icon(&icon_name, resource_path(&format!("data/icons/hicolor/scalable/apps/{icon_name}.svg")));
        let tooltip = if connected && !loc_text.is_empty() { format!("{status_text} — {loc_text}") } else { status_text.clone() };
        tray.set_tooltip(APP_NAME, &tooltip);

        let mut items = vec![MenuItem::label(format!("{} {status_text}", tr!("Status:")))];
        if connected && !loc_text.is_empty() {
            items.push(MenuItem::label(format!("{} {loc_text}", tr!("Location:"))));
        }
        items.push(MenuItem::separator());
        let with = |f: fn(&Rc<App>)| {
            move || {
                if let Some(this) = instance() {
                    f(&this);
                }
            }
        };
        if ctl.logged_in.get() == Some(true) {
            let idle = ctl.busy.get().is_none();
            if state.is_active() {
                items.push(MenuItem::action(tr!("Disconnect"), with(|a| a.ctl.disconnect_vpn(None))).enabled(idle));
            } else {
                items.push(MenuItem::action(tr!("Connect"), with(|a| a.ctl.connect_vpn(None, false))).enabled(idle));
            }
            let mut connect_to = vec![MenuItem::action(tr!("Fastest location"), with(|a| a.ctl.connect_vpn(None, true)))];
            let saved = ctl.saved_locations();
            if !saved.is_empty() {
                connect_to.push(MenuItem::separator());
                connect_to.push(MenuItem::label(tr!("Saved locations")));
                for location in saved {
                    let current = connected && loc.as_ref().is_some_and(|l| l.key() == location.key());
                    let label = format!("{}, {}", location.city, location.country);
                    connect_to.push(
                        MenuItem::action(label, move || {
                            if let Some(this) = instance() {
                                this.ctl.connect_vpn(Some(location.clone()), false);
                            }
                        })
                        .checked(current),
                    );
                }
            }
            let fastest = ctl.fastest_locations(5);
            if !fastest.is_empty() {
                connect_to.push(MenuItem::separator());
                connect_to.push(MenuItem::label(tr!("Fastest locations")));
                for location in fastest {
                    let label = format!("{}, {}", location.city, location.country);
                    connect_to.push(MenuItem::action(label, move || {
                        if let Some(this) = instance() {
                            this.ctl.connect_vpn(Some(location.clone()), false);
                        }
                    }));
                }
            }
            connect_to.push(MenuItem::separator());
            connect_to.push(MenuItem::action(tr!("Other servers ..."), with(|a| a.show_tab("home"))));
            items.push(MenuItem::submenu(tr!("Connect to"), connect_to));
            if ctl.kill_switch_blocking() {
                items.push(MenuItem::action(tr!("Unblock the Internet (Kill Switch)"), with(|a| a.ctl.release_kill_switch())));
            }
        }
        items.push(MenuItem::separator());
        items.push(MenuItem::action(tr!("Open {app}", app = APP_NAME), with(|a| a.show_window())));
        items.push(MenuItem::action(tr!("Settings"), with(|a| a.show_tab("settings"))));
        items.push(MenuItem::separator());
        items.push(MenuItem::action(tr!("Quit {app}", app = APP_NAME), with(|a| a.request_quit())));
        tray.set_menu(items);
    }

    // ================================================================== notifications

    fn notify(&self, tag: &str, title: &str, body: &str) {
        notify(&self.app, &self.settings, tag, title, body);
    }

    fn on_transition(&self, prev: State, new: State) {
        let ctl = &self.ctl;
        let where_ = match ctl.current_location() {
            Some(l) => format!("{}, {}", l.city, l.country),
            None => ctl.location_title(&ctl.status.borrow().location),
        };
        match new {
            State::Connected if prev != State::Connected => {
                self.notify("state", &tr!("Connected"), &tr!("VPN is on: {location}", location = where_));
            }
            State::Reconnecting => self.notify("state", &tr!("Network is lost"), &tr!("Waiting for recovery")),
            State::Disconnected if matches!(prev, State::Connected | State::Reconnecting) => {
                if ctl.user_disconnected.get() {
                    self.notify("state", &tr!("Disconnected"), &tr!("Your traffic is not protected"));
                } else if ctl.kill_switch_blocking() {
                    self.notify("state", &tr!("VPN connection lost"), &tr!("Kill Switch is blocking the Internet until you reconnect"));
                } else {
                    self.notify("state", &tr!("VPN connection lost"), &tr!("Your traffic is not protected"));
                }
            }
            _ => {}
        }
        if self.first_status.get() && matches!(new, State::Connected | State::Disconnected | State::Reconnecting) {
            self.first_status.set(false);
            if new == State::Disconnected
                && self.settings.bool("auto_connect")
                && ctl.logged_in.get() == Some(true)
                && !ctl.kill_switch_blocking()
            {
                ctl.connect_vpn(None, false);
            }
        }
    }

    // ================================================================== updates

    fn auto_update_check(self: &Rc<Self>) {
        if !self.settings.bool("auto_update_cli") || !self.ctl.cli.available() {
            return;
        }
        let channel = self.ctl.config.borrow().update_channel.clone();
        let channel = if channel.is_empty() { "release".to_string() } else { channel };
        let cli = self.ctl.cli.clone();
        let connected = self.ctl.is_connected();
        run_in_thread(
            move || -> Result<Option<(updater::Release, bool)>, String> {
                let current = cli.version();
                let Some(rel) = updater::latest_release(&channel)? else { return Ok(None) };
                if current.is_empty() || version_tuple(&rel.version) <= version_tuple(&current) {
                    return Ok(None);
                }
                if connected {
                    return Ok(Some((rel, false)));
                }
                updater::install_release(&rel)?;
                Ok(Some((rel, true)))
            },
            |result| {
                let Some(this) = instance() else { return };
                match result {
                    Err(err) => log::warn!("CLI update check failed: {err}"),
                    Ok(Some((rel, true))) => {
                        log::info!("CLI updated to {}", rel.version);
                        this.ctl.reload_cli(None);
                    }
                    Ok(Some((rel, false))) => {
                        if let Some(about) = this.window.settings_tab.about() {
                            about.offer_release(rel);
                        }
                    }
                    Ok(None) => {}
                }
            },
        );
        glib::timeout_add_seconds_local_once(6 * 3600, || {
            if let Some(this) = instance() {
                this.auto_update_check();
            }
        });
    }

    /// The passwordless helper keeps its own CLI copy; warn when it falls behind.
    fn check_helper_version(&self) {
        let cli = self.ctl.cli.clone();
        run_in_thread(
            move || cli.available() && cli.privileged.is_installed(true) && !cli.privileged.installed_cli_matches(&cli),
            |outdated| {
                let Some(this) = instance() else { return };
                if outdated {
                    this.window.ui.toast_with(&tr!("Update the passwordless helper in Advanced settings"), &tr!("Open"), || {
                        if let Some(this) = instance() {
                            this.window.show_tab("settings");
                            this.window.settings_tab.show_advanced();
                        }
                    });
                }
            },
        );
    }

    // ================================================================== quitting

    fn on_close_request(&self) -> glib::Propagation {
        if self.quitting.get() {
            return glib::Propagation::Proceed;
        }
        self.window.ui.window.set_visible(false);
        if self.tray.borrow().is_none() {
            self.notify(
                "background",
                APP_NAME,
                &tr!("AdGuard VPN keeps running in the background. Open the app again to show the window."),
            );
        }
        glib::Propagation::Stop
    }

    fn request_quit(self: &Rc<Self>) {
        if !self.ctl.state().is_active() {
            self.really_quit();
            return;
        }
        self.show_window();
        let dialog = adw::AlertDialog::new(Some(&tr!("Quit AdGuard VPN?")), Some(&tr!("Your traffic will no longer be protected.")));
        dialog.add_response("stay", &tr!("Don't quit"));
        dialog.add_response("keep", &tr!("Quit, keep VPN on"));
        dialog.add_response("quit", &tr!("Quit"));
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("stay");
        dialog.set_default_response(Some("stay"));
        dialog.connect_response(None, |_, response| {
            let Some(this) = instance() else { return };
            match response {
                "quit" => this.ctl.disconnect_vpn(Some(Box::new(|_| {
                    if let Some(this) = instance() {
                        this.really_quit();
                    }
                }))),
                "keep" => this.really_quit(),
                _ => {}
            }
        });
        dialog.present(Some(&self.window.ui.window));
    }

    fn really_quit(&self) {
        self.quitting.set(true);
        self.ctl.stop();
        if let Some(tray) = self.tray.borrow_mut().take() {
            tray.hide();
        }
        self.hold.borrow_mut().take();
        self.app.quit();
    }
}
