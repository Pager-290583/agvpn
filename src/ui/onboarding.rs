//! First-run screens: intro carousel, agreements, login and "CLI missing".

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::LazyLock;

use adw::prelude::*;
use gtk::glib;
use regex::Regex;

use crate::cli::{PtyEvent, PtySession};
use crate::consts::{URL_EULA, URL_FORGOT_PASSWORD, URL_PRIVACY};
use crate::parsers::{LOGIN_LINK_RE, LOGIN_SUCCESS_MARKERS};
use crate::tr;
use crate::ui::common::{ask_text, pill_button, AskText, Ui};
use crate::updater;
use crate::util::run_in_thread;

const SLIDES: &[(&str, &str, &str)] = &[
    ("agvpn-shield-symbolic", "Unique VPN protocol",
     "The TrustTunnel protocol works fast and makes your VPN traffic inconspicuous"),
    ("agvpn-exclusions-symbolic", "Websites exclusions",
     "Exclusions help you quickly specify the websites for AdGuard VPN protection"),
    ("agvpn-nolog-symbolic", "No-logging policy",
     "AdGuard VPN makes you anonymous. It never collects or shares your data with third parties"),
];

fn flat_header() -> adw::HeaderBar {
    let header = adw::HeaderBar::builder().show_title(false).build();
    header.add_css_class("flat");
    header
}

// =========================================================================== onboarding

pub struct OnboardingView {
    pub widget: gtk::Box,
}

impl OnboardingView {
    pub fn new(ui: Rc<Ui>, on_done: impl Fn(bool) + 'static) -> Self {
        let widget = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_bottom(24).build();
        widget.append(&flat_header());
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::SlideLeft).vexpand(true).build();
        widget.append(&stack);

        // ---------------------------------------------------------------- carousel
        let intro = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let carousel = adw::Carousel::builder().vexpand(true).allow_scroll_wheel(true).build();
        for (icon, title, body) in SLIDES {
            let page = adw::StatusPage::builder().icon_name(*icon).title(tr!(title)).description(tr!(body)).hexpand(true).build();
            page.add_css_class("onboarding");
            carousel.append(&page);
        }
        intro.append(&carousel);
        intro.append(&adw::CarouselIndicatorDots::builder().carousel(&carousel).build());
        let next = pill_button(&tr!("Next"), &["suggested-action"]);
        next.set_margin_top(6);
        intro.append(&next);
        let skip = gtk::Button::builder().label(tr!("Skip")).halign(gtk::Align::Center).build();
        skip.add_css_class("flat");
        intro.append(&skip);
        stack.add_named(&intro, Some("intro"));
        {
            let (carousel, stack) = (carousel.clone(), stack.clone());
            next.connect_clicked(move |_| {
                let pos = carousel.position().round() as u32;
                if pos + 1 < carousel.n_pages() {
                    carousel.scroll_to(&carousel.nth_page(pos + 1), true);
                } else {
                    stack.set_visible_child_name("agreements");
                }
            });
        }
        {
            let stack = stack.clone();
            skip.connect_clicked(move |_| stack.set_visible_child_name("agreements"));
        }

        // ---------------------------------------------------------------- agreements
        let agree = adw::StatusPage::builder().icon_name("io.github.agvpn.AGVPN").title(tr!("Welcome on board")).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let clamp = adw::Clamp::builder().maximum_size(420).child(&content).build();
        let eula = gtk::CheckButton::new();
        let eula_label = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .xalign(0.0)
            .label(
                tr!("I accept AdGuard VPN’s <a href=\"{eula}\">EULA</a> and <a href=\"{privacy}\">Privacy policy</a>")
                    .replace("{eula}", URL_EULA)
                    .replace("{privacy}", URL_PRIVACY),
            )
            .build();
        {
            let ui = ui.clone();
            eula_label.connect_activate_link(move |_, uri| {
                ui.open_uri(uri);
                glib::Propagation::Stop
            });
        }
        eula.set_child(Some(&eula_label));
        let telemetry = gtk::CheckButton::new();
        telemetry.set_child(Some(
            &gtk::Label::builder().wrap(true).xalign(0.0).label(tr!("Send anonymized app usage data and crash reports to AdGuard")).build(),
        ));
        content.append(&eula);
        content.append(&telemetry);
        let continue_btn = pill_button(&tr!("Continue"), &["suggested-action"]);
        continue_btn.set_sensitive(false);
        continue_btn.set_margin_top(12);
        {
            let continue_btn = continue_btn.clone();
            eula.connect_toggled(move |btn| continue_btn.set_sensitive(btn.is_active()));
        }
        continue_btn.connect_clicked(move |_| on_done(telemetry.is_active()));
        content.append(&continue_btn);
        agree.set_child(Some(&clamp));
        stack.add_named(&agree, Some("agreements"));

