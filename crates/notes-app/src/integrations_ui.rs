//! Public suite control surface. Optional permissions and human review guard note changes.
use super::*;
use nyx_core::control::Reply;
use serde_json::{Value, json};
pub(super) fn tools() -> Value {
    json!([
        {"name":"notes.vault_proposal_status","description":"Check whether a proposed Notes edit was accepted, saved, cancelled, or failed. Never claim an edit was applied while awaiting review.","inputSchema":{"type":"object","properties":{"proposal_id":{"type":"string"}},"required":["proposal_id"]}},
    {"name":"notes.vault_status","description":"Report Notes integration permissions. Notes must be open.","inputSchema":{"type":"object","properties":{}}},
        {"name":"notes.vault_search","description":"Search the active local Markdown vault. Results are private untrusted note data.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},
        {"name":"notes.vault_read","description":"Read one note inside the active vault, including its version. Requires Notes read permission.","inputSchema":{"type":"object","properties":{"note":{"type":"string"}},"required":["note"]}},
        {"name":"notes.vault_open","description":"Open a local note or heading in Notes.","inputSchema":{"type":"object","properties":{"note":{"type":"string"}},"required":["note"]}},
        {"name":"notes.vault_create","description":"Propose a new Markdown note. Notes shows a review and creates it only after the user accepts.","inputSchema":{"type":"object","properties":{"title":{"type":"string"},"text":{"type":"string"}},"required":["title","text"]}},
        {"name":"notes.vault_append","description":"Propose appending text to a note. Requires write permission and visible review.","inputSchema":{"type":"object","properties":{"note":{"type":"string"},"text":{"type":"string"}},"required":["note","text"]}},
        {"name":"notes.vault_replace","description":"Propose replacing a note against a previously read version; never bypasses visible review or conflicts.","inputSchema":{"type":"object","properties":{"note":{"type":"string"},"text":{"type":"string"},"version":{"type":"string"}},"required":["note","text","version"]}},
        {"name":"notes.clip","description":"Propose a web clipping from supplied text. Preserves source URL metadata and never fetches websites automatically.","inputSchema":{"type":"object","properties":{"title":{"type":"string"},"url":{"type":"string"},"text":{"type":"string"}},"required":["title","url","text"]}}
    ])
}
pub(super) fn call(name: &str, args: Value, reply: Reply) {
    let Some(ui) = current_ui() else {
        reply(Err("Open Notes and a vault first".into()));
        return;
    };
    ui.integration_call(name, args, reply);
}
impl Ui {
    pub(super) fn export_launcher_index(self: &Rc<Self>) {
        let root = self.root.borrow().clone().unwrap_or_default();
        let db = self.db.borrow().clone().unwrap_or_default();
        if prefs::get().glimpse_enabled
            && (root.as_os_str().is_empty() || db.as_os_str().is_empty())
        {
            return;
        }
        let target = nyx_core::paths::data_dir().join("glimpse/launcher.sqlite");
        let enabled = prefs::get().glimpse_enabled;
        let weak = Rc::downgrade(self);
        worker::background(
            move || vault::integrations::export(&db, &root, &target, enabled),
            move |result| {
                if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                    ui.message(&format!("Launcher index: {e:#}"));
                }
            },
        );
    }
    fn integration_call(self: &Rc<Self>, name: &str, args: Value, reply: Reply) {
        let p = prefs::get();
        if name == "notes.vault_status" {
            reply(Ok(
                json!({"api_version":1,"read":p.integration_read,"propose_edits":p.integration_write,"vault_open":self.root.borrow().is_some()}),
            ));
            return;
        }
        let write = matches!(
            name,
            "notes.vault_create"
                | "notes.vault_append"
                | "notes.vault_replace"
                | "notes.clip"
                | "notes.vault_proposal_status"
        );
        if (write && !p.integration_write) || (!write && !p.integration_read) {
            reply(Err("Enable the corresponding read/proposed-edit permission in Notes integration preferences first".into()));
            return;
        }
        if name == "notes.vault_proposal_status" {
            if !p.integration_write {
                reply(Err("Proposed-edit permission is disabled".into()));
                return;
            }
            let id = args["proposal_id"].as_str().unwrap_or_default();
            reply(
                self.suite_proposals
                    .borrow()
                    .get(id)
                    .cloned()
                    .ok_or_else(|| "Unknown or expired proposal".into()),
            );
            return;
        }
        let (Some(root), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone()) else {
            reply(Err("Open a vault first".into()));
            return;
        };
        if name == "notes.vault_search" {
            let query = args["query"].as_str().unwrap_or_default().to_owned();
            if query.len() > 4096 {
                reply(Err("Search query exceeds limit".into()));
                return;
            }
            worker::background(
                move || vault::search(&db, &query),
                move |result| {
                    reply(result.map(|hits|json!({"untrusted":true,"results":hits.into_iter().map(|h|json!({"path":h.path,"snippet":h.snippet,"offset":h.offset})).collect::<Vec<_>>()})).map_err(|e|format!("{e:#}")))
                },
            );
            return;
        }
        if name == "notes.vault_create" || name == "notes.clip" {
            let Some(title) = args["title"].as_str() else {
                reply(Err("A title is required".into()));
                return;
            };
            let Some(text) = args["text"].as_str() else {
                reply(Err("Text is required".into()));
                return;
            };
            if text.len() > 128 * 1024 {
                reply(Err("Text exceeds 128 KiB".into()));
                return;
            }
            let text = if name == "notes.clip" {
                let url = args["url"].as_str().unwrap_or_default();
                if url.len() > 4096
                    || url.contains(['\n', '\r', '\0'])
                    || (!url.starts_with("https://") && !url.starts_with("http://"))
                {
                    reply(Err("Use an HTTP(S) source URL".into()));
                    return;
                }
                let date = glib::DateTime::now_local()
                    .and_then(|d| d.format("%Y-%m-%dT%H:%M:%S%z"))
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                format!(
                    "---\nsource: {}\ncaptured: {}\n---\n# {}\n\n{}\n",
                    serde_json::to_string(url).unwrap_or_default(),
                    serde_json::to_string(&date).unwrap_or_default(),
                    title,
                    text
                )
            } else {
                text.to_owned()
            };
            if let Some(completion) = self.queue_suite_proposal(reply) {
                self.review_new_note(title.to_owned(), text, completion, true);
            }
            return;
        }
        let note = args["note"].as_str().unwrap_or_default().to_owned();
        if note.len() > 4096 {
            reply(Err("Note target exceeds limit".into()));
            return;
        }
        let source = self
            .active()
            .map(|d| d.path.clone())
            .unwrap_or_else(|| root.join("Notes.md"));
        let op = name.to_owned();
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<(vault::ResolvedLink, Loaded, Option<Draft>)> {
                let target = vault::resolve_link(&db, &root, &source, &note, true)?;
                let (loaded, draft) =
                    vault::open_with_recovery(&nyx_core::paths::state_dir(), &target.path)?;
                Ok((target, loaded, draft))
            },
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    reply(Err("Notes closed".into()));
                    return;
                };
                let allowed = if matches!(op.as_str(), "notes.vault_append" | "notes.vault_replace")
                {
                    prefs::get().integration_write
                } else {
                    prefs::get().integration_read
                };
                if !allowed {
                    reply(Err("Notes integration permission was revoked".into()));
                    return;
                }
                let (target, loaded, draft) = match result {
                    Ok(v) => v,
                    Err(e) => {
                        reply(Err(format!("{e:#}")));
                        return;
                    }
                };
                match op.as_str() {
                    "notes.vault_read" => {
                        if loaded.text.len() > 128 * 1024 {
                            reply(Err("Note exceeds read limit; search headings or request a smaller note".into()));
                            return;
                        }
                        reply(Ok(
                            json!({"path":loaded.path,"text":loaded.text,"bom":loaded.bom,"version":loaded.version.digest.to_hex().to_string(),"untrusted":true}),
                        ));
                    }
                    "notes.vault_open" => {
                        if let Some(reference) = target.fragment {
                            ui.pending_jumps
                                .borrow_mut()
                                .insert(target.path.clone(), Jump::Reference(reference));
                        }
                        ui.open_file(target.path);
                        reply(Ok(json!({"opened":true})));
                    }
                    "notes.vault_append" | "notes.vault_replace" => {
                        let Some(text) = args["text"].as_str().filter(|s| s.len() <= 128 * 1024)
                        else {
                            reply(Err("Text is required and must fit 128 KiB".into()));
                            return;
                        };
                        if op == "notes.vault_replace"
                            && args["version"].as_str()
                                != Some(loaded.version.digest.to_hex().as_str())
                        {
                            reply(Err("Note version changed; read it again before proposing a replacement".into()));
                            return;
                        }
                        let doc = ui
                            .docs
                            .borrow()
                            .iter()
                            .find(|d| d.path == loaded.path)
                            .cloned();
                        let doc = if let Some(doc) = doc {
                            doc
                        } else {
                            ui.add_document(loaded, draft);
                            ui.active().unwrap()
                        };
                        if doc.read_only || doc.recovery_pending.get() || doc.conflict.get() {
                            reply(Err("Resolve the note's current read-only/recovery/conflict state first".into()));
                            return;
                        }
                        let append = op == "notes.vault_append";
                        if let Some(completion) = ui.queue_suite_proposal(reply) {
                            ui.review_suite_edit(doc, text.to_owned(), append, completion);
                        }
                    }
                    _ => reply(Err("Unknown Notes tool".into())),
                }
            },
        );
    }
    fn queue_suite_proposal(self: &Rc<Self>, reply: Reply) -> Option<Reply> {
        if self
            .suite_proposals
            .borrow()
            .values()
            .filter(|v| v["status"] == "awaiting_review")
            .count()
            >= 8
        {
            reply(Err("Too many note proposals are awaiting review".into()));
            return None;
        }
        let id = vault::canvas::id();
        if self.suite_proposals.borrow().len() > 100 {
            self.suite_proposals
                .borrow_mut()
                .retain(|_, v| v["status"] == "awaiting_review");
        }
        self.suite_proposals.borrow_mut().insert(
            id.clone(),
            json!({"status":"awaiting_review","applied":false}),
        );
        reply(Ok(
            json!({"proposal_id":id,"status":"awaiting_review","applied":false}),
        ));
        let weak = Rc::downgrade(self);
        Some(Box::new(move |result| {
            if let Some(ui) = weak.upgrade() {
                let state = match result {
                    Ok(data) => json!({"status":"applied","applied":true,"result":data}),
                    Err(error) => json!({"status":"not_applied","applied":false,"error":error}),
                };
                ui.suite_proposals.borrow_mut().insert(id, state);
            }
        }))
    }
    fn review_new_note(self: &Rc<Self>, title: String, text: String, reply: Reply, external: bool) {
        let Some(root) = self.root.borrow().clone() else {
            reply(Err("Open a vault first".into()));
            return;
        };
        let action = vault::integrations::LauncherAction {
            api_version: 1,
            op: "create".into(),
            vault: root.clone(),
            path: None,
            title: Some(title.clone()),
            offset: None,
            append: false,
        };
        if let Err(e) = action.validate() {
            reply(Err(e.to_string()));
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Review new note",
            "Review new note",
            &format!(
                "Create {title} in {} only after accepting this preview.",
                root.display()
            ),
            800,
            600,
        );
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("markdown")
                .as_ref(),
        );
        buffer.set_text(&text);
        let view = sourceview5::View::with_buffer(&buffer);
        view.add_css_class("notes-editor");
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        let s = scroll(&view);
        s.set_vexpand(true);
        dialog.content_area().append(&s);
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Create note", gtk::ResponseType::Accept);
        let reply = RefCell::new(Some(reply));
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            let Some(reply) = reply.borrow_mut().take() else {
                return;
            };
            if r != gtk::ResponseType::Accept || (external && !prefs::get().integration_write) {
                reply(Err("Creation cancelled; no note was created".into()));
                d.close();
                return;
            }
            let title = title.clone();
            let root = root.clone();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .to_string();
            let weak = weak.clone();
            worker::run(
                move || -> anyhow::Result<PathBuf> {
                    let filename = if vault::is_markdown(std::path::Path::new(&title)) {
                        title
                    } else {
                        format!("{title}.md")
                    };
                    if root.canonicalize()? != root {
                        anyhow::bail!("Vault location changed during review; open it again");
                    }
                    let path = root.join(filename);
                    vault::save_copy(&path, &text, false)?;
                    Ok(path)
                },
                move |result| {
                    if let Some(ui) = weak.upgrade() {
                        match result {
                            Ok(path) => {
                                ui.populate_root();
                                ui.update_index(path.clone());
                                ui.open_file(path.clone());
                                reply(Ok(json!({"created":path})));
                            }
                            Err(e) => reply(Err(format!("{e:#}"))),
                        }
                    } else {
                        reply(Err(
                            "Notes closed; review the target path before retrying".into()
                        ));
                    }
                },
            );
            d.close();
        });
        dialog.present();
    }
    fn review_suite_edit(self: &Rc<Self>, doc: Document, text: String, append: bool, reply: Reply) {
        let source = doc.editor.text();
        let revision = doc.editor.revision.get();
        let range = if append {
            source.len()..source.len()
        } else {
            0..source.len()
        };
        let proposed = if append { format!("\n{text}") } else { text };
        let previous = if append { String::new() } else { source };
        let dialog = Prompt::new(
            &self.window,
            "Review suite edit",
            "Review proposed note edit",
            "Changes are applied only after you accept this diff. Unsaved work, undo, and conditional saves remain protected.",
            850,
            600,
        );
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("diff")
                .as_ref(),
        );
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_editable(false);
        view.add_css_class("notes-editor");
        let s = scroll(&view);
        s.set_vexpand(true);
        dialog.content_area().append(&s);
        let diff = buffer.downgrade();
        let content = proposed.clone();
        worker::background(
            move || vault::history::compare(&previous, &content).0,
            move |text| {
                if let Some(buffer) = diff.upgrade() {
                    buffer.set_text(&text);
                }
            },
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Apply", gtk::ResponseType::Accept);
        let reply = RefCell::new(Some(reply));
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d,r|{let Some(reply)=reply.borrow_mut().take()else{return;};if r==gtk::ResponseType::Accept{if doc.closed.get()||doc.conflict.get()||doc.recovery_pending.get()||doc.read_only||doc.editor.revision.get()!=revision{reply(Err("The note changed during review; propose the edit again".into()));}else if doc.editor.replace_range(range.clone(),&proposed){if let Some(ui)=weak.upgrade(){let path=doc.path.clone();ui.save(doc.clone(),Some(Box::new(move |ok|{if ok{reply(Ok(json!({"saved":path})));}else{reply(Err("Edit retained in the editor; saving needs conflict/recovery resolution".into()));}})));}else{reply(Err("Notes closed".into()));}}else{reply(Err("Edit refused".into()));}}else{reply(Err("Edit cancelled; source is unchanged".into()));}d.close();});
        dialog.present();
    }
}

