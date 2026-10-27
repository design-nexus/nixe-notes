//! Phase 2 workspace actions. All disk work stays behind the vault worker boundary.
use super::*;
use notes_editor::{Asset, Mode};
use vault::workspace;
impl Ui {
    pub(super) fn feature_menu(self: &Rc<Self>, menu: &gtk::Box) {
        #[cfg(feature = "ai")]
        self.intelligence_menu(menu);
        #[cfg(feature = "sync")]
        self.sync_menu(menu);
        #[cfg(feature = "plugins")]
        self.plugins_menu(menu);
        for (label, action) in [
            ("Daily note", 0),
            ("New note from template…", 1),
            ("Insert template…", 2),
            ("Attach files…", 3),
            ("Paste image", 4),
            ("Edit note properties…", 5),
            ("Rename or move…", 6),
            ("Duplicate…", 7),
            ("Move to trash…", 8),
            ("Save this search…", 9),
            ("Saved searches…", 10),
            ("Vault preferences…", 11),
            ("Recover file operations…", 12),
            ("Document history…", 13),
            ("Focus mode · F11", 14),
            ("Writing workspace…", 15),
            ("Save workspace…", 16),
            ("Workspaces…", 17),
            ("Knowledge graph…", 18),
            ("Local graph…", 19),
            ("Collections…", 20),
            ("Writing projects…", 21),
            ("New canvas…", 22),
            ("Open canvas…", 23),
            ("Suite integration preferences…", 24),
            ("Insert from Capture…", 25),
            ("Save web clipping…", 26),
        ] {
            let button = gtk::Button::with_label(label);
            button.set_has_frame(false);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |button| {
                if let Some(popover) = button
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    popover.popdown();
                }
                if let Some(ui) = weak.upgrade() {
                    match action {
                        0 => ui.daily(),
                        1 => ui.template(false),
                        2 => ui.template(true),
                        3 => ui.attach_files(),
                        4 => ui.paste_image(),
                        5 => ui.edit_properties(),
                        6 => {
                            if let Some(doc) = ui.active() {
                                ui.move_note(doc.path.clone());
                            }
                        }
                        7 => {
                            if let Some(doc) = ui.active() {
                                ui.duplicate_note(doc.path.clone());
                            }
                        }
                        8 => {
                            if let Some(doc) = ui.active() {
                                ui.trash_note(doc.path.clone());
                            }
                        }
                        9 => ui.save_search(),
                        10 => ui.saved_searches(),
                        11 => ui.vault_preferences(),
                        12 => ui.recover_operations(),
                        13 => ui.history(),
                        14 => ui.toggle_focus(),
                        15 => ui.writing_preferences(),
                        16 => ui.save_workspace(),
                        17 => ui.workspaces(),
                        18 => ui.graph(false),
                        19 => ui.graph(true),
                        20 => ui.collections(),
                        21 => ui.projects(),
                        22 => ui.new_canvas(),
                        23 => ui.choose_canvas(),
                        24 => ui.integration_preferences(),
                        25 => ui.capture_insert(),
                        _ => ui.clip_dialog(),
                    }
                }
            });
            menu.append(&button);
        }
    }
    pub(super) fn sync_mode(&self) {
        let mode = if self.reading.is_active() {
            Mode::Reading
        } else if self.live.is_active() {
            Mode::Live
        } else {
            Mode::Source
        };
        for doc in self.docs.borrow().iter() {
            doc.editor.set_mode(mode);
        }
        prefs::update(|p| {
            p.editor_mode = match mode {
                Mode::Source => "source",
                Mode::Live => "live",
                Mode::Reading => "reading",
            }
            .into()
        });
    }
    pub(super) fn configure_editor(self: &Rc<Self>, editor: &Rc<Editor>, path: &std::path::Path) {
        editor.set_location(path.to_owned());
        let weak = Rc::downgrade(self);
        editor.set_asset_handler(Rc::new(move |source, target, wiki, done| {
            let Some(ui) = weak.upgrade() else { return };
            let (Some(root), Some(db)) = (ui.root.borrow().clone(), ui.db.borrow().clone()) else {
                done(Asset::Error("Open a vault to render embeds".into()));
                return;
            };
            worker::background(
                move || workspace::asset(&root, &db, &source, &target, wiki),
                move |result| {
                    done(match result {
                        Ok(workspace::AssetData::Image { bytes, .. }) => Asset::Image(bytes),
                        Ok(workspace::AssetData::Markdown { path, blocks }) => {
                            Asset::Markdown { path, blocks }
                        }
                        Ok(workspace::AssetData::File { path, label }) => {
                            Asset::File { path, label }
                        }
                        Err(e) => Asset::Error(format!("{e:#}")),
                    })
                },
            );
        }));
        let weak = Rc::downgrade(self);
        editor.set_link_handler(Rc::new(move |source, target, wiki| {
            if let Some(ui) = weak.upgrade() {
                ui.open_target(source, target, wiki);
            }
        }));
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        let target = path.to_owned();
        drop.connect_drop(move |_, value, _, _| {
            if let (Some(ui), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) {
                ui.attach_paths(
                    target.clone(),
                    files.files().iter().filter_map(|f| f.path()).collect(),
                );
                true
            } else {
                false
            }
        });
        editor.root.add_controller(drop);
    }
    pub(super) fn open_target(self: &Rc<Self>, source: PathBuf, target: String, wiki: bool) {
        if target.starts_with("https://") || target.starts_with("http://") {
            let weak = Rc::downgrade(self);
            gtk::UriLauncher::new(&target).launch(
                Some(&self.window),
                gio::Cancellable::NONE,
                move |result| {
                    if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                        ui.message(&format!("Could not open website: {e}"));
                    }
                },
            );
            return;
        }
        let (Some(root), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        worker::background(
            move || match vault::resolve_link(&db, &root, &source, &target, wiki) {
                Ok(link) => Ok((Some(link), None)),
                Err(error) => match workspace::asset(&root, &db, &source, &target, wiki) {
                    Ok(workspace::AssetData::File { path, .. }) => Ok((None, Some(path))),
                    Ok(workspace::AssetData::Image { path, .. }) => Ok((None, Some(path))),
                    _ => Err(error),
                },
            },
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok((Some(link), _)) => {
                            if let Some(fragment) = link.fragment {
                                ui.pending_jumps
                                    .borrow_mut()
                                    .insert(link.path.clone(), Jump::Reference(fragment));
                            }
                            ui.open_file(link.path);
                            if let Some(doc) = ui.active() {
                                ui.apply_pending_jump(&doc);
                            }
                        }
                        Ok((_, Some(path))) => ui.open_attachment(path),
                        Err(e) => ui.message(&format!("Could not open link: {e:#}")),
                        _ => {}
                    }
                }
            },
        );
    }
    pub(super) fn open_attachment(self: &Rc<Self>, path: PathBuf) {
        if path.extension().is_some_and(|e| e == "canvas") {
            self.open_canvas(path);
            return;
        }
        let weak = Rc::downgrade(self);
        gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |result| {
                if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                    ui.message(&format!("Could not open attachment: {e}"));
                }
            },
        );
    }
    fn clock() -> (String, String) {
        gtk::glib::DateTime::now_local()
            .map(|date| {
                (
                    date.format("%Y-%m-%d").unwrap_or_default().to_string(),
                    date.format("%H:%M").unwrap_or_default().to_string(),
                )
            })
            .unwrap_or_else(|_| ("1970-01-01".into(), "00:00".into()))
    }
    pub(super) fn daily(self: &Rc<Self>) {
        let Some(root) = self.root.borrow().clone() else {
            self.choose_vault();
            return;
        };
        let p = prefs::get();
        let template = (!p.daily_template.is_empty())
            .then(|| root.join(&p.templates_folder).join(&p.daily_template));
        let (date, time) = Self::clock();
        let weak = Rc::downgrade(self);
        worker::run(
            move || {
                workspace::daily_note(&root, &p.daily_folder, &date, template.as_deref(), &time)
            },
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(path) => {
                            ui.refresh();
                            ui.open_file(path);
                        }
                        Err(e) => ui.message(&format!("Daily note: {e:#}")),
                    }
                }
            },
        );
    }
    pub(super) fn template(self: &Rc<Self>, insert: bool) {
        let Some(root) = self.root.borrow().clone() else {
            self.choose_vault();
            return;
        };
        let p = prefs::get();
        let weak = Rc::downgrade(self);
        worker::run(
            move || workspace::templates(&root, &p.templates_folder),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                let paths = match result {
                    Ok(paths) if !paths.is_empty() => paths,
                    Ok(_) => {
                        ui.message("Add Markdown templates to the vault's Templates directory, or change it in Vault preferences.");
                        return;
                    }
                    Err(e) => {
                        ui.message(&format!("Templates: {e:#}"));
                        return;
                    }
                };
                let dialog = Prompt::new(
                    &ui.window,
                    if insert {
                        "Insert template"
                    } else {
                        "New note from template"
                    },
                    "",
                    "Choose a template. {{title}}, {{date}}, {{time}}, and {{selection}} are expanded.",
                    500,
                    240,
                );
                dialog.add_button("Cancel", gtk::ResponseType::Cancel);
                dialog.add_button(
                    if insert { "Insert" } else { "Create" },
                    gtk::ResponseType::Accept,
                );
                let names = paths
                    .iter()
                    .map(|p| {
                        p.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect::<Vec<_>>();
                let choice = gtk::DropDown::from_strings(
                    &names.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                dialog.content_area().append(&choice);
                let title = gtk::Entry::builder().placeholder_text("Note title").build();
                if !insert {
                    dialog.content_area().append(&title);
                }
                let weak = Rc::downgrade(&ui);
                dialog.connect_response(move |dialog, response| {
                    if response == gtk::ResponseType::Accept
                        && let Some(ui) = weak.upgrade()
                        && let Some(path) = paths.get(choice.selected() as usize)
                    {
                        ui.apply_template(path.clone(), insert, title.text().to_string());
                    }
                    dialog.close();
                });
                dialog.present();
            },
        );
    }
    fn apply_template(self: &Rc<Self>, path: PathBuf, insert: bool, title: String) {
        let (date, time) = Self::clock();
        if insert {
            let Some(doc) = self.active() else { return };
            let title = doc
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let selection = doc
                .editor
                .buffer
                .selection_bounds()
                .map(|(a, b)| doc.editor.buffer.text(&a, &b, true).to_string())
                .unwrap_or_default();
            let weak = Rc::downgrade(self);
            worker::run(
                move || {
                    vault::load(&path).map(|d| {
                        workspace::expand_template(&d.text, &title, &date, &time, &selection)
                    })
                },
                move |result| {
                    if let Some(ui) = weak.upgrade() {
                        match result {
                            Ok(text) => {
                                doc.editor.insert_text(&text);
                            }
                            Err(e) => ui.message(&format!("Template: {e:#}")),
                        }
                    }
                },
            );
        } else {
            let Some(root) = self.root.borrow().clone() else {
                return;
            };
            let weak = Rc::downgrade(self);
            worker::run(
                move || workspace::template_note(&root, "", &title, Some(&path), &date, &time),
                move |result| {
                    if let Some(ui) = weak.upgrade() {
                        match result {
                            Ok(path) => {
                                ui.refresh();
                                ui.open_file(path);
                            }
                            Err(e) => ui.message(&format!("Template note: {e:#}")),
                        }
                    }
                },
            );
        }
    }
    pub(super) fn edit_properties(self: &Rc<Self>) {
        let Some(doc) = self.active() else { return };
        if doc.read_only {
            self.message("This note is read-only");
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Note properties",
            "Edit frontmatter",
            "Values use YAML syntax. Empty values are valid; Remove deletes only this property.",
            520,
            260,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Remove", gtk::ResponseType::Other(1));
        dialog.add_button("Apply", gtk::ResponseType::Accept);
        let key = gtk::Entry::builder()
            .placeholder_text("Property name")
            .build();
        let value = gtk::TextView::builder()
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        let value_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(110)
            .max_content_height(260)
            .child(&value)
            .build();
        dialog.content_area().append(&key);
        dialog.content_area().append(&value_scroll);
        let properties = doc.editor.metadata().properties;
        let names = properties
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>();
        if !names.is_empty() {
            let choice = gtk::DropDown::from_strings(&names);
            let k = key.clone();
            let v = value.clone();
            let values = properties.clone();
            let editor = doc.editor.clone();
            choice.connect_selected_notify(move |choice| {
                if let Some((name, _)) = values.get(choice.selected() as usize) {
                    k.set_text(name);
                    v.buffer()
                        .set_text(&editor.property_value(name).unwrap_or_default());
                }
            });
            if let Some((name, _)) = properties.first() {
                key.set_text(name);
                value
                    .buffer()
                    .set_text(&doc.editor.property_value(name).unwrap_or_default());
            }
            dialog.content_area().prepend(&choice);
        }
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if matches!(
                response,
                gtk::ResponseType::Accept | gtk::ResponseType::Other(1)
            ) && let Some(ui) = weak.upgrade()
            {
                let buffer = value.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
                match doc.editor.set_property(
                    &key.text(),
                    if response == gtk::ResponseType::Other(1) {
                        None
                    } else {
                        Some(text.as_str())
                    },
                ) {
                    Ok(()) => {}
                    Err(error) => {
                        ui.message(&error);
                        return;
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn attach_files(self: &Rc<Self>) {
        let Some(doc) = self.active() else {
            self.message("Open the destination note first");
            return;
        };
        let dialog = gtk::FileDialog::builder().title("Attach files").build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let (Some(ui), Ok(files)) = (weak.upgrade(), result) {
                let paths = (0..files.n_items())
                    .filter_map(|i| {
                        files
                            .item(i)
                            .and_downcast::<gio::File>()
                            .and_then(|f| f.path())
                    })
                    .collect();
                ui.attach_paths(doc.path.clone(), paths);
            }
        });
    }
    pub(super) fn attach_paths(self: &Rc<Self>, note: PathBuf, paths: Vec<PathBuf>) {
        let Some(doc) = self.docs.borrow().iter().find(|d| d.path == note).cloned() else {
            return;
        };
        if doc.read_only {
            self.message("This note is read-only");
            return;
        }
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        if !note.starts_with(&root) {
            self.message("Switch to this note's vault before adding attachments");
            return;
        }
        let folder = prefs::get().attachments_folder;
        let weak = Rc::downgrade(self);
        worker::run(
            move || {
                let mut links = Vec::new();
                for path in paths {
                    let target = workspace::attach(&root, &folder, &path)?;
                    let ext = target
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();
                    links.push(workspace::relative_link(
                        &note,
                        &target,
                        matches!(
                            ext.as_str(),
                            "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp"
                        ),
                    ));
                }
                Ok::<_, anyhow::Error>(links.join("\n"))
            },
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(text) => {
                            doc.editor.insert_text(&text);
                            ui.refresh();
                        }
                        Err(e) => ui.message(&format!("Attachment: {e:#}")),
                    }
                }
            },
        );
    }
    pub(super) fn paste_image(self: &Rc<Self>) {
        let Some(doc) = self.active() else { return };
        let clipboard = self.window.clipboard();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = clipboard.read_texture_future().await;
            let Some(ui) = weak.upgrade() else { return };
            match result {
                Ok(Some(texture)) => {
                    let bytes = texture.save_to_png_bytes();
                    let Some(root) = ui.root.borrow().clone() else {
                        return;
                    };
                    let folder = prefs::get().attachments_folder;
                    let name = format!(
                        "Screenshot-{}.png",
                        glib::DateTime::now_local()
                            .and_then(|d| d.format("%Y%m%d-%H%M%S"))
                            .unwrap_or_default()
                    );
                    let note = doc.path.clone();
                    let weak = Rc::downgrade(&ui);
                    worker::run(
                        move || {
                            workspace::attach_bytes(&root, &folder, &name, bytes.as_ref())
                                .map(|p| workspace::relative_link(&note, &p, true))
                        },
                        move |result| {
                            if let Some(ui) = weak.upgrade() {
                                match result {
                                    Ok(link) => {
                                        doc.editor.insert_text(&link);
                                        ui.refresh();
                                    }
                                    Err(e) => ui.message(&format!("Paste image: {e:#}")),
                                }
                            }
                        },
                    );
                }
                _ => ui.message("The clipboard does not contain an image"),
            }
        });
    }
    pub(super) fn toggle_split(self: &Rc<Self>) {
        let enabled = !self.secondary.is_visible();
        self.secondary.set_visible(enabled);
        if enabled {
            if self.notebook.n_pages() > 1
                && let Some(doc) = self.active()
            {
                self.move_tab(&doc, true);
            }
            self.active_pane.set(true);
            self.editor_split
                .set_position((self.editor_split.width() / 2).max(240));
            self.message("Split view is open. Select a note to open it in the right pane; click either pane to work there.");
        } else {
            let docs = self.docs.borrow().clone();
            for doc in docs {
                if self.secondary.page_num(&doc.editor.root).is_some() {
                    self.move_tab(&doc, false);
                }
            }
            self.active_pane.set(false);
        }
        prefs::update(|p| p.split_enabled = enabled);
        self.update_active();
    }
    pub(super) fn move_tab(&self, doc: &Doc, right: bool) {
        let source = self.doc_notebook(doc);
        let Some(page) = source.page_num(&doc.editor.root) else {
            return;
        };
        let label = source.tab_label(&doc.editor.root);
        source.remove_page(Some(page));
        let target = if right {
            &self.secondary
        } else {
            &self.notebook
        };
        target.append_page(&doc.editor.root, label.as_ref());
        target.set_tab_reorderable(&doc.editor.root, true);
        target.set_current_page(target.page_num(&doc.editor.root));
    }
    pub(super) fn toggle_task(self: &Rc<Self>) {
        if let Some(doc) = self.active()
            && !doc.editor.toggle_task()
        {
            self.message("Place the cursor on a task checkbox line");
        }
    }
    pub(super) fn fold(self: &Rc<Self>) {
        if let Some(doc) = self.active() {
            doc.editor.fold_heading();
        }
    }
    pub(super) fn cycle_mode(&self) {
        if self.reading.is_active() {
            self.reading.set_active(false);
            self.live.set_active(false);
        } else if self.live.is_active() {
            self.reading.set_active(true);
        } else {
            self.live.set_active(true);
        }
    }
    pub(super) fn vault_preferences(self: &Rc<Self>) {
        let p = prefs::get();
        let dialog = Prompt::new(
            &self.window,
            "Vault preferences",
            "Vault folders",
            "Folders are relative to the vault. Existing Obsidian configuration is never modified.",
            540,
            360,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Apply", gtk::ResponseType::Accept);
        let mut entries = Vec::new();
        for (label, text) in [
            ("Templates", p.templates_folder),
            ("Daily notes", p.daily_folder),
            ("Daily template filename (optional)", p.daily_template),
            ("Attachments", p.attachments_folder),
        ] {
            let entry = gtk::Entry::builder().text(text).build();
            dialog
                .content_area()
                .append(&widgets::row(label, "", Some(entry.upcast_ref())));
            entries.push(entry);
        }
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let values = entries
                    .iter()
                    .map(|e| e.text().to_string())
                    .collect::<Vec<_>>();
                prefs::update(|p| {
                    p.templates_folder = values[0].clone();
                    p.daily_folder = values[1].clone();
                    p.daily_template = values[2].clone();
                    p.attachments_folder = values[3].clone();
                });
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn save_search(&self) {
        let query = self.search.text().to_string();
        if query.trim().is_empty() {
            self.message("Enter a search before saving it");
            return;
        }
        let dialog = Prompt::new(&self.window, "Save search", "", "", 420, 160);
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Save", gtk::ResponseType::Accept);
        let name = gtk::Entry::builder()
            .placeholder_text("Search name")
            .build();
        dialog.content_area().append(&name);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept && !name.text().trim().is_empty() {
                prefs::update(|p| {
                    p.saved_searches.retain(|(n, _)| n != name.text().as_str());
                    p.saved_searches
                        .push((name.text().to_string(), query.clone()));
                });
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn saved_searches(self: &Rc<Self>) {
        let searches = prefs::get().saved_searches;
        let dialog = Prompt::new(
            &self.window,
            "Saved searches",
            "",
            "Select a saved query, or remove it.",
            500,
            300,
        );
        dialog.add_button("Close", gtk::ResponseType::Close);
        let list = widgets::vbox(4);
        for (name, query) in searches {
            let row = widgets::hbox(4);
            let button = gtk::Button::with_label(&name);
            button.set_hexpand(true);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.show_search();
                    ui.search.set_text(&query);
                }
            });
            row.append(&button);
            let remove = widgets::bar_button("edit-delete-symbolic", "Remove saved search");
            let row_copy = row.clone();
            remove.connect_clicked(move |_| {
                prefs::update(|p| p.saved_searches.retain(|(n, _)| n != &name));
                row_copy.set_visible(false);
            });
            row.append(&remove);
            list.append(&row);
        }
        dialog.content_area().append(&scroll(&list));
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
    pub(super) fn clean_for_operation(&self) -> bool {
        if self
            .canvases
            .borrow()
            .values()
            .any(|(_, dirty)| dirty.get())
        {
            self.message(
                "Save or resolve canvas changes before file operations or workspace switching.",
            );
            return false;
        }
        if self
            .docs
            .borrow()
            .iter()
            .any(|d| d.dirty.get() || d.saving.get() || d.recovery_pending.get())
        {
            self.message("Save or resolve open notes before changing vault paths. Use Save all from the File menu.");
            false
        } else {
            true
        }
    }
    pub(super) fn save_all(self: &Rc<Self>) {
        let next = self.docs.borrow().iter().find(|d| d.dirty.get()).cloned();
        if let Some(doc) = next {
            let weak = Rc::downgrade(self);
            self.save(
                doc,
                Some(Box::new(move |ok| {
                    if let Some(ui) = weak.upgrade() {
                        if ok {
                            ui.save_all();
                        } else {
                            ui.message("A note could not be saved; resolve it before continuing");
                        }
                    }
                })),
            );
        } else {
            self.message("All open notes are saved");
        }
    }
    pub(super) fn move_note(self: &Rc<Self>, path: PathBuf) {
        if !self.clean_for_operation() {
            return;
        }
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let dialog = Prompt::new(
            &self.window,
            "Rename or move",
            "",
            "Enter the new vault-relative path. Incoming and relative links are updated safely.",
            560,
            180,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Review changes", gtk::ResponseType::Accept);
        let entry = gtk::Entry::builder()
            .text(path.strip_prefix(&root).unwrap_or(&path).to_string_lossy())
            .build();
        dialog.content_area().append(&entry);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let destination = root.join(entry.text().as_str());
                let root = root.clone();
                let source = path.clone();
                let Some(db) = ui.db.borrow().clone() else {
                    return;
                };
                let state = nyx_core::paths::state_dir();
                let weak = Rc::downgrade(&ui);
                worker::run(
                    move || workspace::plan_move(&root, &db, &state, &source, &destination),
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(plan) => ui.confirm_move(plan),
                                Err(e) => ui.message(&format!("Move: {e:#}")),
                            }
                        }
                    },
                );
            }
            dialog.close();
        });
        dialog.present();
    }
    fn confirm_move(self: &Rc<Self>, plan: workspace::MovePlan) {
        let dialog = Prompt::new(
            &self.window,
            "Apply move",
            "",
            &format!(
                "{} → {}\n{} Markdown files require link updates. An interrupted operation can be resumed or restored.",
                plan.source.display(),
                plan.destination.display(),
                plan.changes.len()
            ),
            600,
            220,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Move and update links", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                ui.run_move(plan.clone(), false);
            }
            dialog.close();
        });
        dialog.present();
    }
    fn run_move(self: &Rc<Self>, mut plan: workspace::MovePlan, restore: bool) {
        if !self.clean_for_operation() {
            return;
        }
        self.mutating.set(true);
        let old = plan.source.clone();
        let new = plan.destination.clone();
        let weak = Rc::downgrade(self);
        worker::run(
            move || {
                if restore {
                    workspace::restore_move(&mut plan)
                } else {
                    workspace::execute_move(&mut plan)
                }
            },
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                ui.mutating.set(false);
                match result {
                    Ok(())=>{
                        let canvases=ui.canvases.borrow().iter().filter(|(_,(_,dirty))|!dirty.get()).map(|(path,(window,_))|(path.clone(),window.clone())).collect::<Vec<_>>();
                        for(path,window)in canvases{window.close();let mapped=if !restore{path.strip_prefix(&old).ok().map(|tail|new.join(tail)).unwrap_or(path)}else{path};ui.open_canvas(mapped);}
                        let active=ui.active().map(|d|d.path.clone());
                        let mut snapshots=ui.docs.borrow().iter().map(|doc|{let path=if !restore&&doc.path==old{new.clone()}else if !restore&&doc.path.starts_with(&old){new.join(doc.path.strip_prefix(&old).unwrap())}else{doc.path.clone()};let right=ui.secondary.page_num(&doc.editor.root).is_some();let text=doc.editor.text();let byte=text.char_indices().nth(doc.editor.buffer.cursor_position()as usize).map_or(text.len(),|(b,_)|b);(path,right,byte,active.as_ref()==Some(&doc.path))}).collect::<Vec<_>>();snapshots.sort_by_key(|(_,_,_,active)|*active);
                        let docs=ui.docs.borrow().clone();for doc in docs{ui.remove_tab(doc);}
                        for(path,right,offset,_)in snapshots{ui.pending_panes.borrow_mut().insert(path.clone(),right);ui.pending_jumps.borrow_mut().insert(path.clone(),Jump::Offset(offset));ui.open_file(path);}
                        ui.refresh();ui.message("File operation completed; links and open notes are refreshed.");
                    },
                    Err(e)=>ui.message(&format!("File operation stopped: {e:#}. Use Recover file operations to resume or restore."))
                }
            },
        );
    }
    pub(super) fn recover_operations(self: &Rc<Self>) {
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || workspace::interrupted_moves(&state),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                let plans = match result {
                    Ok(p) => p,
                    Err(e) => {
                        ui.message(&format!("Recovery: {e:#}"));
                        return;
                    }
                };
                if plans.is_empty() {
                    ui.message("No interrupted file operations");
                    return;
                }
                for plan in plans {
                    let dialog = Prompt::new(
                        &ui.window,
                        "Recover file operation",
                        "",
                        &format!("{} → {}", plan.source.display(), plan.destination.display()),
                        560,
                        200,
                    );
                    dialog.add_button("Later", gtk::ResponseType::Cancel);
                    dialog.add_button("Restore originals", gtk::ResponseType::Other(1));
                    dialog.add_button("Resume", gtk::ResponseType::Accept);
                    let weak = Rc::downgrade(&ui);
                    dialog.connect_response(move |dialog, response| {
                        if let Some(ui) = weak.upgrade()
                            && matches!(
                                response,
                                gtk::ResponseType::Accept | gtk::ResponseType::Other(1)
                            )
                        {
                            ui.run_move(plan.clone(), response == gtk::ResponseType::Other(1));
                        }
                        dialog.close();
                    });
                    dialog.present();
                }
            },
        );
    }
    pub(super) fn duplicate_note(self: &Rc<Self>, path: PathBuf) {
        if !self.clean_for_operation() {
            return;
        }
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let dialog = Prompt::new(
            &self.window,
            "Duplicate file",
            "",
            "New vault-relative path",
            500,
            180,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Duplicate", gtk::ResponseType::Accept);
        let name = gtk::Entry::builder().placeholder_text("Copy.md").build();
        dialog.content_area().append(&name);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let root = root.clone();
                let source = path.clone();
                let destination = root.join(name.text().as_str());
                let weak = Rc::downgrade(&ui);
                worker::run(
                    move || workspace::duplicate(&root, &source, &destination),
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(path) => {
                                    ui.refresh();
                                    if vault::is_markdown(&path) {
                                        ui.open_file(path);
                                    }
                                }
                                Err(e) => ui.message(&format!("Duplicate: {e:#}")),
                            }
                        }
                    },
                );
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn trash_note(self: &Rc<Self>, path: PathBuf) {
        if !self.clean_for_operation() {
            return;
        }
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let dialog = Prompt::new(
            &self.window,
            "Move to trash",
            "",
            &format!(
                "Trash {}? It remains recoverable through the desktop trash. Incoming links are retained.",
                path.display()
            ),
            500,
            200,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Move to trash", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let root = root.clone();
                let path = path.clone();
                let affected = path.clone();
                let weak = Rc::downgrade(&ui);
                ui.mutating.set(true);
                worker::run(
                    move || workspace::trash(&root, &path),
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            ui.mutating.set(false);
                            match result {
                                Ok(()) => {
                                    let docs = ui
                                        .docs
                                        .borrow()
                                        .iter()
                                        .filter(|d| {
                                            d.path == affected || d.path.starts_with(&affected)
                                        })
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    for doc in docs {
                                        ui.remove_tab(doc);
                                    }
                                    ui.refresh();
                                }
                                Err(e) => ui.message(&format!("Trash: {e:#}")),
                            }
                        }
                    },
                );
            }
            dialog.close();
        });
        dialog.present();
    }
}