        Self { widget }
    }
}

// =========================================================================== login

/// Login through the browser (the CLI's web flow) or with email and password.
pub struct LoginView {
    pub widget: gtk::Box,
    ui: Rc<Ui>,
    on_logged_in: Box<dyn Fn()>,
    stack: gtk::Stack,
    email: adw::EntryRow,
    password: adw::PasswordEntryRow,
    error: gtk::Label,
    session: RefCell<Option<Rc<PtySession>>>,
    buffer: RefCell<String>,
    link: RefCell<String>,
    prompted: Cell<bool>,
    pending_password: RefCell<Option<String>>,
    cancelled: Cell<bool>,
}

impl LoginView {
    pub fn new(ui: Rc<Ui>, on_logged_in: impl Fn() + 'static) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&flat_header());
        let stack = gtk::Stack::builder().vexpand(true).transition_type(gtk::StackTransitionType::Crossfade).build();
        widget.append(&stack);

        // ---------------------------------------------------------------- start
        let start = adw::StatusPage::builder()
            .icon_name("io.github.agvpn.AGVPN")
            .title(tr!("Log in or create an account"))
            .description(tr!("Sign in with your AdGuard account in the browser. You can also continue with Google, Apple or Facebook."))
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let clamp = adw::Clamp::builder().maximum_size(380).child(&content).build();
        let browser = pill_button(&tr!("Continue in browser"), &["suggested-action"]);
        content.append(&browser);

        let expander = gtk::Expander::builder().label(tr!("Log in with email and password")).margin_top(12).build();
        let form = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(12).build();
        let rows = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        rows.add_css_class("boxed-list");
        let email = adw::EntryRow::builder().title(tr!("Email")).build();
        let password = adw::PasswordEntryRow::builder().title(tr!("Password")).build();
        rows.append(&email);
        rows.append(&password);
        form.append(&rows);
        let login = pill_button(&tr!("Sign in"), &[]);
        form.append(&login);
        form.append(&gtk::LinkButton::with_label(URL_FORGOT_PASSWORD, &tr!("Forgot password?")));
        expander.set_child(Some(&form));
        content.append(&expander);
        let error = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).visible(false).build();
        error.add_css_class("error");
        content.append(&error);
        start.set_child(Some(&clamp));
        stack.add_named(&start, Some("start"));