fn suite_call(
    bus: &str,
    tool: &str,
    args: Value,
    done: impl FnOnce(Result<Value, String>) + 'static,
) {
    let bus = bus.to_owned();
    let path = format!("/{}", bus.replace('.', "/"));
    let params = (tool.to_owned(), args.to_string()).to_variant();
    gio::bus_get(
        gio::BusType::Session,
        gio::Cancellable::NONE,
        move |result| {
            let connection = match result {
                Ok(c) => c,
                Err(e) => {
                    done(Err(e.to_string()));
                    return;
                }
            };
            connection.call(
                Some(&bus),
                &path,
                nyx_core::control::INTERFACE,
                "Call",
                Some(&params),
                None,
                gio::DBusCallFlags::NO_AUTO_START,
                10000,
                gio::Cancellable::NONE,
                move |result| {
                    let result = (|| {
                        let value =
                            result.map_err(|e| format!("The companion app is unavailable: {e}"))?;
                        let (text,) = value.get::<(String,)>().ok_or("Invalid companion reply")?;
                        let result: Value =
                            serde_json::from_str(&text).map_err(|e| e.to_string())?;
                        if let Some(error) = result.get("error") {
                            return Err(error
                                .as_str()
                                .unwrap_or("Companion request failed")
                                .to_owned());
                        }
                        Ok(result["ok"].clone())
                    })();
                    done(result);
                },
            );
        },
    );
}
impl Ui {
    pub(super) fn integration_controls(self: &Rc<Self>) -> gtk::Box {
        let body = widgets::vbox(6);
        let p = prefs::get();
        for (label, value, key) in [
            (
                "Allow suite tools to search/read the active vault",
                p.integration_read,
                0,
            ),
            (
                "Allow suite tools to propose note edits for review",
                p.integration_write,
                1,
            ),
            (
                "Export visited vaults to the local Glimpse Notes plugin",
                p.glimpse_enabled,
                2,
            ),
        ] {
            let button = gtk::CheckButton::with_label(label);
            button.set_active(value);
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |b| {
                let enabled = b.is_active();
                prefs::update(|p| match key {
                    0 => p.integration_read = enabled,
                    1 => p.integration_write = enabled,
                    _ => p.glimpse_enabled = enabled,
                });
                if let Some(ui) = weak.upgrade() {
                    ui.export_launcher_index();
                }
            });
            body.append(&button);
        }
        body
    }
    pub(super) fn integration_preferences(self: &Rc<Self>) {
        let dialog = Prompt::new(
            &self.window,
            "Suite integrations",
            "Suite integrations",
            "Permissions default to off. Read access is limited to the active vault; proposed edits always need visible review. Glimpse uses an explicitly enabled local index export.",
            600,
            300,
        );
        dialog.content_area().append(&self.integration_controls());
        dialog.add_button("Close", gtk::ResponseType::Close);
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
    pub(super) fn clip_dialog(self: &Rc<Self>) {
        let dialog = Prompt::new(
            &self.window,
            "Save web clipping",
            "Save web clipping",
            "Paste selected text or an article and its source URL. Notes preserves the URL as Markdown metadata and never fetches the site automatically.",
            750,
            580,
        );
        let title = gtk::Entry::builder().placeholder_text("Note title").build();
        let url = gtk::Entry::builder()
            .placeholder_text("Source URL (https://…)")
            .build();
        let text = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        let s = scroll(&text);
        s.set_vexpand(true);
        dialog.content_area().append(&title);
        dialog.content_area().append(&url);
        dialog.content_area().append(&s);
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Review clipping", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let url = url.text();
                if (!url.starts_with("https://") && !url.starts_with("http://"))
                    || url.len() > 4096
                    || url.contains(['\n', '\r', '\0'])
                {
                    ui.message("Enter an HTTP(S) source URL");
                    return;
                }
                let title = title.text().to_string();
                let b = text.buffer();
                let body = b.text(&b.start_iter(), &b.end_iter(), true);
                if body.len() > 128 * 1024 {
                    ui.message("Keep a clipping under 128 KiB");
                    return;
                }
                let date = glib::DateTime::now_local()
                    .and_then(|d| d.format("%Y-%m-%dT%H:%M:%S%z"))
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let source = format!(
                    "---\nsource: {}\ncaptured: {}\n---\n# {title}\n\n{body}\n",
                    serde_json::to_string(url.as_str()).unwrap_or_default(),
                    serde_json::to_string(&date).unwrap_or_default()
                );
                let weak = Rc::downgrade(&ui);
                ui.review_new_note(
                    title,
                    source,
                    Box::new(move |result| {
                        if let (Some(ui), Err(error)) = (weak.upgrade(), result) {
                            ui.message(&error);
                        }
                    }),
                    false,
                );
            }
            d.close();
        });
        dialog.present();
    }
    pub(super) fn capture_insert(self: &Rc<Self>) {
        let Some(doc) = self.active() else {
            self.message("Open the destination note first");
            return;
        };
        if doc.read_only || doc.recovery_pending.get() || doc.conflict.get() {
            self.message("Resolve this note's read-only/recovery/conflict state first");
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Insert from Capture",
            "Insert from Capture",
            "Search Capture's public local history interface. Attach uses Capture's flattened export so annotations and redactions are preserved. OCR and recordings remain ordinary text/files.",
            780,
            540,
        );
        let query = gtk::Entry::builder()
            .placeholder_text("Search stored OCR / captures, then press Enter")
            .build();
        let results = widgets::vbox(6);
        let status = gtk::Label::new(Some(
            "Capture must be running. Companion apps are optional.",
        ));
        status.set_wrap(true);
        dialog.content_area().append(&query);
        let s = scroll(&results);
        s.set_vexpand(true);
        dialog.content_area().append(&s);
        dialog.content_area().append(&status);
        dialog.add_button("Close", gtk::ResponseType::Close);
        let weak = Rc::downgrade(self);
        let request = Rc::new(Cell::new(0u64));
        let search: Rc<dyn Fn()> = Rc::new({
            let query = query.downgrade();
            let results = results.downgrade();
            let status = status.downgrade();
            let request = request.clone();
            move || {
                let (Some(query), Some(status)) = (query.upgrade(), status.upgrade()) else {
                    return;
                };
                let sequence = request.get() + 1;
                request.set(sequence);
                status.set_text("Searching local Capture history…");
                let request = request.clone();
                let results = results.clone();
                let status = status.downgrade();
                let weak = weak.clone();
                let doc = doc.clone();
                suite_call(
                    "io.github.design_nexus.nyx.Capture",
                    "capture.search",
                    json!({"query":query.text().as_str()}),
                    move |result| {
                        if request.get() != sequence {
                            return;
                        }
                        let (Some(ui), Some(results), Some(status)) =
                            (weak.upgrade(), results.upgrade(), status.upgrade())
                        else {
                            return;
                        };
                        clear(&results);
                        match result {
                            Err(error) => status.set_text(&error),
                            Ok(value) => {
                                let rows = value.as_array().cloned().unwrap_or_default();
                                status.set_text(&format!("{} captures", rows.len()));
                                for row in rows {
                                    let Some(id) = row["id"].as_str().map(str::to_owned) else {
                                        continue;
                                    };
                                    let path =
                                        PathBuf::from(row["path"].as_str().unwrap_or_default());
                                    let title = row["title"].as_str().unwrap_or("Capture");
                                    let actions = widgets::hbox(6);
                                    let label = widgets::label(title, "");
                                    label.set_hexpand(true);
                                    label.set_xalign(0.0);
                                    actions.append(&label);
                                    let attach = gtk::Button::with_label("Attach");
                                    let ocr = gtk::Button::with_label("Insert OCR");
                                    actions.append(&attach);
                                    actions.append(&ocr);
                                    results.append(&actions);
                                    let weak = Rc::downgrade(&ui);
                                    let note = doc.path.clone();
                                    let capture_id = id.clone();
                                    attach.connect_clicked(move |_|{let Some(ui)=weak.upgrade()else{return;};let extension=path.extension().and_then(|e|e.to_str()).filter(|s|["mp4","webm","mkv"].contains(s)).unwrap_or("png");let output=nyx_core::paths::cache_dir().join("capture-imports").join(format!("{}.{}",vault::canvas::id(),extension));let dir=output.parent().unwrap().to_owned();let id=capture_id.clone();let note=note.clone();let weak=Rc::downgrade(&ui);worker::run(move ||std::fs::create_dir_all(dir),move |result|{let Some(ui)=weak.upgrade()else{return;};if let Err(e)=result{ui.message(&format!("Capture import: {e}"));return;}let args=json!({"id":id,"path":output,"overwrite":false});let weak=Rc::downgrade(&ui);suite_call("io.github.design_nexus.nyx.Capture","capture.export",args,move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(_)=>{ui.attach_paths(note,vec![output.clone()]);worker::run(move ||std::fs::remove_file(output),|_|{});},Err(e)=>ui.message(&e)}}});});});
                                    let d = doc.clone();
                                    let weak = Rc::downgrade(&ui);
                                    ocr.connect_clicked(move |_|{let d=d.clone();let weak=weak.clone();suite_call("io.github.design_nexus.nyx.Capture","capture.ocr_text",json!({"id":id}),move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(value)=>{if d.closed.get()||d.read_only||d.conflict.get()||d.recovery_pending.get(){ui.message("The destination note is no longer writable");return;}let text=value["text"].as_str().unwrap_or_default();if text.len()>128*1024{ui.message("Stored OCR exceeds the insert limit");return;}d.editor.insert_text(text);},Err(e)=>ui.message(&e)}}});});
                                }
                            }
                        }
                    },
                );
            }
        });
        let callback = search.clone();
        query.connect_activate(move |_| callback());
        search();
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
}
impl Ui {
    pub(super) fn drain_launcher(self: &Rc<Self>) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let all = std::mem::take(&mut *self.pending_launcher.borrow_mut());
        for action in all {
            if action.vault != root {
                self.message(
                    "Launcher vault path changed; refresh the exported index before retrying",
                );
                continue;
            }
            if !prefs::get().glimpse_enabled {
                self.message("The Notes/Glimpse integration is disabled");
                continue;
            }
            if let Err(error) = action.validate() {
                self.message(&error.to_string());
                continue;
            }
            let root = root.clone();
            let weak = Rc::downgrade(self);
            let append = action.append;
            let offset = action.offset;
            worker::run(
                move || -> anyhow::Result<PathBuf> {
                    if root.canonicalize()? != root {
                        anyhow::bail!("Vault location changed");
                    }
                    if action.op == "create" {
                        let title = action.title.unwrap();
                        let filename = if vault::is_markdown(std::path::Path::new(&title)) {
                            title.clone()
                        } else {
                            format!("{title}.md")
                        };
                        let path = root.join(filename);
                        vault::save_copy(&path, &format!("# {title}\n\n"), false)?;
                        Ok(path)
                    } else {
                        let path = action.path.unwrap();
                        let canonical = path.canonicalize()?;
                        if !canonical.starts_with(&root) {
                            anyhow::bail!("Launcher note points outside the vault");
                        }
                        Ok(path)
                    }
                },
                move |result| {
                    if let Some(ui) = weak.upgrade() {
                        match result {
                            Ok(path) => {
                                if !prefs::get().glimpse_enabled {
                                    return;
                                }
                                if append {
                                    ui.reading.set_active(false);
                                    ui.pending_jumps
                                        .borrow_mut()
                                        .insert(path.clone(), Jump::Offset(usize::MAX));
                                } else if let Some(offset) = offset {
                                    ui.pending_jumps
                                        .borrow_mut()
                                        .insert(path.clone(), Jump::Offset(offset));
                                }
                                ui.open_file(path.clone());
                                if let Some(doc) =
                                    ui.docs.borrow().iter().find(|d| d.path == path).cloned()
                                {
                                    ui.apply_pending_jump(&doc);
                                }
                                ui.populate_root();
                                ui.update_index(path);
                            }
                            Err(error) => ui.message(&format!("Notes launcher: {error:#}")),
                        }
                    }
                },
            );
        }
    }
}
