//! System tray icon via the StatusNotifierItem + com.canonical.dbusmenu D-Bus protocols.
//!
//! GTK 4 has no tray API; this speaks the protocol directly with Gio, so it works on KDE,
//! Cinnamon, XFCE, Budgie, waybar and GNOME with the AppIndicator extension.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};

const SNI_XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="IconPixmap" type="a(iiay)" access="read"/>
    <property name="AttentionIconName" type="s" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
    <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
    <signal name="NewTitle"/>
    <signal name="NewIcon"/>
    <signal name="NewAttentionIcon"/>
    <signal name="NewOverlayIcon"/>
    <signal name="NewToolTip"/>
    <signal name="NewStatus"><arg name="status" type="s"/></signal>
  </interface>
</node>
"#;

const MENU_XML: &str = r#"
<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" name="parentId" direction="in"/>
      <arg type="i" name="recursionDepth" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="u" name="revision" direction="out"/>
      <arg type="(ia{sv}av)" name="layout" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="as" name="propertyNames" direction="in"/>
      <arg type="a(ia{sv})" name="properties" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="name" direction="in"/>
      <arg type="v" name="value" direction="out"/>
    </method>
    <method name="Event">
      <arg type="i" name="id" direction="in"/>
      <arg type="s" name="eventId" direction="in"/>
      <arg type="v" name="data" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg type="a(isvu)" name="events" direction="in"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg type="i" name="id" direction="in"/>
      <arg type="b" name="needUpdate" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg type="ai" name="ids" direction="in"/>
      <arg type="ai" name="updatesNeeded" direction="out"/>
      <arg type="ai" name="idErrors" direction="out"/>
    </method>
    <signal name="ItemsPropertiesUpdated">
      <arg type="a(ia{sv})" name="updatedProps"/>
      <arg type="a(ias)" name="removedProps"/>
    </signal>
    <signal name="LayoutUpdated">
      <arg type="u" name="revision"/>
      <arg type="i" name="parent"/>
    </signal>
    <signal name="ItemActivationRequested">
      <arg type="i" name="id"/>
      <arg type="u" name="timestamp"/>
    </signal>
  </interface>
</node>
"#;

const SNI_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";

pub type Callback = Rc<dyn Fn()>;

#[derive(Clone, Default)]
pub struct MenuItem {
    pub label: String,
    pub callback: Option<Callback>,
    pub enabled: bool,
    pub separator: bool,
    pub children: Vec<MenuItem>,
    /// None = not a toggle item.
    pub checked: Option<bool>,
}

impl MenuItem {
    pub fn action(label: impl Into<String>, callback: impl Fn() + 'static) -> Self {
        Self { label: label.into(), callback: Some(Rc::new(callback)), enabled: true, ..Default::default() }
    }

    pub fn label(label: impl Into<String>) -> Self {
        Self { label: label.into(), enabled: false, ..Default::default() }
    }

    pub fn separator() -> Self {
        Self { separator: true, enabled: true, ..Default::default() }
    }

    pub fn submenu(label: impl Into<String>, children: Vec<MenuItem>) -> Self {
        Self { label: label.into(), enabled: true, children, ..Default::default() }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
}

#[derive(PartialEq)]
struct Signature(String, bool, bool, Option<bool>, Vec<Signature>);

fn signature(items: &[MenuItem]) -> Vec<Signature> {
    items
        .iter()
        .map(|i| Signature(i.label.clone(), i.enabled, i.separator, i.checked, signature(&i.children)))
        .collect()
}

/// A flattened menu: id -> item (id 0 is the root).
#[derive(Default)]
struct Menu {
    items: HashMap<i32, MenuItem>,
    children: HashMap<i32, Vec<i32>>,
    signature: Vec<Signature>,
}

impl Menu {
    fn build(items: Vec<MenuItem>) -> Self {
        let mut menu = Menu { signature: signature(&items), ..Default::default() };
        let mut counter = 0;
        menu.assign(0, items, &mut counter);
        menu
    }

    fn assign(&mut self, parent: i32, items: Vec<MenuItem>, counter: &mut i32) {
        let mut ids = Vec::new();
        for mut item in items {
            *counter += 1;
            let id = *counter;
            ids.push(id);
            let children = std::mem::take(&mut item.children);
            let has_children = !children.is_empty();
            self.items.insert(id, item);
            if has_children {
                self.assign(id, children, counter);
            }
        }
        self.children.insert(parent, ids);
    }

