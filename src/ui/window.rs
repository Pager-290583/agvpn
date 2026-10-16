//! Main window: routes between first-run screens and the tabbed main UI.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::consts::{APP_NAME, URL_UPGRADE};
use crate::controller::Controller;
use crate::tr;
use crate::ui::common::Ui;
use crate::ui::exclusions::ExclusionsPage;
use crate::ui::home::HomePage;
use crate::ui::onboarding::{LoginView, MissingCliView, OnboardingView};
use crate::ui::settings_page::SettingsTab;
use crate::ui::stats::StatsPage;

fn tab_view(title: &str, content: &impl IsA<gtk::Widget>, header_end: Option<&gtk::Widget>) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new(title, "")).build();
    if let Some(widget) = header_end {
        header.pack_end(widget);
    }
    view.add_top_bar(&header);
    view.set_content(Some(content));
    view
}

pub struct MainWindow {
    pub ui: Rc<Ui>,
    root: gtk::Stack,
    stack: adw::ViewStack,
    login: Rc<LoginView>,
    banner_blocked: adw::Banner,
    banner_traffic: adw::Banner,
    banner_socks: adw::Banner,
    _home: Rc<HomePage>,
    _exclusions: Rc<ExclusionsPage>,
    _stats: Rc<StatsPage>,
    pub settings_tab: Rc<SettingsTab>,
}

impl MainWindow {
    pub fn new(app: &adw::Application, ctl: Rc<Controller>) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(440)
            .default_height(760)
            .build();
        window.set_size_request(360, 520);
        let toasts = adw::ToastOverlay::new();
        window.set_content(Some(&toasts));
        let root = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        toasts.set_child(Some(&root));
        let ui = Rc::new(Ui { window: window.clone(), toasts, ctl: ctl.clone() });

        // The route callbacks below need the window that does not exist yet.
        let this_cell: Rc<std::cell::RefCell<std::rc::Weak<MainWindow>>> = Default::default();
        let route_later = {
            let cell = this_cell.clone();
            move || {
                if let Some(this) = cell.borrow().upgrade() {
                    this.route();
                }
            }
        };

        let missing = MissingCliView::new(ui.clone(), route_later.clone());
        root.add_named(&missing.widget, Some("missing"));
        let onboarding = {
            let (cell, ctl) = (this_cell.clone(), ctl.clone());
            OnboardingView::new(ui.clone(), move |telemetry| {
                ctl.settings.set("onboarding_done", true);
                ctl.settings.set("agreements_accepted", true);
                ctl.apply_initial_config(telemetry);
                if let Some(this) = cell.borrow().upgrade() {
                    this.route();
                }
            })
        };
        root.add_named(&onboarding.widget, Some("onboarding"));
        let login = {
            let (cell, ctl) = (this_cell.clone(), ctl.clone());
            LoginView::new(ui.clone(), move || {
                ctl.on_logged_in();
                if let Some(this) = cell.borrow().upgrade() {
                    this.route();
                    this.ui.toast(&tr!("Successfully logged in"));
                }
            })
        };
        root.add_named(&login.widget, Some("login"));
        let loading = adw::StatusPage::builder().title(tr!("Loading…")).icon_name("io.github.agvpn.AGVPN").build();
        root.add_named(&loading, Some("loading"));

        // ---------------------------------------------------------------- main tabs
        let stack = adw::ViewStack::builder().vexpand(true).build();
        let home = HomePage::new(ui.clone());
        let home_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let banner_blocked = adw::Banner::new(&tr!("Kill Switch is blocking the Internet"));
        banner_blocked.set_button_label(Some(&tr!("Unblock")));
        let banner_traffic = adw::Banner::new(&tr!("Speed is reduced: you’ve reached the free data limit"));
        banner_traffic.set_button_label(Some(&tr!("Upgrade")));
        let banner_socks = adw::Banner::new(&tr!("SOCKS5 mode: only apps configured to use the proxy are protected"));
        {
            let ctl = ctl.clone();
            banner_blocked.connect_button_clicked(move |_| ctl.release_kill_switch());
        }
        {
            let ui = ui.clone();
            banner_traffic.connect_button_clicked(move |_| ui.open_uri(URL_UPGRADE));
        }
        for banner in [&banner_blocked, &banner_traffic, &banner_socks] {
            home_box.append(banner);
        }
        home_box.append(&home.widget);

        let menu = gio::Menu::new();
        menu.append(Some(&tr!("Update locations")), Some("win.refresh-locations"));
        menu.append(Some(&tr!("Settings")), Some("win.tab-settings"));
        let section = gio::Menu::new();
        section.append(Some(&tr!("Quit")), Some("app.quit"));
        menu.append_section(None, &section);
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text(tr!("Main menu"))
            .build();

        stack.add_titled_with_icon(&tab_view(APP_NAME, &home_box, Some(menu_btn.upcast_ref())), Some("home"), &tr!("Home"), "agvpn-home-symbolic");
        let exclusions = ExclusionsPage::new(ui.clone());
        stack.add_titled_with_icon(&tab_view(&tr!("Exclusions"), &exclusions.widget, None), Some("exclusions"), &tr!("Exclusions"), "agvpn-exclusions-symbolic");
        let stats = StatsPage::new(ui.clone());
        stack.add_titled_with_icon(&tab_view(&tr!("Stats"), &stats.widget, None), Some("stats"), &tr!("Stats"), "agvpn-stats-symbolic");
        let settings_tab = SettingsTab::new(ui.clone());
        stack.add_titled_with_icon(&settings_tab.widget, Some("settings"), &tr!("Settings"), "agvpn-settings-symbolic");

