//! Native metadata collections; all note edits go through the vault's conditional save path.
use super::*;
use vault::knowledge::{self, Collection, Note};
struct CollectionView {
    ui: std::rc::Weak<Ui>,
    root: PathBuf,
    db: PathBuf,
    window: gtk::Window,
    body: gtk::Box,
    query: gtk::Entry,
    layout: gtk::DropDown,
    sort: gtk::Entry,
    group: gtk::Entry,
    columns: gtk::Entry,
    descending: gtk::CheckButton,
    status: gtk::Label,
    revision: Cell<u64>,
    data: RefCell<Vec<Note>>,
}
impl CollectionView {
    fn config(&self) -> Collection {
        Collection {
            name: String::new(),
            query: self.query.text().to_string(),
            layout: ["table", "list", "kanban", "gallery"][self.layout.selected().min(3) as usize]
                .into(),
            sort: self.sort.text().to_string(),
            group: self.group.text().to_string(),
            columns: self.columns.text().to_string(),
            descending: self.descending.is_active(),
        }
    }
    fn refresh(self: &Rc<Self>) {
        let sequence = self.revision.get() + 1;
        self.revision.set(sequence);
        let db = self.db.clone();
        let root = self.root.clone();
        let config = self.config();
        let weak = Rc::downgrade(self);
        self.status.set_text("Loading collection…");
        worker::background(
            move || knowledge::collection(&db, &root, &config),
            move |result| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                if view.revision.get() != sequence {
                    return;
                }
                match result {
                    Ok(notes) => {
                        view.status.set_text(&format!(
                            "{} notes · metadata changes update the original Markdown",
                            notes.len()
                        ));
                        *view.data.borrow_mut() = notes;
                        view.render();
                    }
                    Err(e) => view.status.set_text(&format!("Collection: {e:#}")),
                }
            },
        );
    }
    fn render(self: &Rc<Self>) {
        clear(&self.body);
        let config = self.config();
        match config.layout.as_str() {
            "table" => self.table(&config),
            "kanban" => self.kanban(&config),
            "gallery" => self.cards(true, None),
            _ => self.cards(false, None),
        }
    }
    fn table(self: &Rc<Self>, config: &Collection) {
        let count = self.data.borrow().len();
        let values = (0..count).map(|i| i.to_string()).collect::<Vec<_>>();
        let model = gtk::StringList::new(&values.iter().map(String::as_str).collect::<Vec<_>>());
        let selection = gtk::NoSelection::new(Some(model));
        let table = gtk::ColumnView::new(Some(selection));
        table.set_hexpand(true);
        table.set_vexpand(true);
        table.set_show_column_separators(true);
        table.set_show_row_separators(true);
        let columns = std::iter::once("title")
            .chain(
                config
                    .columns
                    .split(',')
                    .map(str::trim)
                    .filter(|k| !k.is_empty()),
            )
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for key in columns {
            let factory = gtk::SignalListItemFactory::new();
            let weak = Rc::downgrade(self);
            let field = key.clone();
            factory.connect_bind(move |_, item| {
                let (Some(view), Some(item)) =
                    (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
                else {
                    return;
                };
                let Some(index) = item
                    .item()
                    .and_downcast::<gtk::StringObject>()
                    .and_then(|s| s.string().parse::<usize>().ok())
                else {
                    return;
                };
                let Some(note) = view.data.borrow().get(index).cloned() else {
                    return;
                };
                let text = match field.as_str() {
                    "title" => note.title.clone(),
                    "path" => note
                        .path
                        .strip_prefix(&view.root)
                        .unwrap_or(&note.path)
                        .to_string_lossy()
                        .to_string(),
                    "tags" => note.tags.join(", "),
                    _ => note.properties.get(&field).cloned().unwrap_or_default(),
                };
                let button = gtk::Button::with_label(&text);
                button.set_has_frame(false);
                button.set_hexpand(true);
                button.set_tooltip_text(Some(&format!("{} · {}", note.path.display(), field)));
                let weak = view.ui.clone();
                let key = field.clone();
                button.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        if key == "title" || key == "path" {
                            ui.open_file(note.path.clone());
                        } else {
                            ui.collection_property(note.path.clone(), key.clone());
                        }
                    }
                });
                item.set_child(Some(&button));
            });
            let column = gtk::ColumnViewColumn::new(Some(&key), Some(factory));
            column.set_resizable(true);
            column.set_expand(key == "title");
            table.append_column(&column);
        }
        self.body.append(&scroll(&table));
    }
    fn cards(self: &Rc<Self>, gallery: bool, indices: Option<Vec<usize>>) {
        let indices = indices.unwrap_or_else(|| (0..self.data.borrow().len()).collect());
        let view = self.card_widget(gallery, indices);
        self.body.append(&view);
    }
    fn card_widget(self: &Rc<Self>, gallery: bool, indices: Vec<usize>) -> gtk::Widget {
        let values = indices.iter().map(usize::to_string).collect::<Vec<_>>();
        let model = gtk::StringList::new(&values.iter().map(String::as_str).collect::<Vec<_>>());
        let selection = gtk::NoSelection::new(Some(model));
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_bind(move |_, item| {
            let (Some(view), Some(item)) = (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let Some(index) = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .and_then(|s| s.string().parse::<usize>().ok())
            else {
                return;
            };
            let Some(note) = view.data.borrow().get(index).cloned() else {
                return;
            };
            let content = widgets::vbox(6);
            content.set_margin_top(8);
            content.set_margin_bottom(8);
            content.set_margin_start(8);
            content.set_margin_end(8);
            if gallery {
                content.set_size_request(220, 170);
                let title = widgets::label(&note.title, "");
                title.set_wrap(true);
                title.set_xalign(0.0);
                content.append(&title);
                if let Some(cover) = note.properties.get("cover").filter(|s| !s.is_empty()) {
                    let picture = gtk::Picture::new();
                    picture.set_size_request(180, 110);
                    picture.set_content_fit(gtk::ContentFit::Contain);
                    content.append(&picture);
                    let target = cover.trim_matches(['!', '[', ']']).to_owned();
                    let root = view.root.clone();
                    let db = view.db.clone();
                    let source = note.path.clone();
                    let weak = picture.downgrade();
                    worker::background(
                        move || vault::workspace::asset(&root, &db, &source, &target, true),
                        move |result| {
                            if let Some(picture) = weak.upgrade()
                                && let Ok(vault::workspace::AssetData::Image { bytes, .. }) = result
                            {
                                let stream = gio::MemoryInputStream::from_bytes(
                                    &glib::Bytes::from_owned(bytes),
                                );
                                if let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_stream(
                                    &stream,
                                    gio::Cancellable::NONE,
                                ) {
                                    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
                                }
                            }
                        },
                    );
                }
            } else {
                let title = widgets::label(&note.title, "");
                title.set_xalign(0.0);
                content.append(&title);
            }
            let info = view
                .config()
                .columns
                .split(',')
                .map(str::trim)
                .filter_map(|key| note.properties.get(key).map(|v| format!("{key}: {v}")))
                .collect::<Vec<_>>()
                .join(" · ");
            let label = widgets::label(&info, "dim-label");
            label.set_wrap(true);
            label.set_xalign(0.0);
            content.append(&label);
            let button = gtk::Button::new();
            button.set_has_frame(false);
            button.set_child(Some(&content));
            let weak = view.ui.clone();
            let path = note.path.clone();
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.open_file(path.clone());
                }
            });
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
            let path = note.path.to_string_lossy().to_string();
            drag.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&path.to_value()))
            });
            button.add_controller(drag);
            let actions = widgets::hbox(4);
            let row = widgets::vbox(0);
            row.append(&button);
            let edit = gtk::Button::with_label("Properties…");
            let weak = view.ui.clone();
            let group = view.group.text().to_string();
            edit.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.collection_property(note.path.clone(), group.clone());
                }
            });
            actions.append(&edit);
            row.append(&actions);
            item.set_child(Some(&row));
        });
        let widget: gtk::Widget = if gallery {
            let view = gtk::GridView::new(Some(selection), Some(factory));
            view.set_min_columns(1);
            view.set_max_columns(6);
            view.upcast()
        } else {
            gtk::ListView::new(Some(selection), Some(factory)).upcast()
        };
        let scroll = scroll(&widget);
        scroll.set_vexpand(true);
        scroll.set_hexpand(true);
        scroll.upcast()
    }
    fn kanban(self: &Rc<Self>, config: &Collection) {
        let mut groups = std::collections::BTreeMap::<String, Vec<usize>>::new();
        for (i, note) in self.data.borrow().iter().enumerate() {
            groups
                .entry(
                    note.properties
                        .get(&config.group)
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| "Unassigned".into()),
                )
                .or_default()
                .push(i);
        }
        let board = widgets::hbox(8);
        for (name, indices) in groups {
            let column = widgets::vbox(4);
            column.set_width_request(260);
            column.set_hexpand(true);
            column.append(&widgets::label(
                &format!("{name} · {}", indices.len()),
                "group-heading",
            ));
            column.append(&self.card_widget(false, indices));
            let drop = gtk::DropTarget::new(
                String::static_type(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
            );
            let weak = Rc::downgrade(self);
            let key = config.group.clone();
            drop.connect_drop(move |_, value, _, _| {
                let (Some(view), Ok(path)) = (weak.upgrade(), value.get::<String>()) else {
                    return false;
                };
                if let Some(ui) = view.ui.upgrade() {
                    let value = if name == "Unassigned" {
                        None
                    } else {
                        Some(serde_json::to_string(&name).unwrap_or_default())
                    };
                    let weak = Rc::downgrade(&view);
                    ui.collection_update(
                        PathBuf::from(path),
                        key.clone(),
                        value,
                        None,
                        move || {
                            if let Some(v) = weak.upgrade() {
                                v.refresh();
                            }
                        },
                    );
                    true
                } else {
                    false
                }
            });
            column.add_controller(drop);
            board.append(&column);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&board)
            .vexpand(true)
            .build();
        self.body.append(&scroller);
    }
}
impl Ui {
    pub(super) fn collections(self: &Rc<Self>) {
        let (Some(root_path), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone())
        else {
            self.message("Open a vault first");
            return;
        };
        let (window, root, toolbar) = self.workspace_window("Collections");
        let saved = prefs::get().collections;
        let names = std::iter::once("New view".to_owned())
            .chain(saved.iter().map(|v| v.name.clone()))
            .collect::<Vec<_>>();
        let chooser =
            gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let save = gtk::Button::with_label("Save view…");
        let remove = gtk::Button::with_label("Remove view");
        let refresh = gtk::Button::with_label("Refresh");
        toolbar.append(&chooser);
        toolbar.append(&save);
        toolbar.append(&remove);
        toolbar.append(&refresh);
        let filters = widgets::vbox(4);
        filters.add_css_class("notes-find");
        let first = widgets::hbox(6);
        let query = gtk::Entry::builder()
            .placeholder_text("Search / tag: / property:name=value")
            .hexpand(true)
            .build();
        let layout = gtk::DropDown::from_strings(&["Table", "List", "Kanban", "Gallery"]);
        first.append(&query);
        first.append(&layout);
        filters.append(&first);
        let second = widgets::hbox(6);
        let sort = gtk::Entry::builder()
            .text("title")
            .placeholder_text("Sort property")
            .width_chars(12)
            .build();
        let group = gtk::Entry::builder()
            .text("status")
            .placeholder_text("Group property")
            .width_chars(12)
            .build();
        let columns = gtk::Entry::builder()
            .text("status,tags")
            .placeholder_text("Columns, comma separated")
            .hexpand(true)
            .build();
        let descending = gtk::CheckButton::with_label("Descending");
        for w in [
            sort.upcast_ref::<gtk::Widget>(),
            group.upcast_ref(),
            columns.upcast_ref(),
            descending.upcast_ref(),
        ] {
            second.append(w);
        }
        filters.append(&second);
        root.append(&filters);
        let body = widgets::vbox(0);
        body.set_vexpand(true);
        root.append(&body);
        let status = gtk::Label::new(None);
        status.add_css_class("status-bar");
        root.append(&status);
        let view = Rc::new(CollectionView {
            ui: Rc::downgrade(self),
            root: root_path,
            db,
            window: window.clone(),
            body,
            query: query.clone(),
            layout: layout.clone(),
            sort: sort.clone(),
            group: group.clone(),
            columns: columns.clone(),
            descending: descending.clone(),
            status,
            revision: Cell::new(0),
            data: RefCell::new(Vec::new()),
        });
        let keep = RefCell::new(Some(view.clone()));
        window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
        let weak = Rc::downgrade(&view);
        refresh.connect_clicked(move |_| {
            if let Some(v) = weak.upgrade() {
                v.refresh();
            }
        });
        for e in [&query, &sort, &group, &columns] {
            let weak = Rc::downgrade(&view);
            e.connect_activate(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.refresh();
                }
            });
        }
        let weak = Rc::downgrade(&view);
        layout.connect_selected_notify(move |_| {
            if let Some(v) = weak.upgrade() {
                v.render();
            }
        });
        let weak = Rc::downgrade(&view);
        descending.connect_toggled(move |_| {
            if let Some(v) = weak.upgrade() {
                v.refresh();
            }
        });
        let weak = Rc::downgrade(&view);
        chooser.connect_selected_notify(move |c| {
            if let Some(v) = weak.upgrade()
                && let Some(config) = saved.get(c.selected().wrapping_sub(1) as usize)
            {
                v.query.set_text(&config.query);
                v.sort.set_text(&config.sort);
                v.group.set_text(&config.group);
                v.columns.set_text(&config.columns);
                v.descending.set_active(config.descending);
                v.layout.set_selected(
                    ["table", "list", "kanban", "gallery"]
                        .iter()
                        .position(|s| *s == config.layout)
                        .unwrap_or(0) as u32,
                );
                v.refresh();
            }
        });
        let weak = Rc::downgrade(&view);
        save.connect_clicked(move |_| {
            let Some(v) = weak.upgrade() else {
                return;
            };
            let Some(ui) = v.ui.upgrade() else {
                return;
            };
            let d = Prompt::new(
                &ui.window,
                "Save collection",
                "Save collection",
                "Views contain filters and layout only; note data remains Markdown.",
                480,
                180,
            );
            d.window.set_transient_for(Some(&v.window));
            let name = gtk::Entry::builder().placeholder_text("View name").build();
            d.content_area().append(&name);
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            d.add_button("Save", gtk::ResponseType::Accept);
            d.connect_response(move |d, r| {
                if r == gtk::ResponseType::Accept {
                    let name = name.text().trim().to_owned();
                    if name.is_empty() {
                        return;
                    }
                    let mut config = v.config();
                    config.name = name.clone();
                    prefs::update(|p| {
                        p.collections.retain(|c| c.name != name);
                        p.collections.push(config);
                    });
                    if let Some(ui) = v.ui.upgrade() {
                        ui.message("Collection saved; available in the next Collections window");
                    }
                }
                d.close();
            });
            d.present();
        });
        let chosen = chooser.clone();
        remove.connect_clicked(move |_| {
            if let Some(name) = chosen
                .selected_item()
                .and_downcast::<gtk::StringObject>()
                .map(|s| s.string().to_string())
            {
                prefs::update(|p| p.collections.retain(|c| c.name != name));
                chosen.set_selected(0);
            }
        });
        view.refresh();
        window.present();
    }
    fn collection_property(self: &Rc<Self>, path: PathBuf, key: String) {
        let root = self.root.borrow().clone();
        let Some(root) = root else {
            return;
        };
        if !path.starts_with(&root) {
            self.message("Choose a note in the active vault");
            return;
        }
        let existing = self.docs.borrow().iter().find(|d| d.path == path).cloned();
        let weak = Rc::downgrade(self);
        let source = existing
            .as_ref()
            .map(|d| (d.editor.text(), d.version.borrow().clone()));
        let base_revision = existing.as_ref().map(|d| d.editor.revision.get());
        let p = path.clone();
        let k = key.clone();
        worker::run(
            move || -> anyhow::Result<(String, Version)> {
                let (text, version) = match source {
                    Some(v) => v,
                    None => {
                        let d = vault::load(&p)?;
                        (d.text, d.version)
                    }
                };
                Ok((notes_editor_property(&text, &k), version))
            },
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let (value, expected) = match result {
                    Ok(v) => v,
                    Err(e) => {
                        ui.message(&format!("Property: {e:#}"));
                        return;
                    }
                };
                let dialog = Prompt::new(
                    &ui.window,
                    "Edit collection property",
                    "Edit property",
                    "Values use YAML syntax; changes update the corresponding Markdown source.",
                    520,
                    260,
                );
                let name = gtk::Entry::builder()
                    .text(&key)
                    .placeholder_text("Property")
                    .build();
                let input = gtk::TextView::builder().monospace(true).build();
                input.buffer().set_text(&value);
                let scroller = scroll(&input);
                scroller.set_min_content_height(100);
                dialog.content_area().append(&name);
                dialog.content_area().append(&scroller);
                dialog.add_button("Cancel", gtk::ResponseType::Cancel);
                dialog.add_button("Remove", gtk::ResponseType::Other(1));
                dialog.add_button("Apply", gtk::ResponseType::Accept);
                let weak = Rc::downgrade(&ui);
                dialog.connect_response(move |d, r| {
                    if matches!(r, gtk::ResponseType::Accept | gtk::ResponseType::Other(1))
                        && let Some(ui) = weak.upgrade()
                    {
                        let b = input.buffer();
                        let value = if r == gtk::ResponseType::Other(1) {
                            None
                        } else {
                            Some(b.text(&b.start_iter(), &b.end_iter(), true).to_string())
                        };
                        if let Some(revision) = base_revision
                            && ui
                                .docs
                                .borrow()
                                .iter()
                                .find(|d| d.path == path)
                                .is_some_and(|d| d.editor.revision.get() != revision)
                        {
                            ui.message("The editor changed. Reopen this property before applying.");
                            return;
                        }
                        ui.collection_update(
                            path.clone(),
                            name.text().to_string(),
                            value,
                            Some(expected.clone()),
                            || {},
                        );
                    }
                    d.close();
                });
                dialog.present();
            },
        );
    }
    fn collection_update(
        self: &Rc<Self>,
        path: PathBuf,
        key: String,
        value: Option<String>,
        expected: Option<Version>,
        done: impl FnOnce() + 'static,
    ) {
        if let Some(doc) = self.docs.borrow().iter().find(|d| d.path == path).cloned() {
            if doc.read_only || doc.conflict.get() || doc.recovery_pending.get() {
                self.message("Resolve this note's read-only or conflict state first");
                return;
            }
            match doc.editor.set_property(&key, value.as_deref()) {
                Ok(()) => {
                    let weak = Rc::downgrade(self);
                    self.save(
                        doc,
                        Some(Box::new(move |ok| {
                            if ok && let Some(ui) = weak.upgrade() {
                                ui.message(
                                    "Property saved. Refresh the collection after indexing.",
                                );
                                done();
                            }
                        })),
                    );
                }
                Err(e) => self.message(&e),
            }
            return;
        }
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let state = nyx_core::paths::state_dir();
        let db = self.db.borrow().clone();
        let p = prefs::get();
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<()> {
                knowledge::edit_property(
                    &state,
                    &root,
                    &path,
                    &key,
                    value.as_deref(),
                    (p.history_keep, p.history_days),
                    expected.as_ref(),
                )?;
                if let Some(db) = db {
                    vault::update_index(&db, &path)?;
                }
                Ok(())
            },
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(()) => {
                            ui.message("Property updated");
                            done();
                        }
                        Err(e) => ui.message(&format!("Property update: {e:#}")),
                    }
                }
            },
        );
    }
}
fn notes_editor_property(source: &str, key: &str) -> String {
    vault::knowledge::property_value(source, key).unwrap_or_default()
}
