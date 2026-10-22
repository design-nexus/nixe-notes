//! First-run connection and explicit local paths also work without a file-picker portal.
use super::*;
fn entered_path(text: &str) -> anyhow::Result<PathBuf> {
    let value = text.trim();
    anyhow::ensure!(!value.is_empty(), "Choose a folder or enter its full path");
    let path = if value == "~" {
        glib::home_dir()
    } else if let Some(tail) = value.strip_prefix("~/") {
        glib::home_dir().join(tail)
    } else {
        PathBuf::from(value)
    };
    anyhow::ensure!(
        path.is_absolute(),
        "Enter an absolute folder path, for example ~/Documents/Notes"
    );
    Ok(path)
}
fn dismissed(error: &glib::Error) -> bool {
    error.matches(gtk::DialogError::Dismissed)
        || error.matches(gtk::DialogError::Cancelled)
        || error.matches(gio::IOErrorEnum::Cancelled)
}
impl Ui {
    pub(super) fn setup_message(&self, text: &str) {
        if let Some(label) = self
            .setup_status
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        {
            label.set_text(text);
        }
    }
    pub(super) fn finish_vault_setup(&self) {
        let window = self
            .setup_window
            .borrow_mut()
            .take()
            .and_then(|w| w.upgrade());
        self.setup_status.borrow_mut().take();
        if let Some(window) = window {
            window.destroy();
        }
    }
    pub(super) fn choose_vault(self: &Rc<Self>) {
        if let Some(window) = self
            .setup_window
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
        {
            window.present();
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Connect a vault",
            "Connect your notes",
            "A vault is simply a folder of Markdown files. Choose your existing Obsidian vault folder (not its .obsidian folder), or create a new folder. Nothing is imported or converted.",
            620,
            420,
        );
        *self.setup_window.borrow_mut() = Some(dialog.window.downgrade());
        let path = gtk::Entry::builder()
            .placeholder_text("~/Documents/Notes")
            .hexpand(true)
            .activates_default(true)
            .build();
        path.set_widget_name("vault-path");
        if let Some(root) = self
            .root
            .borrow()
            .clone()
            .or_else(|| prefs::get().last_vault)
        {
            path.set_text(&root.to_string_lossy());
        }
        dialog
            .content_area()
            .append(&widgets::label("Vault folder", "caption"));
        let row = widgets::hbox(8);
        row.append(&path);
        let browse = gtk::Button::with_label("Browse…");
        row.append(&browse);
        dialog.content_area().append(&row);
        let files = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
        let status = gtk::Label::new(Some(
            "Choose an existing folder, or enter a new folder path and select Create folder.",
        ));
        status.set_wrap(true);
        status.set_xalign(0.0);
        dialog.content_area().append(&status);
        *self.setup_status.borrow_mut() = Some(status.downgrade());
        let markdown = gtk::Button::with_label("Open a Markdown file instead…");
        dialog.content_area().append(&markdown);
        let weak = Rc::downgrade(self);
        let entry = path.downgrade();
        let parent = dialog.window.downgrade();
        let selected = files.clone();
        browse.connect_clicked(move |_|{
            let(Some(ui),Some(parent))=(weak.upgrade(),parent.upgrade())else{return;};
            let picker=gtk::FileDialog::builder().title("Choose your vault folder").build();let weak=Rc::downgrade(&ui);let entry=entry.clone();let selected=selected.clone();
            picker.select_folder(Some(&parent),gio::Cancellable::NONE,move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(file)=>{if let(Some(folder),Some(entry))=(file.path(),entry.upgrade()){selected.borrow_mut().clear();entry.set_text(&folder.to_string_lossy());ui.setup_message("Folder selected. Choose Connect to open it.");}else{ui.setup_message("Choose a local folder; remote locations must first be mounted locally.");}},Err(e)if !dismissed(&e)=>ui.setup_message(&format!("Folder picker: {e}. You can enter the folder path above instead.")),_=>{}}}});
        });
        let weak = Rc::downgrade(self);
        let parent = dialog.window.downgrade();
        let entry = path.downgrade();
        let selected = files.clone();
        markdown.connect_clicked(move |_|{
            let(Some(ui),Some(parent))=(weak.upgrade(),parent.upgrade())else{return;};let filter=gtk::FileFilter::new();filter.set_name(Some("Markdown files"));for pattern in ["*.md","*.markdown","*.MD"]{filter.add_pattern(pattern);}let filters=gio::ListStore::new::<gtk::FileFilter>();filters.append(&filter);
            let picker=gtk::FileDialog::builder().title("Open Markdown file").filters(&filters).build();let weak=Rc::downgrade(&ui);let entry=entry.clone();let selected=selected.clone();
            picker.open(Some(&parent),gio::Cancellable::NONE,move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(file)=>{if let Some(file)=file.path()&&let Some(folder)=file.parent()&&let Some(entry)=entry.upgrade(){entry.set_text(&folder.to_string_lossy());*selected.borrow_mut()=vec![file];ui.setup_message("The containing folder will become the vault. Choose Connect to open this note.");}else{ui.setup_message("Choose a local Markdown file.");}},Err(e)if !dismissed(&e)=>ui.setup_message(&format!("File picker: {e}. You can enter its folder path above instead.")),_=>{}}}});
        });
        let recent = prefs::get().recent_vaults;
        if !recent.is_empty() {
            let list = widgets::vbox(4);
            for root in recent.into_iter().take(5) {
                let button = gtk::Button::with_label(
                    &root.file_name().unwrap_or_default().to_string_lossy(),
                );
                button.set_tooltip_text(Some(&root.to_string_lossy()));
                let entry = path.downgrade();
                let selected = files.clone();
                button.connect_clicked(move |_| {
                    if let Some(entry) = entry.upgrade() {
                        entry.set_text(&root.to_string_lossy());
                        selected.borrow_mut().clear();
                    }
                });
                list.append(&button);
            }
            dialog
                .content_area()
                .append(&widgets::label("Recent vaults", "caption"));
            let s = scroll(&list);
            s.set_max_content_height(150);
            s.set_propagate_natural_height(true);
            dialog.content_area().append(&s);
        }
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Create folder", gtk::ResponseType::Other(1));
        dialog.add_button("Connect", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog,response|{
            if !matches!(response,gtk::ResponseType::Accept|gtk::ResponseType::Other(1)){dialog.close();return;}
            let Some(ui)=weak.upgrade()else{return;};let target=match entered_path(&path.text()){Ok(path)=>path,Err(e)=>{ui.setup_message(&format!("{e:#}"));return;}};
            if response==gtk::ResponseType::Other(1){ui.setup_message("Creating a new vault folder…");let weak=Rc::downgrade(&ui);worker::run(move ||{let parent=vault::vault_root(target.parent().context("Choose a parent folder")?)?;let name=target.file_name().and_then(|s|s.to_str()).context("Choose a visible UTF-8 folder name")?;vault::create(&parent,&parent,name,true)},move |result|{if let Some(ui)=weak.upgrade(){match result{Ok(root)=>ui.open_vault(root,Vec::new()),Err(e)=>ui.setup_message(&format!("Could not create folder: {e:#}. To use an existing folder, choose Connect."))}}});}
            else{ui.setup_message("Opening vault…");let selected=files.borrow().iter().filter(|p|p.parent()==Some(target.as_path())).cloned().collect();ui.open_vault(target,selected);}
        });
        dialog.present();
    }
}