    fn props(&self, id: i32) -> glib::Variant {
        let dict = glib::VariantDict::new(None);
        if id == 0 {
            dict.insert("children-display", "submenu");
            return dict.end();
        }
        let Some(item) = self.items.get(&id) else { return dict.end() };
        if item.separator {
            dict.insert("type", "separator");
            return dict.end();
        }
        dict.insert("label", item.label.replace('_', "__"));
        dict.insert("enabled", item.enabled);
        dict.insert("visible", true);
        if self.children.get(&id).is_some_and(|c| !c.is_empty()) {
            dict.insert("children-display", "submenu");
        }
        if let Some(checked) = item.checked {
            dict.insert("toggle-type", "checkmark");
            dict.insert("toggle-state", if checked { 1i32 } else { 0i32 });
        }
        dict.end()
    }

    fn layout(&self, id: i32, depth: i32) -> glib::Variant {
        if id != 0 && !self.items.contains_key(&id) {
            let empty = glib::VariantDict::new(None).end();
            let kids = glib::Variant::array_from_iter_with_type(glib::VariantTy::VARIANT, Vec::<glib::Variant>::new());
            return glib::Variant::tuple_from_iter([id.to_variant(), empty, kids]);
        }
        let mut kids = Vec::new();
        if depth != 0 {
            for child in self.children.get(&id).into_iter().flatten() {
                let next = if depth > 0 { depth - 1 } else { -1 };
                kids.push(glib::Variant::from_variant(&self.layout(*child, next)));
            }
        }
        let kids = glib::Variant::array_from_iter_with_type(glib::VariantTy::VARIANT, kids);
        glib::Variant::tuple_from_iter([id.to_variant(), self.props(id), kids])
    }
}

struct Inner {
    app_id: String,
    title: String,
    on_activate: Callback,
    icon_name: RefCell<String>,
    icon_path: RefCell<PathBuf>,
    tooltip: RefCell<(String, String)>,
    menu: RefCell<Menu>,
    revision: Cell<u32>,
    bus: RefCell<Option<gio::DBusConnection>>,
    registrations: RefCell<Vec<gio::RegistrationId>>,
    unwatch: RefCell<Option<Box<dyn FnOnce()>>>,
    visible: Cell<bool>,
}

#[derive(Clone)]
pub struct Tray(Rc<Inner>);

impl Tray {
    pub fn new(app_id: &str, title: &str, on_activate: impl Fn() + 'static) -> Self {
        Tray(Rc::new(Inner {
            app_id: app_id.into(),
            title: title.into(),
            on_activate: Rc::new(on_activate),
            icon_name: RefCell::new(String::new()),
            icon_path: RefCell::new(PathBuf::new()),
            tooltip: RefCell::new((String::new(), String::new())),
            menu: RefCell::new(Menu::default()),
            revision: Cell::new(1),
            bus: RefCell::new(None),
            registrations: RefCell::new(Vec::new()),
            unwatch: RefCell::new(None),
            visible: Cell::new(false),
        }))
    }

    // ------------------------------------------------------------------ public

    pub fn show(&self) {
        let inner = &self.0;
        if inner.visible.get() {
            return;
        }
        inner.visible.set(true);
        let bus = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
            Ok(bus) => bus,
            Err(err) => {
                log::warn!("no session bus: {}", err.message());
                return;
            }
        };
        let sni = gio::DBusNodeInfo::for_xml(SNI_XML).ok().and_then(|n| n.lookup_interface("org.kde.StatusNotifierItem"));
        let menu = gio::DBusNodeInfo::for_xml(MENU_XML).ok().and_then(|n| n.lookup_interface("com.canonical.dbusmenu"));
        let (Some(sni), Some(menu)) = (sni, menu) else { return };

        let weak = Rc::downgrade(inner);
        let weak2 = weak.clone();
        let sni_id = bus
            .register_object(SNI_PATH, &sni)
            .method_call(move |_conn, _sender, _path, _iface, method, _params, invocation| {
                if let Some(inner) = weak.upgrade() {
                    if method == "Activate" || method == "SecondaryActivate" {
                        let cb = inner.on_activate.clone();
                        glib::idle_add_local_once(move || cb());
                    }
                }
                invocation.return_value(None);
            })
            .property(move |_conn, _sender, _path, _iface, prop| match weak2.upgrade() {
                Some(inner) => sni_property(&inner, prop),
                None => "".to_variant(),
            })
            .build();