        // ---------------------------------------------------------------- waiting
        let waiting = adw::StatusPage::builder()
            .title(tr!("Signing in"))
            .description(tr!("Complete the sign-in in your browser. This window will update automatically."))
            .build();
        let wbox = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).halign(gtk::Align::Center).build();
        wbox.append(&gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build());
        let again = gtk::Button::builder().label(tr!("Open the link again")).build();
        again.add_css_class("pill");
        wbox.append(&again);
        let copy = gtk::Button::builder().label(tr!("Copy link")).build();
        copy.add_css_class("flat");
        wbox.append(&copy);
        let cancel = gtk::Button::builder().label(tr!("Cancel")).build();
        cancel.add_css_class("flat");
        wbox.append(&cancel);
        waiting.set_child(Some(&wbox));
        stack.add_named(&waiting, Some("waiting"));

        let view = Rc::new(Self {
            widget,
            ui: ui.clone(),
            on_logged_in: Box::new(on_logged_in),
            stack,
            email,
            password,
            error,
            session: RefCell::new(None),
            buffer: RefCell::new(String::new()),
            link: RefCell::new(String::new()),
            prompted: Cell::new(false),
            pending_password: RefCell::new(None),
            cancelled: Cell::new(false),
        });

        let weak = Rc::downgrade(&view);
        let with = move |f: fn(&Rc<LoginView>)| {
            let weak = weak.clone();
            move || {
                if let Some(view) = weak.upgrade() {
                    f(&view);
                }
            }
        };
        let start_web = with(|v| v.start_web());
        browser.connect_clicked(move |_| start_web());
        let start_password = with(|v| v.start_password());
        login.connect_clicked(move |_| start_password());
        let start_password = with(|v| v.start_password());
        view.password.connect_entry_activated(move |_| start_password());
        let open_again = with(|v| {
            let link = v.link.borrow().clone();
            if !link.is_empty() {
                v.ui.open_uri(&link);
            }
        });
        again.connect_clicked(move |_| open_again());
        let copy_link = with(|v| {
            let link = v.link.borrow().clone();
            if !link.is_empty() {
                v.ui.copy_to_clipboard(&link);
            }
        });
        copy.connect_clicked(move |_| copy_link());
        let cancel_cb = with(|v| v.cancel());
        cancel.connect_clicked(move |_| cancel_cb());
        // When the user comes back from the browser, ask the CLI to check right away.
        let check_now = with(|v| {
            if v.ui.window.is_active() && !v.link.borrow().is_empty() {
                if let Some(session) = v.session.borrow().as_ref() {
                    session.write("s");
                }
            }
        });
        ui.window.connect_is_active_notify(move |_| check_now());
        view
    }

    pub fn reset(&self) {
        self.stack.set_visible_child_name("start");
        self.error.set_visible(false);
    }

    // ------------------------------------------------------------------ flows

    fn start_web(self: &Rc<Self>) {
        *self.pending_password.borrow_mut() = None;
        self.start(Vec::new());
    }

    fn start_password(self: &Rc<Self>) {
        static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap());
        let email = self.email.text().trim().to_string();
        let password = self.password.text().to_string();
        if !EMAIL_RE.is_match(&email) {
            self.show_error(&tr!("Email is not valid"));
            return;
        }
        if password.chars().count() < 8 {
            self.show_error(&tr!("Password is too short. It must contain at least 8 characters."));
            return;
        }
        // The password is typed into the CLI's prompt through the pty, never passed in argv.
        *self.pending_password.borrow_mut() = Some(password);
        self.start(vec!["-u".into(), email]);
    }

    fn start(self: &Rc<Self>, args: Vec<String>) {
        if self.session.borrow().is_some() {
            return;
        }
        self.buffer.borrow_mut().clear();
        self.link.borrow_mut().clear();
        self.prompted.set(false);
        self.cancelled.set(false);
        self.error.set_visible(false);
        self.stack.set_visible_child_name("waiting");
        let session = match self.ui.ctl.cli.login_session(&args) {
            Ok(session) => Rc::new(session),
            Err(err) => {
                self.stack.set_visible_child_name("start");
                self.show_error(&err.to_string());
                return;
            }
        };
        let events = session.events.clone();
        *self.session.borrow_mut() = Some(session);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(view) = weak.upgrade() else { break };
                match event {
                    PtyEvent::Output(text) => view.on_output(&text),
                    PtyEvent::Exit(code) => {
                        view.on_exit(code);
                        break;
                    }
                }
            }
        });
    }

    fn cancel(&self) {
        self.cancelled.set(true);
        if let Some(session) = self.session.borrow().as_ref() {
            session.cancel();
        }
        self.stack.set_visible_child_name("start");
    }

    fn write(&self, text: &str) {
        if let Some(session) = self.session.borrow().as_ref() {
            session.write(text);
        }
    }

    fn on_output(self: &Rc<Self>, text: &str) {
        self.buffer.borrow_mut().push_str(text);
        let buffer = self.buffer.borrow().clone();
        let low = buffer.to_lowercase();
        if self.link.borrow().is_empty() {
            if let Some(m) = LOGIN_LINK_RE.find(&buffer) {
                let link = m.as_str().trim_end_matches(['.', ',', ')']).to_string();
                *self.link.borrow_mut() = link.clone();
                self.ui.open_uri(&link);
            }
        }
        if LOGIN_SUCCESS_MARKERS.iter().any(|m| low.contains(m)) {
            return;
        }
        // Interactive prompt (password, 2FA code): the last line ends with ':' and waits for input.
        let last = if buffer.trim().is_empty() {
            String::new()
        } else {
            buffer.trim_end_matches(' ').lines().last().unwrap_or("").to_string()
        };
        let is_prompt = last.trim_end().ends_with(':');
        let last_low = last.to_lowercase();
        if is_prompt && last_low.contains("password") {
            let pending = self.pending_password.borrow_mut().take();
            if let Some(password) = pending {
                self.buffer.borrow_mut().clear();
                self.write(&format!("{password}\n"));
                return;
            }
        }
        if is_prompt && !self.prompted.get() && last_low.contains("code") {
            self.prompted.set(true);
            self.ask_code(last.trim());
        }
    }

    fn ask_code(self: &Rc<Self>, prompt: &str) {
        let label = prompt.trim_end_matches(':').to_string();
        let label = if label.is_empty() { tr!("Code") } else { label };
        let weak = Rc::downgrade(self);
        let confirm = tr!("Confirm");
        let opts = AskText {
            heading: &tr!("Confirmation"),
            body: &tr!("Check your two-factor authentication app"),
            label: &label,
            password: true,
            ok_label: Some(&confirm),
            ..Default::default()
        };
        ask_text(&self.ui.window, opts, move |value| {
            let Some(view) = weak.upgrade() else { return };
            match value {
                None => view.cancel(),
                Some(code) => {
                    view.prompted.set(false);
                    view.buffer.borrow_mut().clear();
                    view.write(&format!("{code}\n"));
                }
            }
        });
    }

    fn on_exit(&self, code: i32) {
        self.session.borrow_mut().take();
        let buffer = self.buffer.borrow().clone();
        let low = buffer.to_lowercase();
        if code == 0 || LOGIN_SUCCESS_MARKERS.iter().any(|m| low.contains(m)) {
            self.password.set_text("");
            (self.on_logged_in)();
            return;
        }
        self.stack.set_visible_child_name("start");
        if self.cancelled.get() {
            return;
        }
        let lines: Vec<&str> = buffer
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("b -") && !l.starts_with("s -") && !l.starts_with("x -"))
            .collect();
        let message = if low.contains("incorrect password") || low.contains("unrecognized") {
            tr!("Unrecognized username or password")
        } else if low.contains("2fa") || low.contains("code is invalid") {
            tr!("Invalid code")
        } else if low.contains("too many requests") {
            tr!("We received too many requests from your IP address. Please try to log in again in a few minutes")
        } else {
            lines.last().map(|s| s.to_string()).unwrap_or_else(|| tr!("Something went wrong"))
        };
        self.show_error(&message);
    }

    fn show_error(&self, message: &str) {
        self.error.set_label(message);
        self.error.set_visible(true);
    }
}