        let main = adw::ToolbarView::new();
        main.set_content(Some(&stack));
        main.add_bottom_bar(&adw::ViewSwitcherBar::builder().stack(&stack).reveal(true).build());
        root.add_named(&main, Some("main"));

        let this = Rc::new(Self {
            ui: ui.clone(),
            root,
            stack,
            login,
            banner_blocked,
            banner_traffic,
            banner_socks,
            _home: home,
            _exclusions: exclusions,
            _stats: stats,
            settings_tab,
        });
        *this_cell.borrow_mut() = Rc::downgrade(&this);
        this.setup_actions(app);
        this.connect_signals();
        this.route();
        this
    }

    fn setup_actions(self: &Rc<Self>, app: &adw::Application) {
        let action = gio::SimpleAction::new("refresh-locations", None);
        let ctl = self.ui.ctl.clone();
        action.connect_activate(move |_, _| ctl.refresh_locations());
        self.ui.window.add_action(&action);
        for (idx, name) in ["home", "exclusions", "stats", "settings"].into_iter().enumerate() {
            let action = gio::SimpleAction::new(&format!("tab-{name}"), None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.show_tab(name);
                }
            });
            self.ui.window.add_action(&action);
            app.set_accels_for_action(&format!("win.tab-{name}"), &[&format!("<Alt>{}", idx + 1)]);
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let signals = &self.ui.ctl.signals;
        let weak = Rc::downgrade(self);
        let on = move |f: fn(&Rc<MainWindow>)| {
            let weak = weak.clone();
            move |_: &()| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            }
        };
        signals.status_changed.connect(on(|w| w.update_banners()));
        signals.license_changed.connect(on(|w| {
            w.update_banners();
            w.route();
        }));
        signals.config_changed.connect(on(|w| w.update_banners()));
        signals.cli_changed.connect(on(|w| w.route()));
        signals.needs_reconnect.connect(on(|w| {
            let ctl = w.ui.ctl.clone();
            w.ui.toast_with(&tr!("To apply the new settings, reconnect to VPN"), &tr!("Reconnect"), move || ctl.reconnect());
        }));
        let ui = self.ui.clone();
        signals.error.connect(move |(title, detail)| ui.show_error(title, detail));
        let ui = self.ui.clone();
        signals.toast.connect(move |message| ui.toast(message));

        // Password prompts for `sudo -A` (when no polkit agent is available).
        let (tx, rx) = async_channel::unbounded();
        self.ui.ctl.cli.privileged.set_password_channel(tx);
        let ui = self.ui.clone();
        glib::spawn_future_local(async move {
            while let Ok((prompt, reply)) = rx.recv().await {
                ask_password(&ui, &prompt, reply);
            }
        });
    }

    // ================================================================== routing

    pub fn route(&self) {
        let ctl = &self.ui.ctl;
        let settings = &ctl.settings;
        let target = if !ctl.cli.available() {
            "missing"
        } else if !settings.bool("onboarding_done") || !settings.bool("agreements_accepted") {
            "onboarding"
        } else {
            match ctl.logged_in.get() {
                None => "loading",
                Some(false) => "login",
                Some(true) => "main",
            }
        };
        if self.root.visible_child_name().as_deref() != Some(target) {
            if target == "login" {
                self.login.reset();
            }
            self.root.set_visible_child_name(target);
        }
    }

    pub fn show_tab(&self, name: &str) {
        if self.root.visible_child_name().as_deref() == Some("main") {
            self.stack.set_visible_child_name(name);
        }
    }

    pub fn show(&self) {
        self.ui.window.set_visible(true);
        self.ui.window.present();
    }

    fn update_banners(&self) {
        let ctl = &self.ui.ctl;
        self.banner_blocked.set_revealed(ctl.kill_switch_blocking());
        self.banner_traffic.set_revealed(ctl.license.borrow().traffic_exhausted);
        self.banner_socks.set_revealed(ctl.config.borrow().mode == "socks");
    }
}

/// Password prompt for `sudo -A` when no polkit agent is available.
fn ask_password(ui: &Rc<Ui>, prompt: &str, reply: async_channel::Sender<Option<String>>) {
    ui.window.present();
    let body = format!("{}\n{prompt}", tr!("AdGuard VPN needs administrator rights to change network settings."));
    let dialog = adw::AlertDialog::new(Some(&tr!("Authentication required")), Some(&body));
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    let entry = adw::PasswordEntryRow::builder().title(tr!("Password")).build();
    list.append(&entry);
    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", &tr!("Cancel"));
    dialog.add_response("ok", &tr!("Authenticate"));
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    let answered = Rc::new(Cell::new(false));
    {
        let (answered, reply, dialog) = (answered.clone(), reply.clone(), dialog.clone());
        entry.connect_entry_activated(move |entry| {
            if !answered.replace(true) {
                let _ = reply.try_send(Some(entry.text().to_string()));
            }
            dialog.close();
        });
    }
    {
        let entry = entry.clone();
        dialog.connect_response(None, move |_, response| {
            if !answered.replace(true) {
                let _ = reply.try_send(if response == "ok" { Some(entry.text().to_string()) } else { None });
            }
        });
    }
    dialog.present(Some(&ui.window));
    entry.grab_focus();
}