        let weak = Rc::downgrade(inner);
        let menu_id = bus
            .register_object(MENU_PATH, &menu)
            .method_call(move |_conn, _sender, _path, _iface, method, params, invocation| match weak.upgrade() {
                Some(inner) => menu_call(&inner, method, &params, invocation),
                None => invocation.return_value(None),
            })
            .property(|_conn, _sender, _path, _iface, prop| match prop {
                "Version" => 3u32.to_variant(),
                "TextDirection" => "ltr".to_variant(),
                "Status" => "normal".to_variant(),
                _ => Vec::<String>::new().to_variant(),
            })
            .build();

        for id in [sni_id, menu_id] {
            match id {
                Ok(id) => inner.registrations.borrow_mut().push(id),
                Err(err) => log::warn!("cannot export tray object: {}", err.message()),
            }
        }

        let watch = gio::bus_watch_name_on_connection(
            &bus,
            "org.kde.StatusNotifierWatcher",
            gio::BusNameWatcherFlags::NONE,
            |conn, _name, _owner| {
                conn.call(
                    Some("org.kde.StatusNotifierWatcher"),
                    "/StatusNotifierWatcher",
                    "org.kde.StatusNotifierWatcher",
                    "RegisterStatusNotifierItem",
                    Some(&(SNI_PATH,).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    -1,
                    gio::Cancellable::NONE,
                    |res| {
                        if let Err(err) = res {
                            log::warn!("tray registration failed: {}", err.message());
                        }
                    },
                );
            },
            |_conn, _name| {},
        );
        *inner.unwatch.borrow_mut() = Some(Box::new(move || gio::bus_unwatch_name(watch)));
        *inner.bus.borrow_mut() = Some(bus);
    }

    pub fn hide(&self) {
        let inner = &self.0;
        if !inner.visible.get() {
            return;
        }
        inner.visible.set(false);
        if let Some(unwatch) = inner.unwatch.borrow_mut().take() {
            unwatch();
        }
        if let Some(bus) = inner.bus.borrow().as_ref() {
            for id in inner.registrations.borrow_mut().drain(..) {
                let _ = bus.unregister_object(id);
            }
        }
    }

    pub fn set_icon(&self, icon_name: &str, icon_path: PathBuf) {
        if *self.0.icon_name.borrow() == icon_name {
            return;
        }
        *self.0.icon_name.borrow_mut() = icon_name.into();
        *self.0.icon_path.borrow_mut() = icon_path;
        self.emit_sni("NewIcon");
    }

    pub fn set_tooltip(&self, title: &str, body: &str) {
        let new = (title.to_string(), body.to_string());
        if *self.0.tooltip.borrow() == new {
            return;
        }
        *self.0.tooltip.borrow_mut() = new;
        self.emit_sni("NewToolTip");
    }

    pub fn set_menu(&self, items: Vec<MenuItem>) {
        // Status polling rebuilds the menu every few seconds; only announce real changes so an
        // open menu does not flicker. Unchanged structure keeps the same positional ids.
        let menu = Menu::build(items);
        let changed = menu.signature != self.0.menu.borrow().signature;
        *self.0.menu.borrow_mut() = menu;
        if !changed {
            return;
        }
        let revision = self.0.revision.get() + 1;
        self.0.revision.set(revision);
        if self.0.visible.get() {
            if let Some(bus) = self.0.bus.borrow().as_ref() {
                let _ = bus.emit_signal(None, MENU_PATH, "com.canonical.dbusmenu", "LayoutUpdated",
                                        Some(&(revision, 0i32).to_variant()));
            }
        }
    }

