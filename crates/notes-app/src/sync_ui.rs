//! Optional personal sync controls. No network or keyring operations run on GTK.
use super::*;
use notes_sync::{Config, Conflict, Engine, Resolution, WebDav};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub(super) struct SyncState {
    busy: Cell<bool>,
    online: RefCell<Vec<Config>>,
    discovered: Cell<u64>,
    discovering: Cell<bool>,
    pub(super) cancel: Arc<AtomicBool>,
    attempt: Cell<u32>,
    next: Cell<u64>,
    status: RefCell<String>,
    observers: RefCell<Vec<glib::WeakRef<gtk::Label>>>,
}
impl Default for SyncState {
    fn default() -> Self {
        Self {
            busy: Cell::new(false),
            online: RefCell::new(
                prefs::get()
                    .online_sync_profiles
                    .into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect(),
            ),
            discovered: Cell::new(0),
            discovering: Cell::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            attempt: Cell::new(0),
            next: Cell::new(0),
            status: RefCell::new(String::new()),
            observers: RefCell::new(Vec::new()),
        }
    }
}
impl SyncState {
    fn status(&self, text: &str) {
        *self.status.borrow_mut() = text.into();
        self.observers.borrow_mut().retain(|w| {
            if let Some(label) = w.upgrade() {
                label.set_text(text);
                true
            } else {
                false
            }
        });
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn profile(root: &std::path::Path) -> Option<Config> {
    prefs::get()
        .sync_profiles
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Config>(v).ok())
        .find(|c| c.root == root)
}
fn remote(config: &Config) -> anyhow::Result<Box<dyn notes_sync::Transport>> {
    if let Some(id) = config.account_id.as_deref() {
        return Ok(Box::new(notes_sync::CloudRemote::connect(id)?));
    }
    let password = notes_sync::lookup_secret("nyx-notes", &config.id(), "webdav")?;
    Ok(Box::new(WebDav::new(config, password)?))
}
impl Ui {
    fn sync_profile(&self, root: &std::path::Path) -> Option<Config> {
        self.synchronization
            .online
            .borrow()
            .iter()
            .find(|c| c.root == root)
            .cloned()
            .or_else(|| profile(root))
    }
    pub(super) fn sync_menu(self: &Rc<Self>, menu: &gtk::Box) {
        for (label, action) in [
            ("Personal sync…", 0),
            ("Sync now", 1),
            ("Sync conflicts…", 2),
            ("Sync recovery versions…", 3),
            ("Cancel sync", 4),
        ] {
            let button = gtk::Button::with_label(label);
            button.set_has_frame(false);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |button| {
                if let Some(p) = button
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    p.popdown();
                }
                if let Some(ui) = weak.upgrade() {
                    match action {
                        0 => ui.sync_preferences(),
                        1 => ui.sync_now(true),
                        2 => ui.sync_conflicts(),
                        3 => ui.sync_versions(),
                        _ => {
                            ui.synchronization.cancel.store(true, Ordering::Relaxed);
                            ui.synchronization
                                .status("Cancelling after the current request…");
                        }
                    }
                }
            });
            menu.append(&button);
        }
    }
    pub(super) fn sync_preferences(self: &Rc<Self>) {
        let Some(root) = self.root.borrow().clone() else {
            self.message("Open a vault first");
            return;
        };
        let config = self.sync_profile(&root).unwrap_or(Config {
            root: root.clone(),
            ..Default::default()
        });
        if config.account_id.is_some() {
            let _ = gio::Subprocess::newv(
                &[
                    std::ffi::OsStr::new("nyx-settings"),
                    std::ffi::OsStr::new("--section"),
                    std::ffi::OsStr::new("online-accounts"),
                ],
                gio::SubprocessFlags::NONE,
            );
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Personal sync",
            "Personal sync",
            "Synchronize this vault with your own Nextcloud/WebDAV collection. Files remain ordinary Markdown and attachments. Deletions and divergent versions require review.",
            720,
            560,
        );
        let accounts = gtk::Button::with_label("Online Accounts…");
        accounts.connect_clicked(|_| {
            let _ = gio::Subprocess::newv(
                &[
                    std::ffi::OsStr::new("nyx-settings"),
                    std::ffi::OsStr::new("--section"),
                    std::ffi::OsStr::new("online-accounts"),
                ],
                gio::SubprocessFlags::NONE,
            );
        });
        dialog.content_area().append(&accounts);
        let endpoint = gtk::Entry::builder()
            .placeholder_text("https://server/remote.php/dav/files/user/Notes/")
            .text(&config.endpoint)
            .build();
        let username = gtk::Entry::builder()
            .placeholder_text("Username")
            .text(&config.username)
            .build();
        let password = gtk::PasswordEntry::builder()
            .placeholder_text("App password · leave blank to keep saved password")
            .show_peek_icon(true)
            .build();
        let excluded = gtk::Entry::builder()
            .placeholder_text("Private; Large Recordings")
            .text(config.excluded.join("; "))
            .build();
        let enabled =
            gtk::CheckButton::with_label("Enable background synchronization for this vault");
        enabled.set_active(config.enabled);
        for (label, entry) in [
            (
                "Dedicated vault collection URL",
                endpoint.upcast_ref::<gtk::Widget>(),
            ),
            ("Username", username.upcast_ref()),
            (
                "Password · stored in desktop keyring",
                password.upcast_ref(),
            ),
            (
                "Excluded relative folders/files · separated by semicolons",
                excluded.upcast_ref(),
            ),
        ] {
            dialog
                .content_area()
                .append(&widgets::label(label, "caption"));
            dialog.content_area().append(entry);
        }
        dialog.content_area().append(&enabled);
        let status = gtk::Label::new(Some(&self.synchronization.status.borrow()));
        status.set_wrap(true);
        status.set_xalign(0.0);
        self.synchronization
            .observers
            .borrow_mut()
            .push(status.downgrade());
        dialog.content_area().append(&status);
        let pause = gtk::Button::with_label("Disable sync immediately");
        let weak = Rc::downgrade(self);
        let pause_root = root.clone();
        pause.connect_clicked(move |_| {
            let root = pause_root.clone();
            prefs::update(|p| {
                for value in &mut p.sync_profiles {
                    if value["root"].as_str() == root.to_str() {
                        value["enabled"] = false.into();
                    }
                }
            });
            if let Some(ui) = weak.upgrade() {
                ui.synchronization.cancel.store(true, Ordering::Relaxed);
                ui.synchronization
                    .status("Sync disabled; local writing remains available");
            }
        });
        dialog.content_area().append(&pause);
        dialog.add_button("Close", gtk::ResponseType::Close);
        dialog.add_button("Create remote collection", gtk::ResponseType::Other(1));
        dialog.add_button("Save settings", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d,response|{
            if !matches!(response,gtk::ResponseType::Accept|gtk::ResponseType::Other(1)){d.close();return;}
            let Some(ui)=weak.upgrade()else{return;};if ui.synchronization.busy.get(){ui.message("Cancel the active sync before changing its account");return;}
            let config=Config{account_id:None,root:root.clone(),endpoint:endpoint.text().trim().into(),username:username.text().trim().into(),enabled:enabled.is_active(),excluded:excluded.text().split(';').map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned).collect()};
            let secret=password.text().to_string();password.set_text("");let output=status.downgrade();let weak=Rc::downgrade(&ui);let create=response==gtk::ResponseType::Other(1);
            status.set_text(if create{"Creating collection…"}else{"Saving account in the desktop keyring…"});
            worker::synchronization(move ||->anyhow::Result<Config>{config.validate()?;if !secret.is_empty(){notes_sync::store_secret("nyx-notes",&config.id(),"webdav",&notes_sync::Secret::new(secret)?)?;}
if create{WebDav::new(&config,notes_sync::lookup_secret("nyx-notes",&config.id(),"webdav")?)?.create_collection()?;}else if config.enabled{let _=notes_sync::lookup_secret("nyx-notes",&config.id(),"webdav")?;}Ok(config)},move |result|{if let Some(label)=output.upgrade(){match result{Ok(config)=>{if !create{prefs::update(|p|{p.sync_profiles.retain(|v|v["root"].as_str()!=config.root.to_str());if let Ok(value)=serde_json::to_value(&config){p.sync_profiles.push(value);}});}label.set_text(if create{"Remote collection created. Save settings to enable sync."}else{"Settings saved. Background sync is optional; conflicts remain reviewable."});if let Some(ui)=weak.upgrade(){ui.synchronization.next.set(0);if !create{ui.sync_now(false);}}},Err(e)=>label.set_text(&format!("Sync settings: {e:#}"))}}});
        });
        dialog.present();
    }
    pub(super) fn sync_now(self: &Rc<Self>, manual: bool) {
        let Some(root) = self.root.borrow().clone() else {
            if manual {
                self.message("Open a vault first");
            }
            return;
        };
        if self.synchronization.discovering.get() {
            return;
        }
        if now().saturating_sub(self.synchronization.discovered.get()) >= 10 {
            self.synchronization.discovering.set(true);
            self.synchronization.discovered.set(now());
            let weak = Rc::downgrade(self);
            worker::run(notes_sync::online_profiles, move |result| {
                if let Some(ui) = weak.upgrade() {
                    ui.synchronization.discovering.set(false);
                    if let Ok(mut profiles) = result {
                        for old in ui.synchronization.online.borrow().iter() {
                            if !profiles.iter().any(|p| p.root == old.root) {
                                let mut disabled = old.clone();
                                disabled.enabled = false;
                                profiles.push(disabled);
                            }
                        }
                        let saved = profiles
                            .iter()
                            .filter_map(|p| serde_json::to_value(p).ok())
                            .collect::<Vec<_>>();
                        prefs::update(|p| p.online_sync_profiles = saved);
                        ui.synchronization.online.replace(profiles);
                    }
                    ui.sync_now(manual);
                }
            });
            return;
        }
        let Some(config) = self.sync_profile(&root).filter(|c| c.enabled) else {
            if manual {
                self.sync_preferences();
            }
            return;
        };
        if self.synchronization.busy.get() {
            if manual {
                self.message("Synchronization is already running");
            }
            return;
        }
        if !manual && now() < self.synchronization.next.get() {
            return;
        }
        self.synchronization.busy.set(true);
        self.synchronization.cancel.store(false, Ordering::Relaxed);
        self.synchronization
            .status("Synchronizing… local editing remains available");
        let blocked = self
            .docs
            .borrow()
            .iter()
            .filter(|d| {
                d.dirty.get() || d.saving.get() || d.conflict.get() || d.recovery_pending.get()
            })
            .map(|d| d.path.clone())
            .chain(
                self.canvases
                    .borrow()
                    .iter()
                    .filter(|(_, (_, dirty))| dirty.get())
                    .map(|(p, _)| p.clone()),
            )
            .collect();
        let cancel = self.synchronization.cancel.clone();
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        let current_root = root.clone();
        worker::synchronization(
            move || {
                config.validate()?;
                Engine::open(&state, config.clone())?.run(&mut remote(&config)?, &blocked, &cancel)
            },
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                ui.synchronization.busy.set(false);
                match result {
                    Ok(report) => {
                        ui.synchronization.attempt.set(0);
                        ui.synchronization.next.set(now() + 120);
                        ui.synchronization.status(&format!("Sync: {report}"));
                        if ui.root.borrow().as_ref() == Some(&current_root) {
                            ui.populate_root();
                            ui.rebuild();
                        }
                        if manual || report.conflicts > 0 {
                            ui.message(&format!(
                                "Sync: {report}. Review conflicts through File → Sync conflicts."
                            ));
                        }
                    }
                    Err(e) => {
                        let attempt = ui.synchronization.attempt.get().saturating_add(1);
                        ui.synchronization.attempt.set(attempt);
                        let seconds = notes_sync::delay(attempt, None).wait.as_secs();
                        ui.synchronization.next.set(now() + seconds);
                        ui.synchronization.status(&format!("Sync paused: {e:#}. Retry in {seconds} seconds while enabled; local files are available."));
                        if manual {
                            ui.message(&ui.synchronization.status.borrow());
                        }
                    }
                }
            },
        );
    }
    pub(super) fn sync_conflicts(self: &Rc<Self>) {
        let Some(config) = self
            .root
            .borrow()
            .as_ref()
            .and_then(|r| self.sync_profile(r))
        else {
            self.message("Configure personal sync first");
            return;
        };
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        let c = config.clone();
        worker::run(
            move || Engine::open(&state, c)?.conflict_paths(),
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match result {
                    Err(e) => ui.message(&format!("Sync conflicts: {e:#}")),
                    Ok(paths) => {
                        if paths.is_empty() {
                            ui.message("No recorded sync conflicts");
                            return;
                        }
                        let dialog = Prompt::new(
                            &ui.window,
                            "Sync conflicts",
                            "Sync conflicts",
                            "Choose a file to compare versions. No deletion or conflicting edit is applied automatically.",
                            700,
                            500,
                        );
                        let list = gtk::ListBox::new();
                        list.set_selection_mode(gtk::SelectionMode::None);
                        for path in paths {
                            let b = gtk::Button::with_label(&path);
                            b.set_has_frame(false);
                            let c = config.clone();
                            let weak = Rc::downgrade(&ui);
                            b.connect_clicked(move |_| {
                                if let Some(ui) = weak.upgrade() {
                                    ui.sync_review(c.clone(), path.clone());
                                }
                            });
                            list.append(&b);
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
    fn sync_review(self: &Rc<Self>, config: Config, path: String) {
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        let c = config.clone();
        worker::run(
            move || {
                let e = Engine::open(&state, c)?;
                let conflict = e.conflict_for(&path)?;
                let display = match (&conflict.local, &conflict.remote) {
                    (Some(a), Some(b)) if a.len() + b.len() < 512 * 1024 => {
                        match (std::str::from_utf8(a), std::str::from_utf8(b)) {
                            (Ok(a), Ok(b)) => vault::history::compare(a, b).0,
                            _ => format!(
                                "Binary versions: local {} bytes; remote {} bytes",
                                a.len(),
                                b.len()
                            ),
                        }
                    }
                    (a, b) => format!(
                        "Local: {}\nRemote: {}\nLarge or binary data is retained without rendering.",
                        a.as_ref()
                            .map_or("Deleted".into(), |b| format!("{} bytes", b.len())),
                        b.as_ref()
                            .map_or("Deleted".into(), |b| format!("{} bytes", b.len()))
                    ),
                };
                Ok::<_, anyhow::Error>((conflict, display))
            },
            move |result| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                match result {
                    Err(e) => ui.message(&format!("Sync conflict: {e:#}")),
                    Ok((conflict, text)) => ui.sync_conflict_dialog(config, conflict, text),
                }
            },
        );
    }
    fn sync_conflict_dialog(self: &Rc<Self>, config: Config, conflict: Conflict, text: String) {
        let dialog = Prompt::new(
            &self.window,
            "Resolve sync conflict",
            &conflict.path,
            "The diff shows local → remote. Choose which reviewed version to retain. Choosing a deleted version deletes the other copy; recovery bytes remain in local sync history.",
            860,
            620,
        );
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("diff")
                .as_ref(),
        );
        buffer.set_text(&text);
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_editable(false);
        let s = scroll(&view);
        s.set_vexpand(true);
        dialog.content_area().append(&s);
        dialog.add_button("Later", gtk::ResponseType::Cancel);
        dialog.add_button(
            if conflict.local.is_none() {
                "Apply local deletion"
            } else {
                "Keep local"
            },
            gtk::ResponseType::Other(1),
        );
        dialog.add_button(
            if conflict.remote.is_none() {
                "Apply remote deletion"
            } else {
                "Use remote"
            },
            gtk::ResponseType::Other(2),
        );
        if conflict.local.is_some() && conflict.remote.is_some() {
            dialog.add_button("Keep both", gtk::ResponseType::Other(3));
        }
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d,response|{let choice=match response{gtk::ResponseType::Other(1)=>Resolution::Local,gtk::ResponseType::Other(2)=>Resolution::Remote,gtk::ResponseType::Other(3)=>Resolution::Both,_=>{d.close();return;}};let Some(ui)=weak.upgrade()else{return;};if ui.synchronization.busy.get(){ui.message("Wait for the current sync to finish before resolving");return;}let target=config.root.join(&conflict.path);if ui.docs.borrow().iter().any(|doc|doc.path==target&&(doc.dirty.get()||doc.saving.get()||doc.recovery_pending.get()||doc.conflict.get()))||ui.canvases.borrow().get(&target).is_some_and(|(_,dirty)|dirty.get()){ui.message("Save or resolve the open document before resolving its sync conflict");return;}
            ui.synchronization.busy.set(true);let c=config.clone();let conflict=conflict.clone();let state=nyx_core::paths::state_dir();let weak=Rc::downgrade(&ui);worker::synchronization(move ||Engine::open(&state,c.clone())?.resolve(&mut remote(&c)?,&conflict,choice),move |result|{if let Some(ui)=weak.upgrade(){ui.synchronization.busy.set(false);match result{Ok(())=>{ui.message("Reviewed sync resolution applied; previous bytes remain in sync recovery history");ui.populate_root();ui.rebuild();ui.synchronization.next.set(0);},Err(e)=>ui.message(&format!("Resolution stopped: {e:#}. Synchronize again for a fresh comparison."))}}});d.close();});
        dialog.present();
    }
    pub(super) fn sync_versions(self: &Rc<Self>) {
        let Some(config) = self
            .root
            .borrow()
            .as_ref()
            .and_then(|r| self.sync_profile(r))
        else {
            self.message("Configure personal sync first");
            return;
        };
        let dialog = Prompt::new(
            &self.window,
            "Sync recovery versions",
            "Sync recovery versions",
            "Enter a relative note or attachment path. Recovery exports never replace an existing file.",
            700,
            480,
        );
        let path = gtk::Entry::builder()
            .placeholder_text("Attachments/image.png")
            .build();
        if let Some(doc) = self.active()
            && let Ok(relative) = doc.path.strip_prefix(&config.root)
        {
            path.set_text(&relative.to_string_lossy());
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let s = scroll(&list);
        s.set_vexpand(true);
        dialog.content_area().append(&path);
        dialog.content_area().append(&s);
        let find = gtk::Button::with_label("Find recovery versions");
        dialog.content_area().append(&find);
        let weak = Rc::downgrade(self);
        find.connect_clicked(move |_|{let c=config.clone();let path=path.text().to_string();let state=nyx_core::paths::state_dir();let weak=weak.clone();let list=list.downgrade();worker::run(move ||Engine::open(&state,c.clone())?.versions(&path).map(|v|(c,v)),move |result|{let(Some(ui),Some(list))=(weak.upgrade(),list.upgrade())else{return;};clear(&list);match result{Err(e)=>ui.message(&format!("Sync history: {e:#}")),Ok((config,versions))=>{if versions.is_empty(){list.append(&widgets::label("No retained versions for this path","caption"));}for(id,created,size)in versions{let b=gtk::Button::with_label(&format!("Version {id} · UTC timestamp {created} · {size} bytes · Export…"));let c=config.clone();let weak=Rc::downgrade(&ui);b.connect_clicked(move |_|{let Some(ui)=weak.upgrade()else{return;};let picker=gtk::FileDialog::builder().title("Export recovery version").initial_name(format!("recovered-{id}")).build();let c=c.clone();let weak=Rc::downgrade(&ui);picker.save(Some(&ui.window),gio::Cancellable::NONE,move |result|{if let Ok(file)=result&&let Some(target)=file.path(){let state=nyx_core::paths::state_dir();worker::run(move ||Engine::open(&state,c)?.export_version(id,&target),move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(())=>ui.message("Recovery copy exported"),Err(e)=>ui.message(&format!("Recovery export: {e:#}"))}}});}});});list.append(&b);}}}});});
        dialog.add_button("Close", gtk::ResponseType::Close);
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
}
