//! Native pointer and keyboard context menus, with actions bound to the clicked item.
use super::*;
#[derive(Clone)]
enum Target {
    Note(Document),
    Path(PathBuf, bool),
}
fn bind(widget: &impl IsA<gtk::Widget>, show: impl Fn(&gtk::Widget, f64, f64) + 'static) {
    let show = Rc::new(show);
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let action = show.clone();
    click.connect_pressed(move |g, _, x, y| {
        if let Some(widget) = g.widget() {
            g.set_state(gtk::EventSequenceState::Claimed);
            action(&widget, x, y);
        }
    });
    widget.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = widget.as_ref().downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK))
        {
            if let Some(widget) = weak.upgrade() {
                show(&widget, 12.0, widget.height() as f64);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(keys);
}
impl Ui {
    pub(super) fn tab_context(self: &Rc<Self>, widget: &gtk::Box, doc: Document) {
        let weak = Rc::downgrade(self);
        let doc = Rc::downgrade(&doc);
        bind(widget, move |anchor, x, y| {
            if let (Some(ui), Some(doc)) = (weak.upgrade(), doc.upgrade()) {
                ui.context_popup(anchor, x, y, Target::Note(doc));
            }
        });
    }
    pub(super) fn explorer_context(
        self: &Rc<Self>,
        widget: &impl IsA<gtk::Widget>,
        path: PathBuf,
        directory: bool,
    ) {
        let weak = Rc::downgrade(self);
        bind(widget, move |anchor, x, y| {
            if let Some(ui) = weak.upgrade() {
                let path = if path.as_os_str().is_empty() {
                    ui.root.borrow().clone().unwrap_or_default()
                } else {
                    path.clone()
                };
                ui.context_popup(anchor, x, y, Target::Path(path, directory));
            }
        });
    }
    fn activate_doc(&self, doc: &Document) {
        let right = self.secondary.page_num(&doc.editor.root).is_some();
        self.active_pane.set(right);
        let notebook = if right {
            &self.secondary
        } else {
            &self.notebook
        };
        notebook.set_current_page(notebook.page_num(&doc.editor.root));
    }
    fn copy_path(path: &std::path::Path) {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&path.to_string_lossy());
        }
    }
    fn context_action(self: &Rc<Self>, target: &Target, action: &str) {
        let (path, directory) = match target {
            Target::Note(doc) => {
                if doc.closed.get() {
                    return;
                }
                self.activate_doc(doc);
                (doc.path.clone(), false)
            }
            Target::Path(path, directory) => (path.clone(), *directory),
        };
        match action {
            "save" => {
                if let Target::Note(doc) = target {
                    self.save(doc.clone(), None);
                }
            }
            "rename" => self.move_note(path),
            "duplicate" => self.duplicate_note(path),
            "trash" => self.trash_note(path),
            "copy-path" => Self::copy_path(&path),
            "open" => {
                if vault::is_markdown(&path) {
                    self.open_file(path);
                } else {
                    self.open_attachment(path);
                }
            }
            "new-note" => self.create_in(false, Some(path)),
            "new-folder" => self.create_in(true, Some(path)),
            "refresh" => self.refresh(),
            "connect" => self.choose_vault(),
            "folder" => {
                let folder = if directory {
                    Some(path)
                } else {
                    path.parent().map(PathBuf::from)
                };
                if let Some(folder) = folder {
                    let uri = gio::File::for_path(folder).uri();
                    let weak = Rc::downgrade(self);
                    gio::AppInfo::launch_default_for_uri_async(
                        &uri,
                        gio::AppLaunchContext::NONE,
                        gio::Cancellable::NONE,
                        move |r| {
                            if let (Some(ui), Err(e)) = (weak.upgrade(), r) {
                                ui.message(&format!("Could not open folder: {e}"));
                            }
                        },
                    );
                }
            }
            "split" => {
                if let Target::Note(doc) = target {
                    let right = self.secondary.page_num(&doc.editor.root).is_none();
                    self.secondary.set_visible(true);
                    self.active_pane.set(right);
                    self.move_tab(doc, right);
                    prefs::update(|p| p.split_enabled = true);
                    self.editor_split
                        .set_position((self.editor_split.width() / 2).max(240));
                    self.update_active();
                    self.save_session();
                }
            }
            "close" => {
                if let Target::Note(doc) = target {
                    self.close_tab(doc.clone());
                }
            }
            _ => {}
        }
    }
    fn context_items(&self, target: &Target) -> Vec<(&'static str, &'static str)> {
        match target {
            Target::Note(_) => vec![
                ("Save", "save"),
                ("Rename / move…", "rename"),
                ("Duplicate…", "duplicate"),
                ("Copy path", "copy-path"),
                ("Open containing folder", "folder"),
                ("Move to other pane", "split"),
                ("Close note", "close"),
            ],
            Target::Path(path, true) => {
                let mut items = vec![
                    ("New note here…", "new-note"),
                    ("New folder here…", "new-folder"),
                    ("Open folder", "folder"),
                    ("Copy path", "copy-path"),
                    ("Refresh", "refresh"),
                ];
                if self.root.borrow().as_ref() != Some(path) {
                    items.extend([
                        ("Rename / move…", "rename"),
                        ("Duplicate…", "duplicate"),
                        ("Move to trash…", "trash"),
                    ]);
                } else {
                    items.push(("Connect another vault…", "connect"));
                }
                items
            }
            Target::Path(_, false) => vec![
                ("Open", "open"),
                ("Rename / move…", "rename"),
                ("Duplicate…", "duplicate"),
                ("Copy path", "copy-path"),
                ("Open containing folder", "folder"),
                ("Move to trash…", "trash"),
            ],
        }
    }
    fn context_popup(self: &Rc<Self>, anchor: &gtk::Widget, x: f64, y: f64, target: Target) {
        let menu = gio::Menu::new();
        let actions = gio::SimpleActionGroup::new();
        for (label, name) in self.context_items(&target) {
            menu.append(Some(label), Some(&format!("context.{name}")));
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let target = target.clone();
            action.connect_activate(move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.context_action(&target, name);
                }
            });
            actions.add_action(&action);
        }
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.add_css_class("menu-popover");
        popover.insert_action_group("context", Some(&actions));
        popover.set_parent(anchor);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|p| {
            let weak = p.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(p) = weak.upgrade() {
                    p.unparent();
                }
            });
        });
        popover.popup();
    }
    pub(super) fn editor_context(self: &Rc<Self>, doc: &Document) {
        let menu = gio::Menu::new();
        let actions = gio::SimpleActionGroup::new();
        for (label, name) in [
            ("Save note", "save"),
            ("Rename / move note…", "rename"),
            ("Copy note path", "copy-path"),
        ] {
            menu.append(Some(label), Some(&format!("note.{name}")));
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let doc = Rc::downgrade(doc);
            action.connect_activate(move |_, _| {
                if let (Some(ui), Some(doc)) = (weak.upgrade(), doc.upgrade()) {
                    ui.context_action(&Target::Note(doc), name);
                }
            });
            actions.add_action(&action);
        }
        doc.editor.view.insert_action_group("note", Some(&actions));
        gtk::prelude::TextViewExt::set_extra_menu(&doc.editor.view, Some(&menu));
    }
}