    fn emit_sni(&self, signal: &str) {
        if self.0.visible.get() {
            if let Some(bus) = self.0.bus.borrow().as_ref() {
                let _ = bus.emit_signal(None, SNI_PATH, "org.kde.StatusNotifierItem", signal, None);
            }
        }
    }
}

fn pixmaps(path: &PathBuf) -> Vec<(i32, i32, Vec<u8>)> {
    if !path.exists() {
        return Vec::new();
    }
    let mut result = Vec::new();
    for size in [22, 32, 48] {
        let Ok(pix) = Pixbuf::from_file_at_size(path, size, size) else { continue };
        let pix = if pix.has_alpha() { pix } else { pix.add_alpha(false, 0, 0, 0).unwrap_or(pix) };
        let (width, height, stride) = (pix.width(), pix.height(), pix.rowstride() as usize);
        let data = pix.read_pixel_bytes();
        let mut argb = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            let row = &data[y * stride..y * stride + width as usize * 4];
            for px in row.chunks_exact(4) {
                argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
            }
        }
        result.push((width, height, argb));
    }
    result
}

fn sni_property(inner: &Inner, prop: &str) -> glib::Variant {
    match prop {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => inner.app_id.to_variant(),
        "Title" => inner.title.to_variant(),
        "Status" => "Active".to_variant(),
        "WindowId" => 0i32.to_variant(),
        "IconName" => inner.icon_name.borrow().to_variant(),
        "IconThemePath" | "AttentionIconName" => "".to_variant(),
        "ItemIsMenu" => false.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH.to_string()).unwrap().to_variant(),
        "IconPixmap" => pixmaps(&inner.icon_path.borrow()).to_variant(),
        "ToolTip" => {
            let (title, body) = inner.tooltip.borrow().clone();
            (inner.icon_name.borrow().clone(), Vec::<(i32, i32, Vec<u8>)>::new(), title, body).to_variant()
        }
        _ => "".to_variant(),
    }
}

fn activate(inner: &Inner, id: i32) {
    let menu = inner.menu.borrow();
    if let Some(item) = menu.items.get(&id) {
        if item.enabled {
            if let Some(cb) = item.callback.clone() {
                glib::idle_add_local_once(move || cb());
            }
        }
    }
}

fn menu_call(inner: &Inner, method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
    match method {
        "GetLayout" => {
            let (parent, depth, _names) = params.get::<(i32, i32, Vec<String>)>().unwrap_or((0, -1, Vec::new()));
            let layout = inner.menu.borrow().layout(parent, depth);
            let reply = glib::Variant::tuple_from_iter([inner.revision.get().to_variant(), layout]);
            invocation.return_value(Some(&reply));
        }
        "GetGroupProperties" => {
            let (ids, _names) = params.get::<(Vec<i32>, Vec<String>)>().unwrap_or_default();
            let menu = inner.menu.borrow();
            let ids: Vec<i32> = if ids.is_empty() { menu.items.keys().copied().collect() } else { ids };
            let entries: Vec<glib::Variant> = ids
                .into_iter()
                .filter(|id| menu.items.contains_key(id))
                .map(|id| glib::Variant::tuple_from_iter([id.to_variant(), menu.props(id)]))
                .collect();
            let array = glib::Variant::array_from_iter_with_type(glib::VariantTy::new("(ia{sv})").unwrap(), entries);
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([array])));
        }
        "GetProperty" => {
            let (id, name) = params.get::<(i32, String)>().unwrap_or_default();
            let props = inner.menu.borrow().props(id);
            let value = glib::VariantDict::new(Some(&props)).lookup_value(&name, None).unwrap_or_else(|| "".to_variant());
            invocation.return_value(Some(&glib::Variant::tuple_from_iter([glib::Variant::from_variant(&value)])));
        }
        "Event" => {
            if let Some((id, event, _data, _ts)) = params.get::<(i32, String, glib::Variant, u32)>() {
                if event == "clicked" {
                    activate(inner, id);
                }
            }
            invocation.return_value(None);
        }
        "EventGroup" => {
            if let Some((events,)) = params.get::<(Vec<(i32, String, glib::Variant, u32)>,)>() {
                for (id, event, _data, _ts) in events {
                    if event == "clicked" {
                        activate(inner, id);
                    }
                }
            }
            invocation.return_value(Some(&(Vec::<i32>::new(),).to_variant()));
        }
        "AboutToShow" => invocation.return_value(Some(&(false,).to_variant())),
        "AboutToShowGroup" => invocation.return_value(Some(&(Vec::<i32>::new(), Vec::<i32>::new()).to_variant())),
        _ => invocation.return_value(None),
    }
}
