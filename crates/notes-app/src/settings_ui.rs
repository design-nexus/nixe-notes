//! Shared Nyx settings card, rebuilt from current vault/preferences when opened.
use super::*;
use nyx_ui::settings_card::{About, Standard};
type SettingsAction = fn(&Rc<Ui>);
fn group(body: &gtk::Box, name: &str) -> gtk::Box {
    let section = widgets::vbox(0);
    section.add_css_class("settings-group");
    section.set_widget_name(name);
    section.append(&widgets::label(&name.to_uppercase(), "group-title"));
    body.append(&section);
    section
}
fn number(
    group: &gtk::Box,
    name: &str,
    description: &str,
    id: &str,
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    change: impl Fn(f64) + 'static,
) {
    let spin = gtk::SpinButton::with_range(*range.start(), *range.end(), 1.0);
    spin.set_widget_name(id);
    spin.set_value(value);
    spin.connect_value_changed(move |s| change(s.value()));
    group.append(&widgets::row(name, description, Some(spin.upcast_ref())));
}
/// Every shortcut, for the settings card's Keyboard group.
const SHORTCUTS: &[(&[&str], &str)] = &[
    (&["Ctrl", "O"], "Open vault"),
    (&["Ctrl", "Enter"], "Follow local note link"),
    (&["F4"], "Cycle source, live and reading"),
    (&["F2"], "Rename or move note"),
    (&["F11"], "Focus mode"),
    (&["Ctrl", "T"], "Toggle task"),
    (&["Ctrl", "Shift", "D"], "Daily note"),
    (&["Ctrl", "Alt", "F"], "Fold heading"),
    (&["Ctrl", "N"], "New note"),
    (&["Ctrl", "S"], "Save"),
    (&["Ctrl", "Shift", "S"], "Save a copy"),
    (&["Ctrl", "F"], "Find in note"),
    (&["Ctrl", "Shift", "F"], "Search vault"),
    (&["Ctrl", "B"], "Collapse sidebar"),
    (&["Ctrl", "Z"], "Undo"),
    (&["Ctrl", "Shift", "Z"], "Redo"),
    (&["Ctrl", "W"], "Close note"),
    (&["Ctrl", "Q"], "Quit"),
    (&["Ctrl", ","], "Settings"),
    (&["F1"], "Shortcuts"),
    (&["Esc"], "Close search or panels"),
];