// =========================================================================== missing CLI

/// Shown when adguardvpn-cli is not available (e.g. running from source).
pub struct MissingCliView {
    pub widget: gtk::Box,
}

impl MissingCliView {
    pub fn new(ui: Rc<Ui>, on_installed: impl Fn() + 'static) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&flat_header());
        let page = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .vexpand(true)
            .title(tr!("AdGuard VPN CLI is not installed"))
            .description(tr!("This app controls VPN through the official AdGuard VPN CLI. It will be downloaded from AdGuard's GitHub releases and verified by its checksum."))
            .build();
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).halign(gtk::Align::Center).build();
        let button = pill_button(&tr!("Download AdGuard VPN CLI"), &["suggested-action"]);
        content.append(&button);
        let status = gtk::Label::builder().wrap(true).build();
        status.add_css_class("dim-label");
        content.append(&status);
        page.set_child(Some(&content));
        widget.append(&page);

        let on_installed: Rc<dyn Fn()> = Rc::new(on_installed);
        button.connect_clicked(move |button| {
            button.set_sensitive(false);
            status.set_label(&tr!("Downloading…"));
            let no_release = tr!("No release for this CPU architecture");
            let (button, status, ui, on_installed) = (button.clone(), status.clone(), ui.clone(), on_installed.clone());
            run_in_thread(
                move || match updater::latest_release("release")? {
                    Some(rel) => updater::install_release(&rel),
                    None => Err(no_release),
                },
                move |result| {
                    button.set_sensitive(true);
                    match result {
                        Err(err) => status.set_label(&err),
                        Ok(_) => {
                            status.set_label("");
                            let on_installed = on_installed.clone();
                            ui.ctl.reload_cli(Some(Box::new(move || on_installed())));
                        }
                    }
                },
            );
        });
        Self { widget }
    }
}
