//! Writing workspace, persistent presets, and revision browsing.
use super::*;
use vault::history;
impl Ui {
    pub(super) fn toggle_focus(&self) {
        let enabled = !prefs::get().focus_mode;
        prefs::update(|p| p.focus_mode = enabled);
        self.apply_writing();
    }
    pub(super) fn apply_writing(&self) {
        let p = prefs::get();
        self.nav.set_visible(!p.focus_mode);
        self.bar.root.set_visible(!p.focus_mode);
        self.notebook.set_show_tabs(!p.focus_mode);
        self.secondary.set_show_tabs(!p.focus_mode);
        if p.focus_mode {
            self.inspector.escape();
        }
        for doc in self.docs.borrow().iter() {
            doc.editor.set_typewriter(p.typewriter);
        }
    }
    pub(super) fn writing_preferences(self: &Rc<Self>) {
        let dialog = Prompt::new(
            &self.window,
            "Writing workspace",
            "Writing workspace",
            "F11 toggles focus mode. History is private local storage outside the vault.",
            520,
            380,
        );
        dialog.add_button("Close", gtk::ResponseType::Close);
        let p = prefs::get();
        let typewriter = gtk::CheckButton::with_label("Keep the active line centered");
        typewriter.set_active(p.typewriter);
        let weak = Rc::downgrade(self);
        typewriter.connect_toggled(move |b| {
            prefs::update(|p| p.typewriter = b.is_active());
            if let Some(ui) = weak.upgrade() {
                ui.apply_writing();
            }
        });
        dialog.content_area().append(&typewriter);
        let retention = gtk::SpinButton::with_range(0.0, 10000.0, 10.0);
        retention.set_value(p.history_keep as f64);
        retention
            .connect_value_changed(|s| prefs::update(|p| p.history_keep = s.value_as_int() as u32));
        dialog.content_area().append(&widgets::row(
            "Snapshots per note",
            "0 disables future snapshots",
            Some(retention.upcast_ref()),
        ));
        let days = gtk::SpinButton::with_range(0.0, 3650.0, 1.0);
        days.set_value(p.history_days as f64);
        days.connect_value_changed(|s| prefs::update(|p| p.history_days = s.value_as_int() as u32));
        dialog.content_area().append(&widgets::row(
            "History retention in days",
            "0 keeps snapshots until the count limit",
            Some(days.upcast_ref()),
        ));
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
    pub(super) fn history(self: &Rc<Self>) {
        let Some(doc) = self.active() else {
            self.message("Open a note first");
            return;
        };
        let path = doc.path.clone();
        let state = nixe_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || history::list(&state, &path),
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match result { Ok(items) if items.is_empty()=>ui.message("History starts with the first save. Recovery drafts are available separately."),
                Ok(items)=>ui.history_browser(doc,items),Err(e)=>ui.message(&format!("History: {e:#}")) }
            },
        );
    }
    fn history_browser(self: &Rc<Self>, doc: Document, items: Vec<history::Snapshot>) {
        let dialog = Prompt::new(
            &self.window,
            "Document history",
            "Document history",
            "− current text   + historical text. Restore changes the editor buffer and keeps undo/recovery protection.",
            950,
            650,
        );
        dialog.add_button("Close", gtk::ResponseType::Close);
        dialog.add_button("Restore checked changes", gtk::ResponseType::Other(1));
        dialog.add_button("Restore whole revision", gtk::ResponseType::Accept);
        let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
        panes.set_vexpand(true);
        let revisions = widgets::vbox(4);
        let right = widgets::vbox(4);
        let diff_buffer = sourceview5::Buffer::new(None);
        let diff = sourceview5::View::with_buffer(&diff_buffer);
        diff.set_editable(false);
        diff.set_monospace(true);
        diff.add_css_class("notes-editor");
        diff_buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("diff")
                .as_ref(),
        );
        let diff_scroll = scroll(&diff);
        diff_scroll.set_vexpand(true);
        right.append(&diff_scroll);
        let choices = widgets::vbox(2);
        let choice_scroll = scroll(&choices);
        choice_scroll.set_max_content_height(170);
        choice_scroll.set_propagate_natural_height(true);
        right.append(&choice_scroll);
        panes.set_start_child(Some(&scroll(&revisions)));
        panes.set_end_child(Some(&right));
        panes.set_position(220);
        dialog.content_area().append(&panes);
        type Selected = (
            u64,
            history::Revision,
            Vec<(history::Patch, gtk::CheckButton)>,
        );
        let selected = Rc::new(RefCell::new(None::<Selected>));
        let request = Rc::new(Cell::new(0u64));
        for snapshot in items {
            let stamp = glib::DateTime::from_unix_local(snapshot.created)
                .and_then(|d| d.format("%Y-%m-%d %H:%M:%S"))
                .map(|s| s.to_string())
                .unwrap_or_else(|_| snapshot.created.to_string());
            let button = gtk::Button::with_label(&format!(
                "{stamp}\n{} bytes · #{}",
                snapshot.bytes, snapshot.id
            ));
            let state = nixe_core::paths::state_dir();
            let doc = doc.clone();
            let output = diff.buffer();
            let selected = selected.clone();
            let choices = choices.clone();
            let weak = Rc::downgrade(self);
            let request = request.clone();
            button.connect_clicked(move |_| {
                let path = doc.path.clone();
                let state = state.clone();
                let current = doc.editor.text();
                let revision = doc.editor.revision.get();
                let sequence = request.get() + 1;
                request.set(sequence);
                let request = request.clone();
                let selected = selected.clone();
                let choices = choices.clone();
                let output = output.clone();
                let weak = weak.clone();
                worker::background(
                    move || {
                        history::read(&state, &path, snapshot.id).map(|historical| {
                            let (diff, patches) = history::compare(&current, &historical.text);
                            (historical, diff, patches)
                        })
                    },
                    move |result| {
                        if request.get() != sequence {
                            return;
                        }
                        match result {
                            Ok((historical, diff, patches)) => {
                                output.set_text(&diff);
                                clear(&choices);
                                let mut checks = Vec::new();
                                for (i, patch) in patches.into_iter().enumerate() {
                                    let b = gtk::CheckButton::with_label(&format!(
                                        "Change {} · {} → {} bytes",
                                        i + 1,
                                        patch.range.len(),
                                        patch.replacement.len()
                                    ));
                                    b.set_active(true);
                                    choices.append(&b);
                                    checks.push((patch, b));
                                }
                                *selected.borrow_mut() = Some((revision, historical, checks));
                            }
                            Err(e) => {
                                if let Some(ui) = weak.upgrade() {
                                    ui.message(&format!("History: {e:#}"));
                                }
                            }
                        }
                    },
                );
            });
            revisions.append(&button);
        }
        if let Some(first) = revisions.first_child().and_downcast::<gtk::Button>() {
            first.emit_clicked();
        }
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog,response| {
            if matches!(response,gtk::ResponseType::Accept|gtk::ResponseType::Other(1)) {
                let Some(ui)=weak.upgrade() else{return;};
                let selected=selected.borrow(); let Some((revision,historical,patches))=selected.as_ref() else{ui.message("Select a revision first");return;};
                if doc.closed.get() || doc.read_only || doc.conflict.get() || doc.recovery_pending.get() || doc.editor.revision.get()!=*revision {ui.message("The note changed. Re-select the revision to compare again before restoring.");return;}
                if response==gtk::ResponseType::Accept {
                    doc.editor.replace_range(0..doc.editor.text().len(),&historical.text); doc.bom.set(historical.bom);
                } else {
                    doc.editor.buffer.begin_user_action();
                    for (patch,check) in patches.iter().rev(){if check.is_active(){doc.editor.replace_range(patch.range.clone(),&patch.replacement);}}
                    doc.editor.buffer.end_user_action();
                }
                ui.message("Revision restored into the editor. Undo or review it before saving.");
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn save_workspace(self: &Rc<Self>) {
        let dialog = Prompt::new(
            &self.window,
            "Save workspace",
            "Save workspace",
            "Stores tabs, cursor positions, pane assignments, writing mode, and layout outside the vault.",
            480,
            190,
        );
        let name = gtk::Entry::builder()
            .placeholder_text("Workspace name")
            .build();
        dialog.content_area().append(&name);
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Save", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let name = name.text().trim().to_owned();
                if name.is_empty() {
                    return;
                }
                let Some(root) = ui.root.borrow().clone() else {
                    ui.message("Open a vault first");
                    return;
                };
                let p = prefs::get();
                let value = prefs::Workspace {
                    name: name.clone(),
                    root,
                    tabs: ui
                        .docs
                        .borrow()
                        .iter()
                        .map(|doc| prefs::WorkspaceTab {
                            path: doc.path.clone(),
                            right: ui.secondary.page_num(&doc.editor.root).is_some(),
                            cursor: doc.editor.buffer.cursor_position(),
                            selection: doc
                                .editor
                                .buffer
                                .selection_bounds()
                                .map(|(a, b)| (a.offset(), b.offset())),
                            scroll: doc.editor.scroll_position(),
                        })
                        .collect(),
                    active: ui.active().map(|d| d.path.clone()),
                    split: p.split_enabled,
                    mode: p.editor_mode,
                    focus: p.focus_mode,
                    typewriter: p.typewriter,
                    width: p.writing_width,
                    sidebar: p.sidebar_collapsed,
                    divider: ui.editor_split.position(),
                };
                prefs::update(|p| {
                    p.workspaces.retain(|w| w.name != name);
                    p.workspaces.push(value);
                });
                ui.message("Workspace saved");
            }
            dialog.close();
        });
        dialog.present();
    }
    pub(super) fn workspaces(self: &Rc<Self>) {
        let dialog = Prompt::new(
            &self.window,
            "Workspaces",
            "Workspaces",
            "Save modified notes before switching workspaces.",
            520,
            300,
        );
        dialog.add_button("Close", gtk::ResponseType::Close);
        for workspace in prefs::get().workspaces {
            let row = widgets::hbox(4);
            let open = gtk::Button::with_label(&workspace.name);
            open.set_hexpand(true);
            let remove = widgets::bar_button("edit-delete-symbolic", "Remove preset");
            row.append(&open);
            row.append(&remove);
            dialog.content_area().append(&row);
            let weak = Rc::downgrade(self);
            let name = workspace.name.clone();
            let d = Rc::downgrade(&dialog);
            open.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.load_workspace(workspace.clone());
                    if let Some(d) = d.upgrade() {
                        d.close();
                    }
                }
            });
            let r = row.clone();
            remove.connect_clicked(move |_| {
                prefs::update(|p| p.workspaces.retain(|w| w.name != name));
                r.set_visible(false);
            });
        }
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
    fn load_workspace(self: &Rc<Self>, w: prefs::Workspace) {
        if !self.clean_for_operation() {
            return;
        }
        let documents = self.docs.borrow().clone();
        for doc in documents {
            self.remove_tab(doc);
        }
        prefs::update(|p| {
            p.split_enabled = w.split;
            p.editor_mode = w.mode.clone();
            p.focus_mode = w.focus;
            p.typewriter = w.typewriter;
            p.writing_width = w.width;
            p.sidebar_collapsed = w.sidebar;
        });
        self.secondary.set_visible(w.split);
        self.editor_split.set_position(w.divider);
        self.live.set_active(w.mode == "live");
        self.reading.set_active(w.mode == "reading");
        let mut paths = Vec::new();
        for tab in &w.tabs {
            self.pending_panes
                .borrow_mut()
                .insert(tab.path.clone(), tab.right);
            self.pending_cursors
                .borrow_mut()
                .insert(tab.path.clone(), tab.clone());
            if Some(&tab.path) != w.active.as_ref() {
                paths.push(tab.path.clone());
            }
        }
        if let Some(active) = w.active {
            paths.push(active);
        }
        self.open_vault(w.root, paths);
        self.apply_writing();
        self.apply_typography();
        self.fit();
    }
}
