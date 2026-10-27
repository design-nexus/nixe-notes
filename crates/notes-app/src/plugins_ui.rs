//! Native extension management. Installation never grants execution or note access implicitly.
use super::*;
use notes_plugins::{Grant, Package};
fn storage() -> PathBuf {
    nyx_core::paths::data_dir().join("extensions")
}
fn grants() -> Vec<Grant> {
    prefs::get()
        .plugin_grants
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect()
}
fn granted(package: &Package) -> Option<Grant> {
    grants()
        .into_iter()
        .find(|g| g.id == package.manifest.id && g.digest == package.digest && g.enabled)
}
fn save_grant(grant: Grant) {
    prefs::update(|p| {
        p.plugin_grants
            .retain(|v| v["id"].as_str() != Some(grant.id.as_str()));
        if let Ok(value) = serde_json::to_value(&grant) {
            p.plugin_grants.push(value);
        }
    });
}
impl Ui {
    pub(super) fn plugins_menu(self: &Rc<Self>, menu: &gtk::Box) {
        for (label, run) in [("Extension commands…", true), ("Extensions…", false)] {
            let b = gtk::Button::with_label(label);
            b.set_has_frame(false);
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |b| {
                if let Some(p) = b
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    p.popdown();
                }
                if let Some(ui) = weak.upgrade() {
                    if run {
                        ui.extension_commands();
                    } else {
                        ui.extensions();
                    }
                }
            });
            menu.append(&b);
        }
    }
    pub(super) fn extensions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        worker::extension(
            move || notes_plugins::discover(&storage()),
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                let packages = match result {
                    Ok(p) => p,
                    Err(e) => {
                        ui.message(&format!("Extensions: {e:#}"));
                        return;
                    }
                };
                let dialog = Prompt::new(
                    &ui.window,
                    "Extensions",
                    "Extensions",
                    "WASM processors have no filesystem, network, process, or suite access. Approved commands may receive the selected text and propose reviewed edits. Installing or updating never enables a new package automatically.",
                    760,
                    560,
                );
                let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
                if packages.is_empty() {
                    list.append(&widgets::label("No extensions installed", "caption"));
                }
                for package in packages {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let enabled = granted(&package).is_some();
                    let label = gtk::Label::new(Some(&format!(
                        "{} {} · {} · {}",
                        package.manifest.name,
                        package.manifest.version,
                        &package.digest[..12],
                        if enabled { "Enabled" } else { "Disabled" }
                    )));
                    label.set_hexpand(true);
                    label.set_xalign(0.0);
                    label.set_wrap(true);
                    row.append(&label);
                    let review = gtk::Button::with_label("Review permissions…");
                    let p = package.clone();
                    let weak = Rc::downgrade(&ui);
                    review.connect_clicked(move |_| {
                        if let Some(ui) = weak.upgrade() {
                            ui.extension_approval(p.clone());
                        }
                    });
                    row.append(&review);
                    let disable = gtk::Button::with_label("Disable");
                    let id = package.manifest.id.clone();
                    let weak = Rc::downgrade(&ui);
                    let label = label.downgrade();
                    disable.connect_clicked(move |_| {
                        prefs::update(|p| {
                            for value in &mut p.plugin_grants {
                                if value["id"].as_str() == Some(&id) {
                                    value["enabled"] = false.into();
                                }
                            }
                        });
                        if let Some(label) = label.upgrade() {
                            label.set_text(&format!("{id} · Disabled"));
                        }
                        if let Some(ui) = weak.upgrade() {
                            ui.message(
                                "Extension disabled; pending unapproved edits cannot be applied",
                            );
                        }
                    });
                    row.append(&disable);
                    list.append(&row);
                }
                let s = scroll(&list);
                s.set_vexpand(true);
                dialog.content_area().append(&s);
                dialog.add_button("Close", gtk::ResponseType::Close);
                dialog.add_button("Install manifest…", gtk::ResponseType::Other(1));
                let weak = Rc::downgrade(&ui);
                dialog.connect_response(move |d, r| {
                    if r == gtk::ResponseType::Other(1)
                        && let Some(ui) = weak.upgrade()
                    {
                        ui.install_extension();
                    }
                    d.close();
                });
                dialog.present();
            },
        );
    }
    fn install_extension(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Extension manifest (.json)"));
        filter.add_pattern("*.json");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let picker = gtk::FileDialog::builder()
            .title("Install extension manifest")
            .filters(&filters)
            .build();
        let weak = Rc::downgrade(self);
        picker.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let weak = Rc::downgrade(&ui);
                worker::extension(
                    move || Package::install(&path, &storage()),
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(package) => ui.extension_approval(package),
                                Err(e) => ui.message(&format!("Extension installation: {e:#}")),
                            }
                        }
                    },
                );
            }
        });
    }
    fn extension_approval(self: &Rc<Self>, package: Package) {
        let dialog = Prompt::new(
            &self.window,
            "Extension permissions",
            &package.manifest.name,
            &format!(
                "Version {} · API {}\nPackage {}\nGrant only the capabilities you want. Each text edit still requires a separate diff review.",
                package.manifest.version, package.manifest.api_version, package.digest
            ),
            680,
            420,
        );
        let mut choices = Vec::new();
        for permission in &package.manifest.permissions {
            let explanation = match permission.as_str() {
                "selection.read" => {
                    "Read the selected passage (or the active note when explicitly chosen)"
                }
                "edit.propose" => "Return text for review as a replacement of that passage",
                _ => "Unsupported capability",
            };
            let check = gtk::CheckButton::with_label(explanation);
            check.set_active(false);
            dialog.content_area().append(&check);
            choices.push((permission.clone(), check));
        }
        if choices.is_empty() {
            dialog.content_area().append(&widgets::label(
                "This extension requests no note access",
                "caption",
            ));
        }
        dialog.add_button("Keep disabled", gtk::ResponseType::Cancel);
        dialog.add_button("Enable with checked permissions", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d,r|{if r==gtk::ResponseType::Accept{let permissions=choices.iter().filter(|(_,c)|c.is_active()).map(|(p,_)|p.clone()).collect::<Vec<_>>();if !package.manifest.permissions.iter().all(|p|permissions.contains(p)){if let Some(ui)=weak.upgrade(){ui.message("Required capabilities were not granted; the extension remains disabled");}return;}save_grant(Grant{id:package.manifest.id.clone(),digest:package.digest.clone(),enabled:true,permissions});if let Some(ui)=weak.upgrade(){ui.message("Extension enabled. Its commands are in File → Extension commands.");}}d.close();});
        dialog.present();
    }
    pub(super) fn extension_commands(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        worker::extension(
            move || notes_plugins::discover(&storage()),
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match result {
                    Err(e) => ui.message(&format!("Extension commands: {e:#}")),
                    Ok(packages) => {
                        let dialog = Prompt::new(
                            &ui.window,
                            "Extension commands",
                            "Extension commands",
                            "Commands run in an isolated WASM interpreter. The active passage is sent only after you choose a command and confirm it.",
                            700,
                            480,
                        );
                        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
                        let mut count = 0;
                        for package in packages.into_iter().filter(|p| granted(p).is_some()) {
                            for command in &package.manifest.commands {
                                count += 1;
                                let b = gtk::Button::with_label(&format!(
                                    "{} · {}",
                                    package.manifest.name, command.title
                                ));
                                let p = package.clone();
                                let command = command.id.clone();
                                let weak = Rc::downgrade(&ui);
                                b.connect_clicked(move |_| {
                                    if let Some(ui) = weak.upgrade() {
                                        ui.run_extension(p.clone(), command.clone());
                                    }
                                });
                                list.append(&b);
                            }
                        }
                        if count == 0 {
                            list.append(&widgets::label("No enabled extension commands. Install and approve an extension first.","caption"));
                        }
                        let s = scroll(&list);
                        s.set_vexpand(true);
                        dialog.content_area().append(&s);
                        dialog.add_button("Close", gtk::ResponseType::Close);
                        dialog.connect_response(|d, _| d.close());
                        dialog.present();
                    }
                }
            },
        );
    }
    fn run_extension(self: &Rc<Self>, package: Package, command: String) {
        let Some(doc) = self.active() else {
            self.message("Open a note first");
            return;
        };
        if doc.read_only || doc.conflict.get() || doc.recovery_pending.get() {
            self.message("Resolve the document state before running a processor");
            return;
        }
        let source = doc.editor.text();
        let byte = |n: i32| {
            source
                .char_indices()
                .nth(n as usize)
                .map_or(source.len(), |(i, _)| i)
        };
        let selected = doc.editor.buffer.selection_bounds();
        let range = selected
            .map(|(a, b)| byte(a.offset())..byte(b.offset()))
            .unwrap_or(0..source.len());
        let previous = source[range.clone()].to_owned();
        if previous.len() > notes_plugins::MAX_TEXT {
            self.message("Select a passage below 128 KiB for the extension");
            return;
        }
        let revision = doc.editor.revision.get();
        let reads = package
            .manifest
            .permissions
            .iter()
            .any(|p| p == "selection.read");
        let dialog = Prompt::new(
            &self.window,
            "Run extension command",
            &package.manifest.name,
            &format!(
                "Command: {command}\n{} will be shared. Proposed edits require another review before they can change the note.",
                if reads {
                    format!(
                        "{} bytes from {}",
                        previous.len(),
                        if selected.is_some() {
                            "the selected passage"
                        } else {
                            "the entire active note"
                        }
                    )
                } else {
                    "No note content".into()
                }
            ),
            640,
            300,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Run command", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            if r != gtk::ResponseType::Accept {
                d.close();
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(grant) = granted(&package) else {
                ui.message("Extension is no longer enabled");
                d.close();
                return;
            };
            if ui.extension_busy.replace(true) {
                ui.message("An extension command is already running");
                return;
            }
            let p = package.clone();
            let command = command.clone();
            let input = previous.clone();
            let doc = doc.clone();
            let previous = previous.clone();
            let range = range.clone();
            let weak = Rc::downgrade(&ui);
            let approved = package.clone();
            ui.message("Running isolated extension command…");
            worker::extension(
                move || p.run(&grant, &command, &input),
                move |result| {
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    ui.extension_busy.set(false);
                    if granted(&approved).is_none() {
                        ui.message("Extension was disabled; its result was discarded");
                        return;
                    }
                    match result {
                        Err(e) => ui.message(&format!("Extension stopped: {e:#}")),
                        Ok(output) => {
                            if let Some(message) = output.message {
                                ui.message(&message);
                            }
                            if let Some(text) = output.text {
                                let guard: Rc<dyn Fn() -> bool> =
                                    Rc::new(move || granted(&approved).is_some());
                                ui.proposal_guarded(doc, revision, range, previous, text, guard);
                            }
                        }
                    }
                },
            );
            d.close();
        });
        dialog.present();
    }
}