impl Ui {
    pub(super) fn settings(self: &Rc<Self>) {
        const GROUPS: &[(&str, &str, &str)] = &[
            (
                "Editor",
                "format-text-rich-symbolic",
                "Typography, width, editing mode",
            ),
            (
                "Vault",
                "folder-symbolic",
                "Connected folder, templates, attachments",
            ),
            (
                "Writing",
                "document-edit-symbolic",
                "Focus, typewriter, local history",
            ),
            (
                "Integrations",
                "applications-system-symbolic",
                "Suite permissions, AI, sync, extensions",
            ),
        ];
        let p = prefs::get();
        let body = widgets::vbox(0);
        body.set_hexpand(true);
        let editor = group(&body, "Editor");
        let weak = Rc::downgrade(self);
        number(
            &editor,
            "Font size",
            "Editor text size in points",
            "notes-font-size",
            p.font_size,
            8.0..=32.0,
            move |value| {
                prefs::update(|p| p.font_size = value);
                if let Some(ui) = weak.upgrade() {
                    ui.apply_typography();
                }
            },
        );
        let family = gtk::Entry::builder()
            .text(&p.font_family)
            .width_chars(16)
            .max_width_chars(24)
            .build();
        family.set_widget_name("notes-font-family");
        let weak = Rc::downgrade(self);
        family.connect_changed(move |e| {
            if !e.text().trim().is_empty() {
                prefs::update(|p| p.font_family = e.text().into());
                if let Some(ui) = weak.upgrade() {
                    ui.apply_typography();
                }
            }
        });
        editor.append(&widgets::row(
            "Font family",
            "Font used in the Markdown editor",
            Some(family.upcast_ref()),
        ));
        let weak = Rc::downgrade(self);
        number(
            &editor,
            "Line spacing",
            "Extra space above and below each line",
            "notes-line-spacing",
            p.line_spacing as f64,
            0.0..=20.0,
            move |value| {
                prefs::update(|p| p.line_spacing = value as i32);
                if let Some(ui) = weak.upgrade() {
                    ui.apply_typography();
                }
            },
        );
        let weak = Rc::downgrade(self);
        let (row, _) = widgets::choice_row(
            "Writing width",
            "Center text in a comfortable writing column",
            vec![
                ("0".into(), "Full width".into()),
                ("80".into(), "80 columns".into()),
                ("100".into(), "100 columns".into()),
            ],
            &p.writing_width.to_string(),
            move |v| {
                prefs::update(|p| p.writing_width = v.parse().unwrap_or(0));
                if let Some(ui) = weak.upgrade() {
                    ui.apply_typography();
                }
            },
        );
        editor.append(&row);
        let weak = Rc::downgrade(self);
        let (row, _) = widgets::choice_row(
            "Editor mode",
            "Source, live formatting, or native reading",
            vec![
                ("source".into(), "Source".into()),
                ("live".into(), "Live preview".into()),
                ("reading".into(), "Reading".into()),
            ],
            &p.editor_mode,
            move |value| {
                prefs::update(|p| p.editor_mode = value.clone());
                if let Some(ui) = weak.upgrade() {
                    ui.live.set_active(value == "live");
                    ui.reading.set_active(value == "reading");
                    ui.sync_mode();
                }
            },
        );
        editor.append(&row);

        let vault = group(&body, "Vault");
        let current = self.root.borrow().clone().or_else(|| p.last_vault.clone());
        let location = gtk::Label::new(Some(
            &current
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| "No vault connected".into()),
        ));
        location.set_widget_name("notes-vault-path");
        location.set_selectable(true);
        location.set_wrap(true);
        location.set_xalign(0.0);
        vault.append(&widgets::row(
            "Connected vault",
            "This folder reopens automatically when Notes starts",
            None,
        ));
        vault.append(&location);
        let connect = gtk::Button::with_label("Connect or change vault…");
        let weak = Rc::downgrade(self);
        connect.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                nyx_ui::settings_card::close();
                ui.choose_vault();
            }
        });
        vault.append(&connect);
        let (row, _) = widgets::switch_row(
            "Restore open notes",
            "Reopen this vault's saved tabs; the vault itself is always remembered",
            p.restore_tabs,
            |value| prefs::update(|p| p.restore_tabs = value),
        );
        vault.append(&row);
        let mut folder_entries = Vec::new();
        for (index, label, value) in [
            (0, "Templates folder", p.templates_folder),
            (1, "Daily notes folder", p.daily_folder),
            (2, "Daily template filename", p.daily_template),
            (3, "Attachments folder", p.attachments_folder),
        ] {
            let entry = gtk::Entry::builder()
                .text(value)
                .width_chars(14)
                .max_width_chars(24)
                .build();
            entry.set_widget_name(&format!("notes-vault-folder-{index}"));
            folder_entries.push(entry.clone());
            vault.append(&widgets::row(
                label,
                "Relative to the vault; leave empty to use its root",
                Some(entry.upcast_ref()),
            ));
        }

        let folder_status = gtk::Label::new(Some(
            "Folder changes are saved together with Apply folders.",
        ));
        folder_status.set_wrap(true);
        folder_status.set_xalign(0.0);
        vault.append(&folder_status);
        let apply = gtk::Button::with_label("Apply folders");
        apply.set_widget_name("notes-apply-folders");
        apply.connect_clicked(move |_| {
            let values=folder_entries.iter().map(|entry|entry.text().to_string()).collect::<Vec<_>>();
            let mut valid=true;
            for(entry,value)in folder_entries.iter().zip(&values){
                let allowed=!std::path::Path::new(value).is_absolute()&&!value.contains('\\')&&std::path::Path::new(value).components().all(|c|matches!(c,std::path::Component::Normal(name)if !name.to_string_lossy().starts_with('.')));
                if allowed {entry.remove_css_class("error");}else{entry.add_css_class("error");valid=false;}
            }
            if !valid {folder_status.set_text("Use relative folders without .. or hidden components. Previous settings are retained.");return;}
            prefs::update(|p|{p.templates_folder=values[0].clone();p.daily_folder=values[1].clone();p.daily_template=values[2].clone();p.attachments_folder=values[3].clone();});folder_status.set_text("Folder settings saved.");
        });
        vault.append(&apply);

        let writing = group(&body, "Writing");
        let weak = Rc::downgrade(self);
        let (row, _) = widgets::switch_row(
            "Focus mode",
            "Hide navigation and toolbars while writing (F11)",
            p.focus_mode,
            move |value| {
                prefs::update(|p| p.focus_mode = value);
                if let Some(ui) = weak.upgrade() {
                    ui.apply_writing();
                }
            },
        );
        writing.append(&row);
        let weak = Rc::downgrade(self);
        let (row, _) = widgets::switch_row(
            "Typewriter scrolling",
            "Keep the active line centered",
            p.typewriter,
            move |value| {
                prefs::update(|p| p.typewriter = value);
                if let Some(ui) = weak.upgrade() {
                    ui.apply_writing();
                }
            },
        );
        writing.append(&row);
        number(
            &writing,
            "History versions per note",
            "Automatic local snapshots; 0 disables future snapshots",
            "notes-history-count",
            p.history_keep as f64,
            0.0..=2000.0,
            |value| prefs::update(|p| p.history_keep = value as u32),
        );
        number(
            &writing,
            "History retention days",
            "Keep recent snapshots; 0 removes the age limit",
            "notes-history-days",
            p.history_days as f64,
            0.0..=3650.0,
            |value| prefs::update(|p| p.history_days = value as u32),
        );
        let integrations = group(&body, "Integrations");
        integrations.append(&self.integration_controls());
        let actions: Vec<(&str, SettingsAction)> = vec![
            #[cfg(feature = "ai")]
            (
                "Local AI and semantic indexing…",
                Ui::intelligence_preferences,
            ),
            #[cfg(feature = "sync")]
            ("Personal sync settings…", Ui::sync_preferences),
            #[cfg(feature = "plugins")]
            ("Manage extensions…", Ui::extensions),
            ("Keyboard shortcuts…", Ui::shortcuts_from_settings),
        ];
        for (label, action) in actions {
            let button = gtk::Button::with_label(label);
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    nyx_ui::settings_card::close();
                    action(&ui);
                }
            });
            integrations.append(&button);
        }
        let root = scroll(&body);
        root.set_vexpand(true);
        root.set_hexpand(true);
        nyx_ui::settings_card::open_page(
            &self.overlay,
            &self.window,
            nyx_ui::settings_card::Spec {
                groups: GROUPS,
                search_text: None,
                standard: Some(Standard {
                    app: "notes",
                    about: About {
                        name: "Notes",
                        icon: "io.github.design_nexus.nyx.Notes",
                        version: env!("CARGO_PKG_VERSION"),
                        description: "Markdown notes in a folder on this computer.",
                        licence: "MIT",
                        website: "https://github.com/design-nexus/nyx-notes",
                        issues: "https://github.com/design-nexus/nyx-notes/issues",
                        credits: &[],
                    },
                    shortcuts: SHORTCUTS,
                    reduce_motion: || prefs::get().reduce_motion,
                    set_reduce_motion: |on| prefs::update(|p| p.reduce_motion = on),
                }),
            },
            root.upcast_ref(),
            &body,
        );
    }
    fn shortcuts_from_settings(self: &Rc<Self>) {
        self.shortcuts();
    }
}
