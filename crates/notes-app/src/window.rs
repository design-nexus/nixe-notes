#[path = "canvas_ui.rs"]
mod canvas_ui;
#[path = "collections_ui.rs"]
mod collections_ui;
#[path = "context_menus.rs"]
mod context_menus;
#[path = "features.rs"]
mod features;
#[path = "graph_ui.rs"]
mod graph_ui;
#[path = "integrations_ui.rs"]
mod integrations_ui;
#[cfg(feature = "ai")]
#[path = "intelligence.rs"]
mod intelligence;
#[cfg(feature = "plugins")]
#[path = "plugins_ui.rs"]
mod plugins_ui;
#[path = "projects_ui.rs"]
mod projects_ui;
#[cfg(any(feature = "ai", feature = "plugins"))]
#[path = "proposals.rs"]
mod proposals;
#[path = "settings_ui.rs"]
mod settings_ui;
#[cfg(feature = "sync")]
#[path = "sync_ui.rs"]
mod sync_ui;
#[path = "vault_setup.rs"]
mod vault_setup;
#[path = "views.rs"]
mod views;
#[path = "writing.rs"]
mod writing;
use crate::{prefs, worker};
use anyhow::Context;
use gtk::{gdk, gio, glib, prelude::*};
use notes_editor::Editor;
use notes_vault::{self as vault, Draft, Loaded, Version};
use nyx_ui::{chrome, inspector::Inspector, widgets};
use sourceview5::prelude::*;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

type Document = Rc<Doc>;
type CanvasWindows = std::collections::HashMap<PathBuf, (gtk::Window, Rc<Cell<bool>>)>;
enum Jump {
    Reference(String),
    Offset(usize),
}
struct Doc {
    path: PathBuf,
    editor: Rc<Editor>,
    version: RefCell<Version>,
    bom: Cell<bool>,
    read_only: bool,
    loading: Cell<bool>,
    closed: Cell<bool>,
    recovery_pending: Cell<bool>,
    dirty: Cell<bool>,
    saving: Cell<bool>,
    conflict: Cell<bool>,
    label: gtk::Label,
    save_timer: RefCell<Option<glib::SourceId>>,
    recovery_timer: RefCell<Option<glib::SourceId>>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
}
struct Ui {
    window: gtk::ApplicationWindow,
    overlay: gtk::Overlay,
    nav: gtk::Box,
    split: gtk::Paned,
    #[cfg_attr(not(test), allow(dead_code))]
    head: gtk::Box,
    /// The 2.0 header; its subtitle is the vault's name.
    header: nyx_ui::chrome::AppHeader,
    tree: gtk::Box,
    /// The vault's most used tags, under the tree.
    tags: gtk::Box,
    root: RefCell<Option<PathBuf>>,
    db: RefCell<Option<PathBuf>>,
    generation: Cell<u64>,
    notebook: gtk::Notebook,
    secondary: gtk::Notebook,
    editor_split: gtk::Paned,
    active_pane: Cell<bool>,
    pending_cursors: RefCell<std::collections::HashMap<PathBuf, prefs::WorkspaceTab>>,
    pending_panes: RefCell<std::collections::HashMap<PathBuf, bool>>,
    reading: gtk::ToggleButton,
    mutating: Cell<bool>,
    docs: RefCell<Vec<Document>>,
    opening: RefCell<Vec<PathBuf>>,
    canvases: RefCell<CanvasWindows>,
    /// The open note's place in the vault and name, in the header's middle.
    doc_title: DocTitle,
    inspector: Rc<Inspector>,
    outline_model: gtk::StringList,
    outline_key: RefCell<Option<(PathBuf, u64)>>,
    outline_offsets: RefCell<Vec<usize>>,
    outgoing_model: gtk::StringList,
    outgoing: RefCell<Vec<notes_editor::Link>>,
    backlinks_model: gtk::StringList,
    backlink_items: RefCell<Vec<vault::Backlink>>,
    backlink_key: RefCell<Option<PathBuf>>,
    backlink_request: Cell<u64>,
    mentions_model: gtk::StringList,
    mention_items: RefCell<Vec<vault::SearchHit>>,
    properties_model: gtk::StringList,
    tags_model: gtk::StringList,
    tag_items: RefCell<Vec<String>>,
    #[cfg(feature = "ai")]
    pending_proposals: RefCell<std::collections::HashMap<PathBuf, String>>,
    pending_launcher: RefCell<Vec<vault::integrations::LauncherAction>>,
    suite_proposals: RefCell<std::collections::HashMap<String, serde_json::Value>>,
    pending_jumps: RefCell<std::collections::HashMap<PathBuf, Jump>>,
    status: gtk::Label,
    save_state: gtk::Label,
    split_toggle: gtk::ToggleButton,
    /// The tree's note rows by path, to mark the open note.
    tree_notes: RefCell<std::collections::HashMap<PathBuf, glib::WeakRef<gtk::Button>>>,
    backlinks_heading: Option<gtk::Label>,
    notice: gtk::Label,
    notice_row: gtk::Box,
    resolve_button: gtk::Button,
    resolve_target: RefCell<Option<Document>>,
    workspace: gtk::Stack,
    setup_window: RefCell<Option<glib::WeakRef<gtk::Window>>>,
    setup_status: RefCell<Option<glib::WeakRef<gtk::Label>>>,
    sidebar_expanded: Cell<bool>,
    search: gtk::SearchEntry,
    results: gtk::Box,
    find: gtk::SearchEntry,
    replacement: gtk::Entry,
    find_row: gtk::Box,
    compact: Cell<bool>,
    live: gtk::ToggleButton,
    allow_close: Cell<bool>,
    closing: Cell<bool>,
    close_wait: Cell<bool>,
    close_prompt: RefCell<Option<glib::WeakRef<gtk::Window>>>,
    #[cfg(feature = "ai")]
    semantic_busy: Cell<bool>,
    #[cfg(feature = "ai")]
    semantic_pending: Cell<bool>,
    #[cfg(feature = "ai")]
    semantic_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(feature = "sync")]
    synchronization: Rc<sync_ui::SyncState>,
    #[cfg(feature = "plugins")]
    extension_busy: Cell<bool>,
    indexing: Cell<bool>,
    reindex_needed: Cell<bool>,
    css: gtk::CssProvider,
    monitors: RefCell<std::collections::HashMap<PathBuf, gio::FileMonitor>>,
}
thread_local! {static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };}
pub fn launcher_action(
    app: &gtk::Application,
    action: vault::integrations::LauncherAction,
) -> Result<(), String> {
    action.validate().map_err(|e| e.to_string())?;
    if !prefs::get().glimpse_enabled {
        return Err("Enable the local Notes/Glimpse integration first".into());
    }
    let guard = app.hold();
    let app = app.clone();
    let root = action.vault.clone();
    let db = nyx_core::paths::data_dir().join("glimpse/launcher.sqlite");
    worker::run(
        move || {
            vault::integrations::launcher_vaults(&db).and_then(|roots| {
                if roots.contains(&root) {
                    Ok(())
                } else {
                    anyhow::bail!("This vault is not exported to Glimpse")
                }
            })
        },
        move |result| {
            let _guard = guard;
            if let Err(error) = result {
                if let Some(ui) = current_ui() {
                    ui.message(&error.to_string());
                } else {
                    eprintln!("Notes launcher: {error}");
                }
                return;
            }
            if !prefs::get().glimpse_enabled {
                return;
            }
            present(&app, Some(action.vault.clone()), Vec::new());
            if let Some(ui) = current_ui() {
                ui.pending_launcher.borrow_mut().push(action);
            }
        },
    );
    Ok(())
}
pub fn integration_tools() -> serde_json::Value {
    integrations_ui::tools()
}
pub fn integration_call(name: &str, args: serde_json::Value, reply: nyx_core::control::Reply) {
    integrations_ui::call(name, args, reply);
}
fn current_ui() -> Option<Rc<Ui>> {
    UI.with(|s| s.borrow().clone())
}
fn settings() {
    if let Some(ui) = current_ui() {
        ui.settings();
    }
}
fn shortcuts() {
    if let Some(ui) = current_ui() {
        ui.shortcuts();
    }
}

pub fn present(app: &gtk::Application, vault_path: Option<PathBuf>, files: Vec<PathBuf>) {
    if let Some(ui) = current_ui() {
        if let Some(path) = vault_path {
            ui.open_vault(path, files);
        } else {
            for file in files {
                ui.open_file(file);
            }
        }
        ui.window.present();
        return;
    }
    nyx_theme::use_design_2();
    nyx_theme::install(nyx_theme::Style {
        window_class: "notes-window",
        base_skip: &[],
        css: include_str!("style.css"),
        no_glow_css: "",
        extra_colors: None,
        prefs: || {
            let p = prefs::get();
            nyx_theme::ThemePrefs {
                mode: p.mode,
                theme: p.theme,
                glow: p.glow,
                reduce_motion: p.reduce_motion,
            }
        },
        set_theme: Some(|mode, theme| {
            prefs::update(|p| {
                p.mode = mode;
                p.theme = theme;
            })
        }),
    });
    // Settings saved before 2.0 carry the old defaults (Source, full width); move
    // those once to Live and a readable column. Later choices are left alone.
    if !prefs::get().design_2 {
        prefs::update(|p| {
            if p.editor_mode == "source" {
                p.editor_mode = "live".into();
            }
            if p.writing_width == 0 {
                p.writing_width = 70;
            }
            p.design_2 = true;
        });
    }
    let ui = Ui::build(app);
    UI.with(|s| *s.borrow_mut() = Some(ui.clone()));
    ui.apply_typography();
    ui.apply_writing();
    let root = vault_path
        .or_else(|| files.first().and_then(|p| p.parent().map(PathBuf::from)))
        .or_else(|| prefs::get().last_vault);
    ui.window.present();
    if let Some(root) = root {
        ui.open_vault(root, files);
    } else {
        ui.welcome();
    }
    ui.window.present();
    if prefs::take_broken() {
        ui.message("Settings could not be read. The previous file was kept as settings.toml.bak.");
    }
    let state = nyx_core::paths::state_dir();
    let weak = Rc::downgrade(&ui);
    worker::run(
        move || vault::recovery_paths(&state),
        move |result| {
            if let Some(ui) = weak.upgrade() {
                match result {
                    Ok(paths) if !paths.is_empty() => ui.offer_recovery(paths),
                    Err(e) => ui.message(&format!("Recovery could not be read: {e:#}")),
                    _ => {}
                }
            }
        },
    );
    let state = nyx_core::paths::state_dir();
    let weak = Rc::downgrade(&ui);
    worker::run(
        move || vault::workspace::interrupted_moves(&state),
        move |result| {
            if let Some(ui) = weak.upgrade() {
                match result{Ok(plans)if !plans.is_empty()=>ui.message("An interrupted file operation is available. Use File → Recover file operations."),Err(e)=>ui.message(&format!("Operation recovery: {e:#}")),_=>{}}
            }
        },
    );
}
/// The header's middle: the open note's folder (dimmed), its name, and a dot while
/// it has unsaved changes.
struct DocTitle {
    root: gtk::Box,
    folder: gtk::Label,
    name: gtk::Label,
    dot: gtk::Box,
}

impl DocTitle {
    fn new() -> Self {
        let root = widgets::hbox(6);
        root.add_css_class("doc-title");
        let folder = widgets::label("", "doc-title-folder");
        folder.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        folder.set_max_width_chars(28);
        let name = widgets::label("", "doc-title-name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(40);
        let dot = widgets::hbox(0);
        dot.add_css_class("doc-title-dot");
        dot.set_valign(gtk::Align::Center);
        dot.set_tooltip_text(Some("Unsaved changes"));
        dot.set_visible(false);
        root.append(&folder);
        root.append(&name);
        root.append(&dot);
        Self {
            root,
            folder,
            name,
            dot,
        }
    }

    fn set(&self, folder: &Path, note: &Path, dirty: bool) {
        let parts: Vec<String> = folder
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.folder.set_text(&if parts.is_empty() {
            String::new()
        } else {
            format!("{} /", parts.join(" / "))
        });
        self.folder.set_visible(!parts.is_empty());
        self.name
            .set_text(&note.file_stem().unwrap_or_default().to_string_lossy());
        self.dot.set_visible(dirty);
    }
}

impl Ui {
    fn build(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Notes")
            .default_width(1120)
            .default_height(800)
            .build();
        window.add_css_class("notes-window");
        window.set_icon_name(Some("io.github.design_nexus.nyx.Notes"));
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        nyx_ui::keys::install(
            &window,
            nyx_ui::keys::Common {
                settings: Some(settings),
                shortcuts: Some(shortcuts),
                ..Default::default()
            },
        );
        let overlay = gtk::Overlay::new();
        let frame = widgets::vbox(0);
        let body = widgets::hbox(0);
        body.set_vexpand(true);
        let nav = widgets::vbox(0);
        nav.add_css_class("settings-navigation");
        nav.set_width_request(SIDEBAR);
        // The sidebar's toggle sits in the header, after the app's name.
        let head = widgets::hbox(0);
        let collapse = widgets::bar_button("sidebar-show-symbolic", "Toggle the sidebar (Ctrl+B)");
        head.append(&collapse);
        let tree = widgets::vbox(0);
        let tags = widgets::vbox(0);
        tags.add_css_class("tree-tags");
        let side = widgets::vbox(0);
        side.append(&tree);
        side.append(&tags);
        let tree_scroll = scroll(&side);
        tree_scroll.set_vexpand(true);
        nav.append(&tree_scroll);
        let center = widgets::vbox(0);
        center.set_hexpand(true);
        // The header's ⋯ menu: the vault and file actions, editing, and the modes (each
        // also on its key); features add their items below.
        let item = |label: &str| {
            let b = gtk::Button::with_label(label);
            b.set_has_frame(false);
            b.add_css_class("notes-menu-item");
            if let Some(child) = b.child().and_downcast::<gtk::Label>() {
                child.set_xalign(0.0);
            }
            b.connect_clicked(|b| {
                if let Some(popover) = b
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    popover.popdown();
                }
            });
            b
        };
        let toggle_item = |label: &str, tooltip: &str| {
            let t = gtk::ToggleButton::with_label(label);
            t.set_has_frame(false);
            t.set_tooltip_text(Some(tooltip));
            t.add_css_class("notes-menu-item");
            if let Some(child) = t.child().and_downcast::<gtk::Label>() {
                child.set_xalign(0.0);
            }
            t
        };
        let open = item("Open vault…  (Ctrl+O)");
        let new_note = item("New note  (Ctrl+N)");
        let new_folder = item("New folder");
        let refresh = item("Refresh vault and search index");
        let save = item("Save  (Ctrl+S)");
        let save_as = item("Save a copy  (Ctrl+Shift+S)");
        let undo = item("Undo  (Ctrl+Z)");
        let redo = item("Redo  (Ctrl+Shift+Z)");
        let live = toggle_item(
            "Live formatting",
            "Live formatting; Markdown source remains intact",
        );
        let reading = toggle_item("Reading mode", "Reading mode (F4 cycles modes)");
        let task_button = item("Toggle task  (Ctrl+T)");
        let fold_button = item("Fold or unfold heading  (Ctrl+Alt+F)");
        let find_button = item("Find in note  (Ctrl+F)");
        let menu = widgets::vbox(2);
        menu.add_css_class("notes-menu");
        let popover = gtk::Popover::new();
        let menu_scroll = scroll(&menu);
        menu_scroll.set_max_content_height(520);
        menu_scroll.set_propagate_natural_height(true);
        popover.set_child(Some(&menu_scroll));
        let file_menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Menu (F10)")
            .build();
        file_menu.add_css_class("bar-button");
        file_menu.set_popover(Some(&popover));
        for (i, widget) in [
            open.upcast_ref::<gtk::Widget>(),
            new_note.upcast_ref(),
            new_folder.upcast_ref(),
            refresh.upcast_ref(),
            save.upcast_ref(),
            save_as.upcast_ref(),
            undo.upcast_ref(),
            redo.upcast_ref(),
            live.upcast_ref(),
            reading.upcast_ref(),
            task_button.upcast_ref(),
            fold_button.upcast_ref(),
            find_button.upcast_ref(),
        ]
        .into_iter()
        .enumerate()
        {
            if i == 4 || i == 8 {
                menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            menu.append(widget);
        }
        menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let split_button = gtk::ToggleButton::new();
        split_button.set_icon_name("view-dual-symbolic");
        split_button.set_tooltip_text(Some("Split editor (Ctrl+\\)"));
        split_button.add_css_class("bar-button");
        split_button.set_valign(gtk::Align::Center);
        split_button.set_active(prefs::get().split_enabled);
        // The editor's foot: the note's words and the cursor, then the format and
        // whether it's saved; or what the vault is doing.
        let status = gtk::Label::new(Some("Open a vault to begin"));
        status.add_css_class("notes-status");
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_xalign(0.0);
        status.set_hexpand(true);
        let save_state = gtk::Label::new(None);
        save_state.add_css_class("notes-status");
        let save_dot = widgets::hbox(0);
        save_dot.add_css_class("save-dot");
        save_dot.set_valign(gtk::Align::Center);
        let foot = widgets::hbox(14);
        foot.add_css_class("notes-foot");
        foot.append(&status);
        foot.append(&widgets::label("Markdown", "notes-status"));
        let saved = widgets::hbox(6);
        saved.append(&save_dot);
        saved.append(&save_state);
        foot.append(&saved);
        let doc_title = DocTitle::new();
        let inspector = Inspector::new();
        let (outline_model, outline_view, outline_scroll) = text_list(240);
        let (backlinks_model, backlinks_view, backlinks_scroll) = card_list(260);
        let outline = inspector.segment("Outline");
        outline.section("On this page").add(&outline_scroll);
        outline.section("Backlinks").add(&backlinks_scroll);
        // The outline and the cards take the room they need, so Backlinks follows the
        // headings directly.
        for list in [&outline_scroll, &backlinks_scroll] {
            list.set_min_content_height(0);
            list.set_propagate_natural_height(true);
        }
        let backlinks_heading = backlinks_scroll
            .parent()
            .and_then(|section| section.first_child())
            .and_downcast::<gtk::Label>();
        let (outgoing_model, outgoing_view, outgoing_scroll) = text_list(160);
        let links = inspector.segment("Links");
        links
            .section("Outgoing · Ctrl+Enter to follow")
            .add(&outgoing_scroll);
        let (mentions_model, mentions_view, mentions_scroll) = text_list(140);
        links.section("Unlinked mentions").add(&mentions_scroll);
        let (properties_model, _, properties_scroll) = text_list(200);
        let (tags_model, tags_view, tags_scroll) = text_list(120);
        let info = inspector.segment("Info");
        let properties_section = info.section("Properties");
        properties_section.add(&properties_scroll);
        let edit_properties = gtk::Button::with_label("Edit properties…");
        properties_section.add(&edit_properties);
        info.section("Tags · click to search").add(&tags_scroll);
        let notebook = gtk::Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_group_name(Some("notes-tabs"));
        notebook.set_show_border(false);
        notebook.set_vexpand(true);
        notebook.set_hexpand(true);
        let find = gtk::SearchEntry::new();
        find.set_placeholder_text(Some("Find in note…"));
        let replacement = gtk::Entry::builder()
            .placeholder_text("Replace with…")
            .build();
        let next = widgets::bar_button("go-down-symbolic", "Find next (Enter)");
        let replace = gtk::Button::with_label("Replace");
        let find_row = widgets::hbox(4);
        find_row.add_css_class("notes-find");
        find.set_hexpand(true);
        replacement.set_hexpand(true);
        for widget in [
            find.upcast_ref::<gtk::Widget>(),
            replacement.upcast_ref(),
            next.upcast_ref(),
            replace.upcast_ref(),
        ] {
            find_row.append(widget);
        }
        find_row.set_visible(false);
        let notice = gtk::Label::new(None);
        notice.set_wrap(true);
        notice.set_hexpand(true);
        notice.set_xalign(0.0);
        let notice_row = widgets::hbox(6);
        notice_row.add_css_class("notes-notice");
        notice_row.append(&notice);
        let resolve = gtk::Button::with_label("Resolve");
        resolve.set_visible(false);
        notice_row.append(&resolve);
        let dismiss = widgets::bar_button("window-close-symbolic", "Dismiss message");
        notice_row.append(&dismiss);
        notice_row.set_visible(false);
        center.append(&find_row);
        center.append(&notice_row);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search notes"));
        search.set_tooltip_text(Some("Words, or tag: path: property:name=value heading:"));
        // A fixed-width field with the key that reaches it, like the mockup's.
        search.set_width_chars(16);
        let search_box = widgets::hbox(6);
        search_box.add_css_class("header-search");
        search_box.set_valign(gtk::Align::Center);
        search_box.append(&search);
        search_box.append(&widgets::label("Ctrl ⇧ F", "key-hint"));
        let results = widgets::vbox(4);
        let result_scroll = scroll(&results);
        result_scroll.set_max_content_height(240);
        result_scroll.set_propagate_natural_height(true);
        result_scroll.set_visible(false);
        center.append(&result_scroll);
        let secondary = gtk::Notebook::new();
        secondary.set_scrollable(true);
        secondary.set_group_name(Some("notes-tabs"));
        secondary.set_show_border(false);
        secondary.set_vexpand(true);
        secondary.set_hexpand(true);
        secondary.set_visible(prefs::get().split_enabled);
        let editor_split = gtk::Paned::new(gtk::Orientation::Horizontal);
        editor_split.set_start_child(Some(&notebook));
        editor_split.set_end_child(Some(&secondary));
        editor_split.set_resize_start_child(true);
        editor_split.set_resize_end_child(true);
        editor_split.set_shrink_start_child(true);
        editor_split.set_shrink_end_child(true);
        let workspace = gtk::Stack::new();
        workspace.set_vexpand(true);
        workspace.set_hexpand(true);
        let welcome_panel = widgets::vbox(16);
        welcome_panel.set_halign(gtk::Align::Center);
        welcome_panel.set_valign(gtk::Align::Center);
        welcome_panel.set_margin_start(24);
        welcome_panel.set_margin_end(24);
        welcome_panel.append(&widgets::label("Your notes, in your folders", "title"));
        let explanation = gtk::Label::new(Some(
            "Connect an existing Markdown or Obsidian vault, or create a new folder.
Your Markdown files stay where they are.",
        ));
        explanation.set_wrap(true);
        explanation.set_max_width_chars(48);
        welcome_panel.append(&explanation);
        let connect_vault = widgets::labeled_button("folder-open-symbolic", "Connect a vault…");
        welcome_panel.append(&connect_vault);
        workspace.add_named(&welcome_panel, Some("welcome"));
        workspace.add_named(&inspector.wrap(&editor_split), Some("editor"));
        inspector.set_open(prefs::get().inspector_open);
        inspector.connect_open(|open| {
            if prefs::get().inspector_open != open {
                prefs::update(|p| p.inspector_open = open);
            }
        });
        center.append(&workspace);
        center.append(&foot);
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.set_start_child(Some(&nav));
        split.set_end_child(Some(&center));
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);
        split.set_position(SIDEBAR);
        split.set_hexpand(true);
        body.append(&split);
        // The header: the app and the vault, vault search, and Settings.
        let header =
            nyx_ui::chrome::app_header(&window, "accessories-text-editor-symbolic", "Notes");
        header.start.append(&head);
        header.center.append(&doc_title.root);
        header.end.set_spacing(6);
        search_box.set_margin_end(8);
        header.end.append(&search_box);
        header.end.append(&split_button);
        inspector.toggle_button().add_css_class("bar-button");
        inspector.toggle_button().set_valign(gtk::Align::Center);
        inspector
            .toggle_button()
            .set_tooltip_text(Some("Inspector (Ctrl+Alt+I)"));
        header.end.append(inspector.toggle_button());
        header.end.append(&file_menu);
        header
            .end
            .append(&nyx_ui::chrome::settings_button(settings));
        frame.append(&header.root);
        frame.append(&body);
        overlay.set_child(Some(&frame));
        window.set_child(Some(&overlay));
        let css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
        let ui = Rc::new(Self {
            window,
            overlay,
            nav,
            split,
            head: head.clone(),
            header,
            tree,
            tags,
            root: RefCell::new(None),
            db: RefCell::new(None),
            generation: Cell::new(0),
            notebook,
            secondary,
            editor_split,
            active_pane: Cell::new(false),
            pending_cursors: RefCell::new(std::collections::HashMap::new()),
            pending_panes: RefCell::new(if nyx_theme::reopens_last_page() {
                let p = prefs::get();
                p.open_files
                    .iter()
                    .map(|path| (path.clone(), p.split_files.contains(path)))
                    .collect()
            } else {
                std::collections::HashMap::new()
            }),
            reading: reading.clone(),
            mutating: Cell::new(false),
            docs: RefCell::new(Vec::new()),
            opening: RefCell::new(Vec::new()),
            canvases: RefCell::new(std::collections::HashMap::new()),
            doc_title,
            inspector,
            outline_model,
            outline_key: RefCell::new(None),
            outline_offsets: RefCell::new(Vec::new()),
            outgoing_model,
            outgoing: RefCell::new(Vec::new()),
            backlinks_model,
            backlink_items: RefCell::new(Vec::new()),
            backlink_key: RefCell::new(None),
            backlink_request: Cell::new(0),
            mentions_model,
            mention_items: RefCell::new(Vec::new()),
            properties_model,
            tags_model,
            tag_items: RefCell::new(Vec::new()),
            #[cfg(feature = "ai")]
            pending_proposals: RefCell::new(std::collections::HashMap::new()),
            pending_launcher: RefCell::new(Vec::new()),
            suite_proposals: RefCell::new(std::collections::HashMap::new()),
            pending_jumps: RefCell::new(std::collections::HashMap::new()),
            status,
            save_state,
            split_toggle: split_button.clone(),
            tree_notes: RefCell::new(std::collections::HashMap::new()),
            backlinks_heading,
            notice,
            notice_row,
            resolve_button: resolve.clone(),
            resolve_target: RefCell::new(None),
            workspace,
            setup_window: RefCell::new(None),
            setup_status: RefCell::new(None),
            sidebar_expanded: Cell::new(false),
            search,
            results,
            find,
            replacement,
            find_row,
            compact: Cell::new(false),
            live: live.clone(),
            allow_close: Cell::new(false),
            closing: Cell::new(false),
            close_wait: Cell::new(false),
            close_prompt: RefCell::new(None),
            #[cfg(feature = "ai")]
            semantic_busy: Cell::new(false),
            #[cfg(feature = "ai")]
            semantic_pending: Cell::new(false),
            #[cfg(feature = "ai")]
            semantic_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(feature = "sync")]
            synchronization: Rc::new(sync_ui::SyncState::default()),
            #[cfg(feature = "plugins")]
            extension_busy: Cell::new(false),
            indexing: Cell::new(false),
            reindex_needed: Cell::new(false),
            css,
            monitors: RefCell::new(std::collections::HashMap::new()),
        });
        let weak = Rc::downgrade(&ui);
        outline_view.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade()
                && let Some(doc) = ui.active()
                && let Some(offset) = ui.outline_offsets.borrow().get(position as usize)
            {
                doc.editor.jump_to(*offset);
            }
        });
        let weak = Rc::downgrade(&ui);
        outgoing_view.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade() {
                let link = ui.outgoing.borrow().get(position as usize).cloned();
                if let Some(link) = link {
                    ui.follow_link(link);
                }
            }
        });
        let weak = Rc::downgrade(&ui);
        backlinks_view.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade() {
                let backlink = ui.backlink_items.borrow().get(position as usize).cloned();
                if let Some(link) = backlink {
                    ui.pending_jumps
                        .borrow_mut()
                        .insert(link.path.clone(), Jump::Offset(link.offset));
                    ui.open_file(link.path);
                    if let Some(doc) = ui.active() {
                        ui.apply_pending_jump(&doc);
                    }
                }
            }
        });
        let weak = Rc::downgrade(&ui);
        mentions_view.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade() {
                let hit = ui.mention_items.borrow().get(position as usize).cloned();
                if let Some(hit) = hit {
                    if let Some(offset) = hit.offset {
                        ui.pending_jumps
                            .borrow_mut()
                            .insert(hit.path.clone(), Jump::Offset(offset));
                    }
                    ui.open_file(hit.path);
                }
            }
        });
        let weak = Rc::downgrade(&ui);
        tags_view.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade() {
                let tag = ui.tag_items.borrow().get(position as usize).cloned();
                if let Some(tag) = tag {
                    ui.show_search();
                    ui.search.set_text(&format!("tag:{tag}"));
                }
            }
        });
        let weak = Rc::downgrade(&ui);
        edit_properties.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.edit_properties();
            }
        });
        let weak = Rc::downgrade(&ui);
        app.connect_window_removed(move |_, window| {
            if let Some(ui) = weak.upgrade()
                && ui.closing.get()
                && window != &ui.window
            {
                let weak = Rc::downgrade(&ui);
                glib::idle_add_local_once(move || {
                    if let Some(ui) = weak.upgrade()
                        && ui.closing.get()
                    {
                        ui.close_window();
                    }
                });
            }
        });
        ui.feature_menu(&menu);
        ui.explorer_context(&ui.tree, PathBuf::new(), true);
        let save_all = gtk::Button::with_label("Save all");
        let weak = Rc::downgrade(&ui);
        save_all.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.save_all();
            }
        });
        menu.prepend(&save_all);
        let weak = Rc::downgrade(&ui);
        split_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.toggle_split();
            }
        });
        let weak = Rc::downgrade(&ui);
        task_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.toggle_task();
            }
        });
        let weak = Rc::downgrade(&ui);
        fold_button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.fold();
            }
        });
        macro_rules! click {
            ($button:expr,$method:ident) => {{
                let weak = Rc::downgrade(&ui);
                $button.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.$method();
                    }
                });
            }};
        }
        click!(open, choose_vault);
        click!(connect_vault, choose_vault);
        click!(new_note, new_note);
        click!(new_folder, new_folder);
        click!(refresh, refresh);
        click!(save, save_active);
        click!(save_as, save_copy);
        click!(find_button, show_find);
        click!(next, find_next);
        click!(replace, replace);
        click!(resolve, resolve_active);
        let weak = Rc::downgrade(&ui);
        dismiss.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.notice_row.set_visible(false);
            }
        });
        let weak = Rc::downgrade(&ui);
        collapse.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.toggle_sidebar();
            }
        });
        let weak = Rc::downgrade(&ui);
        undo.connect_clicked(move |_| {
            if let Some(doc) = weak.upgrade().and_then(|ui| ui.active())
                && doc.editor.buffer.can_undo()
            {
                doc.editor.buffer.undo();
            }
        });
        let weak = Rc::downgrade(&ui);
        redo.connect_clicked(move |_| {
            if let Some(doc) = weak.upgrade().and_then(|ui| ui.active())
                && doc.editor.buffer.can_redo()
            {
                doc.editor.buffer.redo();
            }
        });
        let weak = Rc::downgrade(&ui);
        live.connect_toggled(move |button| {
            if let Some(ui) = weak.upgrade() {
                if button.is_active() {
                    ui.reading.set_active(false);
                }
                ui.sync_mode();
            }
        });
        let weak = Rc::downgrade(&ui);
        reading.connect_toggled(move |button| {
            if let Some(ui) = weak.upgrade() {
                if button.is_active() {
                    ui.live.set_active(false);
                }
                ui.sync_mode();
            }
        });
        let saved_mode = prefs::get().editor_mode;
        if saved_mode == "live" {
            live.set_active(true);
        } else if saved_mode == "reading" {
            reading.set_active(true);
        }
        let weak = Rc::downgrade(&ui);
        ui.secondary.connect_switch_page(move |_, _, _| {
            if let Some(ui) = weak.upgrade() {
                ui.active_pane.set(true);
                ui.update_active();
            }
        });
        let weak = Rc::downgrade(&ui);
        ui.notebook.connect_switch_page(move |_, _, _| {
            if let Some(ui) = weak.upgrade() {
                ui.active_pane.set(false);
                ui.update_active();
            }
        });
        let weak = Rc::downgrade(&ui);
        ui.find.connect_activate(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.find_next();
            }
        });
        let weak = Rc::downgrade(&ui);
        ui.search.connect_search_changed(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.search_vault();
            }
        });
        let weak = Rc::downgrade(&ui);
        ui.search.connect_activate(move |_| {
            if let Some(ui) = weak.upgrade()
                && let Some(button) = ui.results.first_child().and_downcast::<gtk::Button>()
            {
                button.emit_clicked();
            }
        });
        let search_keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&ui);
        search_keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Down
                && let Some(ui) = weak.upgrade()
                && let Some(first) = ui.results.first_child()
            {
                first.grab_focus();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        ui.search.add_controller(search_keys);
        let results_scroll = result_scroll.clone();
        ui.search.connect_search_changed(move |search| {
            results_scroll.set_visible(!search.text().is_empty());
        });
        let weak = Rc::downgrade(&ui);
        ui.window.connect_close_request(move |_| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if ui.allow_close.get() {
                return glib::Propagation::Proceed;
            }
            #[cfg(feature = "sync")]
            ui.synchronization
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            ui.close_window();
            glib::Propagation::Stop
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&ui);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::F11 => ui.toggle_focus(),
                gdk::Key::F4 => ui.cycle_mode(),
                gdk::Key::F2 => {
                    if let Some(doc) = ui.active() {
                        ui.move_note(doc.path.clone());
                    }
                }
                gdk::Key::t | gdk::Key::T if ctrl => ui.toggle_task(),
                gdk::Key::d | gdk::Key::D if ctrl && shift => ui.daily(),
                gdk::Key::f | gdk::Key::F if ctrl && mods.contains(gdk::ModifierType::ALT_MASK) => {
                    ui.fold()
                }
                gdk::Key::Return if ctrl => {
                    if let Some(link) = ui.active().and_then(|doc| doc.editor.link_at_cursor()) {
                        ui.follow_link(link);
                    }
                }
                gdk::Key::s | gdk::Key::S if ctrl && shift => ui.save_copy(),
                gdk::Key::s | gdk::Key::S if ctrl => ui.save_active(),
                gdk::Key::o | gdk::Key::O if ctrl => ui.choose_vault(),
                gdk::Key::n | gdk::Key::N if ctrl => ui.new_note(),
                gdk::Key::f | gdk::Key::F if ctrl && shift => ui.show_search(),
                gdk::Key::f | gdk::Key::F if ctrl => ui.show_find(),
                gdk::Key::b | gdk::Key::B if ctrl => {
                    ui.toggle_sidebar();
                }
                gdk::Key::q | gdk::Key::Q if ctrl => ui.window.close(),
                gdk::Key::w | gdk::Key::W if ctrl => {
                    if let Some(doc) = ui.active() {
                        ui.close_tab(doc);
                    }
                }
                gdk::Key::F1 => ui.shortcuts(),
                gdk::Key::Escape => {
                    if prefs::get().focus_mode {
                        ui.toggle_focus();
                    }
                    nyx_ui::settings_card::escape();
                    ui.find_row.set_visible(false);
                    ui.search.set_text("");
                    ui.inspector.escape();
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        ui.window.add_controller(keys);
        let weak = Rc::downgrade(&ui);
        ui.window.add_tick_callback(move |_, _| {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            ui.fit();
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(&ui);
        nyx_theme::subscribe(&ui.window, move || {
            if let Some(ui) = weak.upgrade() {
                ui.apply_typography();
            }
        });
        let weak = Rc::downgrade(&ui);
        glib::timeout_add_local(Duration::from_secs(30), move || {
            let Some(ui) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            #[cfg(feature = "sync")]
            ui.sync_now(false);
            if ui.root.borrow().is_some() && !ui.indexing.get() {
                ui.rebuild();
            }
            glib::ControlFlow::Continue
        });
        ui
    }
    fn current_notebook(&self) -> &gtk::Notebook {
        if self.active_pane.get() && self.secondary.is_visible() {
            &self.secondary
        } else {
            &self.notebook
        }
    }
    fn doc_notebook(&self, doc: &Doc) -> &gtk::Notebook {
        if self.secondary.page_num(&doc.editor.root).is_some() {
            &self.secondary
        } else {
            &self.notebook
        }
    }
    fn active(&self) -> Option<Document> {
        let notebook = self.current_notebook();
        let page = notebook.current_page()?;
        let widget = notebook.nth_page(Some(page))?;
        self.docs
            .borrow()
            .iter()
            .find(|doc| doc.editor.root.upcast_ref::<gtk::Widget>() == &widget)
            .cloned()
    }
    fn message(&self, text: &str) {
        self.resolve_button.set_visible(false);
        self.resolve_target.borrow_mut().take();
        self.notice_row.remove_css_class("conflict");
        self.notice.set_text(text);
        self.notice_row.set_visible(true);
    }
    fn fit(&self) {
        nyx_ui::settings_card::fit(&self.window);
        let width = if self.window.width() > 0 {
            self.window.width()
        } else {
            self.window.default_width()
        };
        let compact =
            prefs::get().sidebar_collapsed || (width < 980 && !self.sidebar_expanded.get());
        if self.compact.replace(compact) != compact {
            // Collapsed, the sidebar goes away; its toggle stays in the header.
            self.nav.set_visible(!compact);
            self.tree.set_visible(!compact);
            if !compact {
                self.split.set_position(SIDEBAR);
            }
        }
    }
    fn toggle_sidebar(&self) {
        let expand = self.compact.get();
        self.sidebar_expanded.set(expand);
        prefs::update(|p| p.sidebar_collapsed = !expand);
        self.fit();
    }
    fn welcome(self: &Rc<Self>) {
        clear(&self.tree);
        self.tree
            .append(&widgets::label("RECENT VAULTS", "nav-group"));
        for path in prefs::get().recent_vaults {
            let button = widgets::labeled_button(
                "folder-symbolic",
                &path.file_name().unwrap_or_default().to_string_lossy(),
            );
            button.add_css_class("nav-item");
            button.set_tooltip_text(Some(&path.display().to_string()));
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.open_vault(path.clone(), vec![]);
                }
            });
            self.tree.append(&button);
        }
        self.workspace.set_visible_child_name("welcome");
        self.choose_vault();
    }
    fn open_vault(self: &Rc<Self>, path: PathBuf, files: Vec<PathBuf>) {
        let weak = Rc::downgrade(self);
        worker::run(
            move || vault::vault_root(&path),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                match result {
                    Err(e) => {
                        let message = format!("Could not open vault: {e:#}");
                        ui.message(&message);
                        if ui.root.borrow().is_none() && prefs::get().last_vault.is_none() {
                            ui.choose_vault();
                        }
                        ui.setup_message(&message);
                    }
                    Ok(root) => {
                        ui.finish_vault_setup();
                        ui.workspace.set_visible_child_name("editor");
                        ui.notice_row.set_visible(false);
                        *ui.backlink_key.borrow_mut() = None;
                        ui.generation.set(ui.generation.get() + 1);
                        *ui.root.borrow_mut() = Some(root.clone());
                        ui.monitors.borrow_mut().clear();
                        let id = vault::draft_path(&nyx_core::paths::cache_dir(), &root)
                            .file_stem()
                            .unwrap()
                            .to_os_string();
                        *ui.db.borrow_mut() = Some(
                            nyx_core::paths::cache_dir()
                                .join(id)
                                .with_extension("sqlite"),
                        );
                        prefs::update(|p| {
                            p.last_vault = Some(root.clone());
                            p.recent_vaults.retain(|r| r != &root);
                            p.recent_vaults.insert(0, root.clone());
                            p.recent_vaults.truncate(12);
                        });
                        let vault_name = root.file_name().unwrap_or_default().to_string_lossy();
                        chrome::set_title(&ui.window, "Notes", &vault_name);
                        ui.header.set_subtitle(&vault_name);
                        ui.populate_root();
                        ui.rebuild();
                        let to_open = if files.is_empty() && prefs::get().restore_tabs {
                            prefs::get()
                                .open_files
                                .into_iter()
                                .filter(|p| p.starts_with(&root))
                                .collect()
                        } else {
                            files
                        };
                        for file in to_open {
                            ui.open_file(file);
                        }
                        ui.drain_launcher();
                    }
                }
            },
        );
    }
    fn populate_root(self: &Rc<Self>) {
        self.monitors.borrow_mut().clear();
        clear(&self.tree);
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        // The vault's name is in the header; the tree starts with its folders.
        self.tree_notes.borrow_mut().clear();
        let listing = widgets::vbox(0);
        self.tree.append(&listing);
        self.populate(root, listing, 0);
    }
    /// The Tags group under the tree; a tag searches the vault for it.
    fn populate_tags(self: &Rc<Self>) {
        let Some(db) = self.db.borrow().clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        worker::run(
            move || vault::tag_counts(&db, 12),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                clear(&ui.tags);
                let Ok(tags) = result else { return };
                if tags.is_empty() {
                    return;
                }
                ui.tags.append(&widgets::label("TAGS", "nav-group"));
                ui.tags.last_child().unwrap().add_css_class("tree-group");
                for (tag, count) in tags {
                    let row = widgets::hbox(8);
                    let hash = widgets::label("#", "tree-hash");
                    hash.set_width_chars(2);
                    hash.set_xalign(0.5);
                    row.append(&hash);
                    let name = widgets::label(&tag, "tree-name");
                    name.set_hexpand(true);
                    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    row.append(&name);
                    row.append(&widgets::label(&count.to_string(), "tree-count"));
                    let button = gtk::Button::new();
                    button.set_child(Some(&row));
                    button.add_css_class("nav-item");
                    button.set_tooltip_text(Some(&format!("Notes tagged #{tag}")));
                    let weak = Rc::downgrade(&ui);
                    button.connect_clicked(move |_| {
                        if let Some(ui) = weak.upgrade() {
                            ui.search.set_text(&format!("tag:{tag}"));
                            ui.show_search();
                        }
                    });
                    ui.tags.append(&button);
                }
            },
        );
    }
    fn populate(self: &Rc<Self>, dir: PathBuf, target: gtk::Box, depth: i32) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        let watch = dir.clone();
        let vault_root = root.clone();
        worker::run(
            // Each folder with the notes directly in it, for the tree's counts.
            move || {
                vault::children_all(&root, &dir).map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| {
                            let notes = if entry.directory {
                                std::fs::read_dir(&entry.path)
                                    .map(|items| {
                                        items
                                            .flatten()
                                            .filter(|item| vault::is_markdown(&item.path()))
                                            .count()
                                    })
                                    .unwrap_or(0)
                            } else {
                                0
                            };
                            (entry, notes)
                        })
                        .collect::<Vec<_>>()
                })
            },
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                if ui.generation.get() != generation
                    || ui.allow_close.get()
                    || target.root().is_none()
                {
                    return;
                }
                match result {
                    Err(e) => ui.message(&format!("Could not list folder: {e:#}")),
                    Ok(entries) => {
                        let remembered = prefs::get()
                            .expanded_folders
                            .get(&vault_root)
                            .cloned()
                            .unwrap_or_default();
                        for (entry, notes) in entries {
                            if entry.directory {
                                let row = widgets::hbox(8);
                                row.add_css_class("tree-folder");
                                row.append(&gtk::Image::from_icon_name("folder-symbolic"));
                                let name = widgets::label(
                                    &entry.path.file_name().unwrap_or_default().to_string_lossy(),
                                    "tree-name",
                                );
                                name.set_hexpand(true);
                                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                                row.append(&name);
                                if notes > 0 {
                                    row.append(&widgets::label(&notes.to_string(), "tree-count"));
                                }
                                let expander = gtk::Expander::builder().label_widget(&row).build();
                                expander.add_css_class("tree-expander");
                                // Nested rows sit inside their parent, which is already
                                // indented: each level adds one step.
                                expander.set_margin_start(if depth > 0 { INDENT } else { 0 });
                                let children = widgets::vbox(0);
                                expander.set_child(Some(&children));
                                let loaded = Rc::new(Cell::new(false));
                                let weak = Rc::downgrade(&ui);
                                let path = entry.path.clone();
                                let relative =
                                    entry.path.strip_prefix(&vault_root).unwrap().to_owned();
                                let restore = remembered.contains(&relative);
                                let root = vault_root.clone();
                                expander.connect_expanded_notify(move |e| {
                                    let Some(ui) = weak.upgrade() else {
                                        return;
                                    };
                                    if ui.allow_close.get()
                                        || ui.generation.get() != generation
                                        || ui.root.borrow().as_ref() != Some(&root)
                                        || e.root().is_none()
                                    {
                                        return;
                                    }
                                    let expanded = e.is_expanded();
                                    let saved = prefs::get()
                                        .expanded_folders
                                        .get(&root)
                                        .is_some_and(|folders| folders.contains(&relative));
                                    if saved != expanded {
                                        prefs::update(|p| {
                                            let folders =
                                                p.expanded_folders.entry(root.clone()).or_default();
                                            if expanded {
                                                folders.insert(relative.clone());
                                            } else {
                                                folders.remove(&relative);
                                            }
                                        });
                                    }
                                    if expanded && !loaded.replace(true) {
                                        ui.populate(path.clone(), children.clone(), depth + 1);
                                    }
                                });
                                ui.explorer_context(&expander, entry.path.clone(), true);
                                target.append(&expander);
                                expander.set_expanded(restore);
                            } else {
                                // Notes by their name; other files keep their extension.
                                let name = if vault::is_markdown(&entry.path) {
                                    entry.path.file_stem()
                                } else {
                                    entry.path.file_name()
                                }
                                .unwrap_or_default()
                                .to_string_lossy();
                                let row = widgets::hbox(8);
                                row.add_css_class("tree-note");
                                row.append(&gtk::Image::from_icon_name(if entry.symlink {
                                    "emblem-symbolic-link-symbolic"
                                } else {
                                    "text-x-generic-symbolic"
                                }));
                                let label = widgets::label(&name, "tree-name");
                                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                                label.set_hexpand(true);
                                row.append(&label);
                                let button = gtk::Button::new();
                                button.set_child(Some(&row));
                                button.add_css_class("nav-item");
                                button.add_css_class("tree-item");
                                // Under its folder's name, past the folder's arrow.
                                button.set_margin_start(if depth > 0 { INDENT } else { 0 });
                                if ui.active().is_some_and(|d| d.path == entry.path) {
                                    button.add_css_class("active");
                                }
                                ui.tree_notes
                                    .borrow_mut()
                                    .insert(entry.path.clone(), button.downgrade());
                                button.set_tooltip_text(Some(&entry.path.display().to_string()));
                                let weak = Rc::downgrade(&ui);
                                let open_path = entry.path.clone();
                                button.connect_clicked(move |_| {
                                    if let Some(ui) = weak.upgrade() {
                                        if vault::is_markdown(&open_path) {
                                            ui.open_file(open_path.clone());
                                        } else {
                                            ui.open_attachment(open_path.clone());
                                        }
                                    }
                                });
                                ui.explorer_context(&button, entry.path.clone(), false);
                                target.append(&button);
                            }
                        }
                    }
                }
                ui.watch_directory(watch, target, depth);
            },
        );
    }
    fn watch_directory(self: &Rc<Self>, path: PathBuf, target: gtk::Box, depth: i32) {
        if self.monitors.borrow().contains_key(&path) {
            return;
        }
        let Ok(monitor) = gio::File::for_path(&path)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        let pending = Rc::new(Cell::new(false));
        let watch = path.clone();
        let generation = self.generation.get();
        monitor.connect_changed(move |_, file, other, event| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.generation.get() != generation {
                return;
            }
            let structural = matches!(
                event,
                gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::Renamed
                    | gio::FileMonitorEvent::MovedIn
                    | gio::FileMonitorEvent::MovedOut
            );
            if structural && !pending.replace(true) {
                let weak = Rc::downgrade(&ui);
                let pending = pending.clone();
                let path = watch.clone();
                let target = target.clone();
                glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    pending.set(false);
                    if let Some(ui) = weak.upgrade()
                        && ui.generation.get() == generation
                    {
                        clear(&target);
                        ui.populate(path, target, depth);
                        ui.rebuild();
                    }
                });
            }
            if !matches!(
                event,
                gio::FileMonitorEvent::Changed | gio::FileMonitorEvent::AttributeChanged
            ) {
                for file in [Some(file), other].into_iter().flatten() {
                    if let Some(path) = file.path()
                        && vault::is_markdown(&path)
                    {
                        ui.update_index(path);
                    }
                }
            }
        });
        self.monitors.borrow_mut().insert(path, monitor);
    }
    fn refresh(self: &Rc<Self>) {
        self.monitors.borrow_mut().clear();
        self.populate_root();
        self.rebuild();
    }
    fn new_note(self: &Rc<Self>) {
        self.create(false);
    }
    fn new_folder(self: &Rc<Self>) {
        self.create(true);
    }
    fn create(self: &Rc<Self>, directory: bool) {
        self.create_in(directory, None);
    }
    fn create_in(self: &Rc<Self>, directory: bool, folder: Option<PathBuf>) {
        let Some(root) = self.root.borrow().clone() else {
            self.choose_vault();
            return;
        };
        let dialog = Prompt::new(
            &self.window,
            if directory { "New folder" } else { "New note" },
            "",
            "",
            420,
            140,
        );
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Create", gtk::ResponseType::Accept);
        let entry = gtk::Entry::builder()
            .placeholder_text("Name")
            .activates_default(true)
            .build();
        dialog.content_area().append(&entry);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                let root = root.clone();
                let name = folder
                    .as_ref()
                    .and_then(|f| f.strip_prefix(&root).ok())
                    .map_or_else(
                        || entry.text().to_string(),
                        |f| f.join(entry.text().as_str()).to_string_lossy().into_owned(),
                    );
                let weak = Rc::downgrade(&ui);
                worker::run(
                    move || {
                        let relative = std::path::Path::new(&name);
                        if relative.is_absolute() {
                            anyhow::bail!("Use a vault-relative path");
                        }
                        let parent = relative.parent().unwrap_or(std::path::Path::new(""));
                        let folder = vault::workspace::directory(&root, &parent.to_string_lossy())?;
                        let leaf = relative
                            .file_name()
                            .context("Enter a name")?
                            .to_string_lossy();
                        vault::create(&root, &folder, &leaf, directory)
                    },
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(path) => {
                                    ui.refresh();
                                    if !directory {
                                        ui.open_file(path);
                                    }
                                }
                                Err(e) => ui.message(&format!("Could not create: {e:#}")),
                            }
                        }
                    },
                );
            }
            dialog.close();
        });
        dialog.present();
    }
    fn open_file(self: &Rc<Self>, path: PathBuf) {
        if path.extension().is_some_and(|e| e == "canvas") {
            self.open_canvas(path);
            return;
        }
        if let Some(doc) = self.docs.borrow().iter().find(|d| d.path == path).cloned() {
            let notebook = self.doc_notebook(&doc);
            self.active_pane.set(notebook == &self.secondary);
            notebook.set_current_page(notebook.page_num(&doc.editor.root));
            self.update_active();
            return;
        }
        if self.opening.borrow().contains(&path) {
            return;
        }
        self.opening.borrow_mut().push(path.clone());
        let state = nyx_core::paths::state_dir();
        let requested = path.clone();
        let weak = Rc::downgrade(self);
        worker::run(
            move || vault::open_with_recovery(&state, &path),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                ui.opening.borrow_mut().retain(|p| p != &requested);
                match result {
                    Err(e) => ui.message(&format!("Could not open {}: {e:#}", requested.display())),
                    Ok((loaded, draft)) => ui.add_document(loaded, draft),
                }
            },
        );
    }
    fn add_document(self: &Rc<Self>, loaded: Loaded, draft: Option<Draft>) {
        let editor = Editor::new(&loaded.text);
        self.configure_editor(&editor, &loaded.path);
        let weak = Rc::downgrade(self);
        let mine = Rc::downgrade(&editor);
        editor.buffer.connect_cursor_position_notify(move |_| {
            if let (Some(ui), Some(editor)) = (weak.upgrade(), mine.upgrade())
                && ui.active().is_some_and(|d| Rc::ptr_eq(&d.editor, &editor))
            {
                ui.update_status();
            }
        });
        editor.set_typewriter(prefs::get().typewriter);
        let restored = self.pending_cursors.borrow_mut().remove(&loaded.path);
        if let Some(state) = restored.as_ref() {
            let count = editor.buffer.char_count();
            if let Some((a, b)) = state.selection {
                editor.buffer.select_range(
                    &editor.buffer.iter_at_offset(a.clamp(0, count)),
                    &editor.buffer.iter_at_offset(b.clamp(0, count)),
                );
            } else {
                editor
                    .buffer
                    .place_cursor(&editor.buffer.iter_at_offset(state.cursor.clamp(0, count)));
            }
        }
        editor.set_mode(if self.reading.is_active() {
            notes_editor::Mode::Reading
        } else if self.live.is_active() {
            notes_editor::Mode::Live
        } else {
            notes_editor::Mode::Source
        });
        editor.set_writable(!loaded.read_only);
        if let Some(state) = restored {
            editor.restore_scroll(state.scroll);
        }
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        let tab = widgets::hbox(6);
        tab.add_css_class("note-tab");
        let icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
        icon.add_css_class("note-tab-icon");
        tab.append(&icon);
        tab.append(&label);
        let close = widgets::bar_button("window-close-symbolic", "Close note (Ctrl+W)");
        close.add_css_class("note-tab-close");
        tab.append(&close);
        tab.set_hexpand(false);
        tab.set_focusable(true);
        let doc = Rc::new(Doc {
            path: loaded.path,
            editor,
            version: RefCell::new(loaded.version),
            bom: Cell::new(loaded.bom),
            read_only: loaded.read_only,
            loading: Cell::new(false),
            closed: Cell::new(false),
            recovery_pending: Cell::new(false),
            dirty: Cell::new(false),
            saving: Cell::new(false),
            conflict: Cell::new(false),
            label,
            save_timer: RefCell::new(None),
            recovery_timer: RefCell::new(None),
            _monitor: RefCell::new(None),
        });
        tab.set_tooltip_text(Some(&doc.path.to_string_lossy()));
        self.tab_context(&tab, doc.clone());
        self.editor_context(&doc);
        self.docs.borrow_mut().push(doc.clone());
        if let Some(right) = self.pending_panes.borrow_mut().remove(&doc.path) {
            self.active_pane.set(right);
        }
        let notebook = self.current_notebook();
        notebook.append_page(&doc.editor.root, Some(&tab));
        notebook.set_tab_reorderable(&doc.editor.root, true);
        notebook.set_tab_detachable(&doc.editor.root, true);
        notebook.set_current_page(notebook.page_num(&doc.editor.root));
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        let weak_doc = Rc::downgrade(&doc);
        focus.connect_enter(move |_| {
            if let (Some(ui), Some(doc)) = (weak.upgrade(), weak_doc.upgrade()) {
                ui.active_pane
                    .set(ui.secondary.page_num(&doc.editor.root).is_some());
                ui.update_active();
            }
        });
        doc.editor.root.add_controller(focus);
        let weak = Rc::downgrade(self);
        let d = Rc::downgrade(&doc);
        close.connect_clicked(move |_| {
            if let (Some(ui), Some(doc)) = (weak.upgrade(), d.upgrade()) {
                ui.close_tab(doc);
            }
        });
        let weak = Rc::downgrade(self);
        let d = Rc::downgrade(&doc);
        doc.editor.buffer.connect_changed(move |_| {
            if let (Some(ui), Some(doc)) = (weak.upgrade(), d.upgrade()) {
                if doc.loading.get() || doc.closed.get() {
                    return;
                }
                doc.dirty.set(true);
                ui.label(&doc);
                ui.changed(doc);
            }
        });
        let weak = Rc::downgrade(self);
        doc.editor.on_parsed(move |_| {
            if let Some(ui) = weak.upgrade() {
                let weak = Rc::downgrade(&ui);
                glib::idle_add_local_once(move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.update_active();
                        if let Some(doc) = ui.active() {
                            ui.apply_pending_jump(&doc);
                        }
                    }
                });
            }
        });
        if let Some(parent) = doc.path.parent()
            && let Ok(monitor) = gio::File::for_path(parent)
                .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            let weak = Rc::downgrade(self);
            let d = Rc::downgrade(&doc);
            monitor.connect_changed(move |_, file, other, _| {
                if let (Some(ui), Some(doc)) = (weak.upgrade(), d.upgrade())
                    && [Some(file), other]
                        .into_iter()
                        .flatten()
                        .any(|f| f.path().as_ref() == Some(&doc.path))
                {
                    ui.external_check(doc);
                }
            });
            *doc._monitor.borrow_mut() = Some(monitor);
        }
        self.label(&doc);
        self.apply_typography();
        self.update_active();
        self.save_session();
        if doc.read_only {
            self.message(
                "This note is read-only. Symbolic links are not edited in place; use Save a copy.",
            );
        }
        #[cfg(feature = "ai")]
        self.apply_pending_proposal(doc.clone());
        if let Some(draft) = draft {
            if draft.text != doc.editor.text() {
                self.offer_draft(doc, draft);
            } else {
                self.remove_recovery(doc.path.clone());
            }
        }
    }
    fn label(&self, doc: &Doc) {
        let name = doc.path.file_stem().unwrap_or_default().to_string_lossy();
        doc.label
            .set_tooltip_text(Some(&doc.path.to_string_lossy()));
        doc.label.set_text(&format!(
            "{}{}{}",
            if doc.conflict.get() { "⚠ " } else { "" },
            name,
            if doc.dirty.get() { " •" } else { "" }
        ));
        // A tab fits its name; only long names are cut, at 26 characters.
        let long = doc.label.text().chars().count() > 26;
        doc.label.set_ellipsize(if long {
            gtk::pango::EllipsizeMode::Middle
        } else {
            gtk::pango::EllipsizeMode::None
        });
        doc.label.set_max_width_chars(if long { 26 } else { -1 });
    }
    fn changed(self: &Rc<Self>, doc: Document) {
        if doc.closed.get() || doc.recovery_pending.get() {
            return;
        }
        if let Some(timer) = doc.recovery_timer.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        let d = Rc::downgrade(&doc);
        *doc.recovery_timer.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(250),
            move || {
                if let (Some(ui), Some(doc)) = (weak.upgrade(), d.upgrade()) {
                    doc.recovery_timer.borrow_mut().take();
                    ui.persist_draft(&doc);
                }
            },
        ));
        if let Some(timer) = doc.save_timer.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        let d = Rc::downgrade(&doc);
        *doc.save_timer.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_secs(1),
            move || {
                if let (Some(ui), Some(doc)) = (weak.upgrade(), d.upgrade()) {
                    doc.save_timer.borrow_mut().take();
                    ui.save(doc, None);
                }
            },
        ));
    }
    fn draft(&self, doc: &Doc) -> Draft {
        Draft {
            path: doc.path.clone(),
            text: doc.editor.text(),
            bom: doc.bom.get(),
            base_etag: doc.version.borrow().etag.clone(),
            base_digest: doc.version.borrow().digest.to_hex().to_string(),
        }
    }
    fn persist_draft(self: &Rc<Self>, doc: &Doc) {
        if doc.recovery_pending.get() {
            return;
        }
        let state = nyx_core::paths::state_dir();
        let draft = self.draft(doc);
        let weak = Rc::downgrade(self);
        worker::run(
            move || vault::write_draft(&state, &draft),
            move |result| {
                if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                    ui.message(&format!("Recovery could not be saved: {e:#}"));
                }
            },
        );
    }
    fn remove_recovery(self: &Rc<Self>, path: PathBuf) {
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || vault::remove_draft(&state, &path),
            move |result| {
                if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                    ui.message(&format!("Could not remove old recovery draft: {e:#}"));
                }
            },
        );
    }
    fn save_active(self: &Rc<Self>) {
        if let Some(doc) = self.active() {
            self.save(doc, None);
        }
    }
    fn save(self: &Rc<Self>, doc: Document, done: Option<Box<dyn FnOnce(bool)>>) {
        if !doc.dirty.get() {
            if let Some(done) = done {
                done(true);
            }
            return;
        }
        if doc.closed.get()
            || doc.recovery_pending.get()
            || doc.saving.get()
            || doc.read_only
            || doc.conflict.get()
        {
            if let Some(done) = done {
                done(false);
            }
            if doc.conflict.get() {
                self.conflict_message(
                    &doc,
                    "This note changed outside Notes. Choose Resolve to compare or save a copy.",
                );
            }
            return;
        }
        doc.saving.set(true);
        let revision = doc.editor.revision.get();
        let draft = self.draft(&doc);
        let expected = doc.version.borrow().clone();
        let history_prefs = prefs::get();
        let state = nyx_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || {
                vault::write_draft(&state, &draft).map_err(vault::SaveError::Failure)?;
                if history_prefs.history_keep > 0 {
                    let disk = vault::load(&draft.path).map_err(vault::SaveError::Failure)?;
                    vault::history::capture(
                        &state,
                        &draft.path,
                        &disk.text,
                        disk.bom,
                        history_prefs.history_keep,
                        history_prefs.history_days,
                    )
                    .map_err(vault::SaveError::Failure)?;
                    vault::history::capture(
                        &state,
                        &draft.path,
                        &draft.text,
                        draft.bom,
                        history_prefs.history_keep,
                        history_prefs.history_days,
                    )
                    .map_err(vault::SaveError::Failure)?;
                }
                vault::save(&draft.path, &draft.text, draft.bom, &expected)
            },
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                doc.saving.set(false);
                let ok = match result {
                    Ok(version) => {
                        *doc.version.borrow_mut() = version;
                        if revision == doc.editor.revision.get() {
                            doc.dirty.set(false);
                            doc.editor.buffer.set_modified(false);
                            ui.remove_recovery(doc.path.clone());
                        } else {
                            ui.changed(doc.clone());
                        }
                        ui.update_index(doc.path.clone());
                        true
                    }
                    Err(e) => {
                        let message = format!(
                            "Could not save {}: {e}",
                            doc.path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        if matches!(e, vault::SaveError::Conflict) {
                            doc.conflict.set(true);
                            ui.conflict_message(&doc, &message);
                        } else {
                            ui.message(&message);
                        }
                        false
                    }
                };
                ui.label(&doc);
                ui.update_active();
                if let Some(done) = done {
                    done(ok && !doc.dirty.get());
                }
            },
        );
    }
    fn external_check(self: &Rc<Self>, doc: Document) {
        if self.mutating.get() {
            return;
        }
        if doc.saving.get() || doc.closed.get() || doc.recovery_pending.get() {
            return;
        }
        let path = doc.path.clone();
        let weak = Rc::downgrade(self);
        let revision = doc.editor.revision.get();
        worker::run(
            move || vault::load(&path),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                if doc.saving.get() || doc.closed.get() || doc.recovery_pending.get() {
                    return;
                }
                match result {
                    Ok(loaded) if loaded.version.digest == doc.version.borrow().digest => {
                        *doc.version.borrow_mut() = loaded.version;
                    }
                    Ok(loaded) if !doc.dirty.get() && revision == doc.editor.revision.get() => {
                        // Reopen externally edited text without creating an undo action into stale disk contents.
                        let cursor = doc.editor.buffer.cursor_position();
                        let adjustment = doc.editor.scroll.vadjustment();
                        let scroll = adjustment.value();
                        doc.loading.set(true);
                        doc.bom.set(loaded.bom);
                        doc.editor.buffer.begin_irreversible_action();
                        doc.editor.buffer.set_text(&loaded.text);
                        doc.editor.buffer.end_irreversible_action();
                        doc.loading.set(false);
                        doc.dirty.set(false);
                        doc.editor.buffer.set_modified(false);
                        *doc.version.borrow_mut() = loaded.version;
                        doc.conflict.set(false);
                        doc.editor.buffer.place_cursor(
                            &doc.editor
                                .buffer
                                .iter_at_offset(cursor.min(doc.editor.buffer.char_count())),
                        );
                        adjustment.set_value(scroll);
                        ui.remove_recovery(doc.path.clone());
                    }
                    _ => {
                        doc.conflict.set(true);
                        ui.persist_draft(&doc);
                        ui.conflict_message(&doc,"A note changed or was removed outside Notes. Local edits are retained. Choose Resolve.");
                    }
                }
                ui.label(&doc);
                ui.update_index(doc.path.clone());
            },
        );
    }
    fn conflict_message(&self, doc: &Document, text: &str) {
        self.message(text);
        *self.resolve_target.borrow_mut() = Some(doc.clone());
        self.resolve_button.set_visible(true);
        self.notice_row.add_css_class("conflict");
    }
    fn resolve_active(self: &Rc<Self>) {
        let target = self
            .resolve_target
            .borrow()
            .clone()
            .filter(|d| !d.closed.get() && d.conflict.get())
            .or_else(|| self.active().filter(|d| d.conflict.get()))
            .or_else(|| {
                self.docs
                    .borrow()
                    .iter()
                    .find(|d| d.conflict.get() && !d.closed.get())
                    .cloned()
            });
        let Some(doc) = target else {
            if self.root.borrow().is_none() {
                self.choose_vault();
            } else {
                self.message(
                    "There is no unresolved external change. Your open notes are available.",
                );
            }
            return;
        };
        let weak = Rc::downgrade(self);
        let path = doc.path.clone();
        worker::run(
            move || vault::load(&path),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                let dialog = Prompt::new(
                    &ui.window,
                    "Resolve external changes",
                    &doc.path.file_name().unwrap_or_default().to_string_lossy(),
                    "Compare your edits with the disk version. Save a copy to keep both.",
                    760,
                    500,
                );
                dialog.add_button("Cancel", gtk::ResponseType::Cancel);
                dialog.add_button("Save a copy", gtk::ResponseType::Other(1));
                if result.is_ok() {
                    dialog.add_button("Use disk version", gtk::ResponseType::Accept);
                }
                let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
                let reviewed_revision = doc.editor.revision.get();
                let mine = readonly_text(&doc.editor.text());
                panes.set_start_child(Some(&mine));
                let disk = readonly_text(
                    &result
                        .as_ref()
                        .map(|d| d.text.clone())
                        .unwrap_or_else(|e| format!("Disk note unavailable: {e:#}")),
                );
                panes.set_end_child(Some(&disk));
                panes.set_position(420);
                panes.set_vexpand(true);
                dialog.content_area().append(&widgets::label(
                    "Your edits                           Disk contents",
                    "dim",
                ));
                dialog.content_area().append(&panes);
                let weak = Rc::downgrade(&ui);
                dialog.connect_response(move |dialog, response| {
                    if let Some(ui) = weak.upgrade() {
                        if response == gtk::ResponseType::Other(1) {
                            ui.save_copy_doc(doc.clone());
                        } else if response == gtk::ResponseType::Accept
                            && let Ok(loaded) = &result
                        {
                            if doc.closed.get()||doc.editor.revision.get()!=reviewed_revision {
                                ui.conflict_message(&doc,"Your edits changed while this comparison was open. Choose Resolve for a fresh comparison.");
                                dialog.close();return;
                            }
                            doc.loading.set(true);
                            doc.recovery_pending.set(false);
                            doc.bom.set(loaded.bom);
                            doc.editor.set_writable(!doc.read_only);
                            doc.editor.buffer.begin_irreversible_action();
                            doc.editor.buffer.set_text(&loaded.text);
                            doc.editor.buffer.end_irreversible_action();
                            doc.loading.set(false);
                            *doc.version.borrow_mut() = loaded.version.clone();
                            doc.dirty.set(false);
                            doc.conflict.set(false);
                            doc.editor.buffer.set_modified(false);
                            ui.remove_recovery(doc.path.clone());
                            ui.label(&doc);
                            ui.notice_row.set_visible(false);
                            ui.resolve_target.borrow_mut().take();
                            ui.update_active();
                        }
                    }
                    dialog.close();
                });
                dialog.present();
            },
        );
    }
    fn save_copy(self: &Rc<Self>) {
        if let Some(doc) = self.active() {
            self.save_copy_doc(doc);
        }
    }
    fn save_copy_doc(self: &Rc<Self>, doc: Document) {
        let dialog = gtk::FileDialog::builder()
            .title("Save a copy")
            .initial_name(doc.path.file_name().unwrap_or_default().to_string_lossy())
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window),gio::Cancellable::NONE,move |result| {
            let (Some(ui),Ok(file))=(weak.upgrade(),result) else {return};
            let Some(path)=file.path() else {return};
            let text=doc.editor.text();let bom=doc.bom.get();let weak=Rc::downgrade(&ui);
            worker::run(move || {vault::save_copy(&path,&text,bom)?;Ok::<_,anyhow::Error>(path)},move |result| {
                if let Some(ui)=weak.upgrade() {match result {
                    Ok(path)=>{ui.message("Copy saved. The original note and recovery draft are retained.");ui.open_file(path);}
                    Err(e)=>ui.message(&format!("Could not save copy: {e:#}. Choose an unused filename.")),
                }}
            });
        });
    }
    fn close_tab(self: &Rc<Self>, doc: Document) {
        if doc.dirty.get() || doc.saving.get() {
            let dialog = Prompt::new(
                &self.window,
                "Close note",
                "Close this note?",
                "Save your edits, or keep a recovery draft for next time.",
                480,
                180,
            );
            dialog.add_button("Cancel", gtk::ResponseType::Cancel);
            dialog.add_button("Keep draft", gtk::ResponseType::Other(1));
            dialog.add_button("Save", gtk::ResponseType::Accept);
            let weak = Rc::downgrade(self);
            dialog.connect_response(move |dialog, response| {
                if let Some(ui) = weak.upgrade() {
                    match response {
                        gtk::ResponseType::Accept => {
                            let weak = Rc::downgrade(&ui);
                            let d = doc.clone();
                            ui.save(
                                doc.clone(),
                                Some(Box::new(move |ok| {
                                    if ok && let Some(ui) = weak.upgrade() {
                                        ui.remove_tab(d);
                                    }
                                })),
                            );
                        }
                        gtk::ResponseType::Other(1) => {
                            ui.persist_draft(&doc);
                            ui.remove_tab(doc.clone());
                        }
                        _ => {}
                    }
                }
                dialog.close();
            });
            dialog.present();
        } else {
            self.remove_tab(doc);
        }
    }
    fn remove_tab(self: &Rc<Self>, doc: Document) {
        doc.closed.set(true);
        if let Some(timer) = doc.save_timer.borrow_mut().take() {
            timer.remove();
        }
        if let Some(timer) = doc.recovery_timer.borrow_mut().take() {
            timer.remove();
        }
        let notebook = self.doc_notebook(&doc);
        notebook.remove_page(notebook.page_num(&doc.editor.root));
        self.docs.borrow_mut().retain(|d| !Rc::ptr_eq(d, &doc));
        self.save_session();
        self.update_active();
    }
    fn close_window(self: &Rc<Self>) {
        if self.allow_close.get() {
            return;
        }
        self.closing.set(true);
        if self.close_wait.get() {
            return;
        }
        if let Some(prompt) = self
            .close_prompt
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
            .filter(|w| w.is_visible())
        {
            prompt.present();
            return;
        }
        // Workspace windows can own unsaved canvases and keep GtkApplication alive.
        // Close one at a time; their destroy callback resumes this request.
        if let Some(window) = self
            .window
            .application()
            .and_then(|a| a.windows().into_iter().find(|w| w != &self.window))
        {
            window.close();
            return;
        }
        if self.docs.borrow().iter().any(|d| d.saving.get()) {
            self.close_wait.set(true);
            let weak = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(30), move || {
                let Some(ui) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if !ui.closing.get() {
                    ui.close_wait.set(false);
                    return glib::ControlFlow::Break;
                }
                if ui.docs.borrow().iter().any(|d| d.saving.get()) {
                    return glib::ControlFlow::Continue;
                }
                ui.close_wait.set(false);
                ui.close_window();
                glib::ControlFlow::Break
            });
            return;
        }
        let dirty = self.docs.borrow().iter().any(|d| d.dirty.get());
        if !dirty {
            self.finish_close();
            return;
        }
        let dialog = Prompt::new(
            &self.window,
            "Close Notes",
            "Save notes before closing?",
            "Keep recovery drafts to resume unsaved work next time.",
            480,
            180,
        );
        *self.close_prompt.borrow_mut() = Some(dialog.window.downgrade());
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Keep drafts", gtk::ResponseType::Other(1));
        dialog.add_button("Save all", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog,response|{
            if let Some(ui)=weak.upgrade(){
                ui.close_prompt.borrow_mut().take();
                match response {
                    gtk::ResponseType::Other(1)=>{
                        let documents=ui.docs.borrow().iter().filter(|d|d.dirty.get()).cloned().collect::<Vec<_>>();
                        let snapshots=documents.iter().map(|d|(d.clone(),d.editor.revision.get())).collect::<Vec<_>>();
                        let drafts=documents.iter().map(|d|ui.draft(d)).collect::<Vec<_>>();let state=nyx_core::paths::state_dir();let weak=Rc::downgrade(&ui);ui.close_wait.set(true);
                        worker::run(move ||->anyhow::Result<()>{for draft in drafts{vault::write_draft(&state,&draft)?;}Ok(())},move |result|{if let Some(ui)=weak.upgrade(){ui.close_wait.set(false);match result{Ok(()) if snapshots.iter().all(|(d,r)|d.editor.revision.get()==*r)=>ui.finish_close(),Ok(())=>ui.close_window(),Err(e)=>{ui.closing.set(false);ui.message(&format!("Could not preserve recovery drafts: {e:#}. Notes remains open."));}}}});
                    }
                    gtk::ResponseType::Accept=>ui.save_all_then_close(),
                    _=>ui.closing.set(false),
                }
            }
            dialog.close();
        });
        dialog.present();
    }
    fn save_all_then_close(self: &Rc<Self>) {
        if self.docs.borrow().iter().any(|d| d.saving.get()) {
            self.close_window();
            return;
        }
        let next = self.docs.borrow().iter().find(|d| d.dirty.get()).cloned();
        if let Some(doc) = next {
            let weak = Rc::downgrade(self);
            self.save(doc,Some(Box::new(move |ok|{if let Some(ui)=weak.upgrade(){if ok{ui.save_all_then_close();}else{ui.closing.set(false);ui.message("Notes remains open because a note could not be saved. Resolve it or keep recovery drafts.");}}})));
        } else {
            self.finish_close();
        }
    }
    fn finish_close(&self) {
        for doc in self.docs.borrow().iter() {
            doc.closed.set(true);
            if let Some(timer) = doc.save_timer.borrow_mut().take() {
                timer.remove();
            }
            if let Some(timer) = doc.recovery_timer.borrow_mut().take() {
                timer.remove();
            }
        }
        self.save_session();
        prefs::flush();
        self.allow_close.set(true);
        // Calling close() recursively from close-request is ignored by GTK.
        // The original handler inhibits its default close, so destroy directly.
        self.window.destroy();
    }
    fn save_session(&self) {
        let files = self.docs.borrow().iter().map(|d| d.path.clone()).collect();
        let split_files = self
            .docs
            .borrow()
            .iter()
            .filter(|d| self.secondary.page_num(&d.editor.root).is_some())
            .map(|d| d.path.clone())
            .collect();
        prefs::update(|p| {
            p.open_files = files;
            p.split_files = split_files;
        });
    }
    fn offer_recovery(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let dialog = Prompt::new(
            &self.window,
            "Recovery drafts",
            "Recovery drafts are available",
            &format!(
                "{} note(s) have retained drafts. Review them before resuming editing.",
                paths.len()
            ),
            480,
            180,
        );
        dialog.add_button("Later", gtk::ResponseType::Cancel);
        dialog.add_button("Review drafts", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept
                && let Some(ui) = weak.upgrade()
            {
                for path in &paths {
                    ui.open_file(path.clone());
                }
            }
            dialog.close();
        });
        dialog.present();
    }
    fn offer_draft(self: &Rc<Self>, doc: Document, draft: Draft) {
        doc.recovery_pending.set(true);
        doc.editor.set_writable(false);
        doc.conflict.set(true);
        self.label(&doc);
        let dialog = Prompt::new(
            &self.window,
            "Recover note",
            "Recover unsaved changes?",
            &doc.path.display().to_string(),
            480,
            180,
        );
        dialog.add_button("Keep for later", gtk::ResponseType::Cancel);
        dialog.add_button("Discard draft", gtk::ResponseType::Reject);
        dialog.add_button("Recover", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog,response| {
            if let Some(ui)=weak.upgrade() {
                match response {
                    gtk::ResponseType::Accept => {
                        doc.recovery_pending.set(false);
                        doc.editor.set_writable(!doc.read_only);
                        let changed=draft.base_digest!=doc.version.borrow().digest.to_hex().to_string();
                        doc.bom.set(draft.bom);
                        doc.editor.buffer.set_text(&draft.text);
                        doc.conflict.set(changed);
                        doc.dirty.set(true);
                        ui.label(&doc);
                        if changed {ui.conflict_message(&doc,"Draft recovered. Disk contents also changed; choose Resolve before saving.");}
                        else {ui.message("Draft recovered. Autosave will check the disk version before saving.");}
                    }
                    gtk::ResponseType::Reject => {
                        doc.recovery_pending.set(false);
                        doc.editor.set_writable(!doc.read_only);
                        doc.conflict.set(false);
                        ui.remove_recovery(doc.path.clone());ui.label(&doc);
                    }
                    _ => {
                        ui.remove_tab(doc.clone());
                        ui.message("The recovery draft is retained for the next time you open this note.");
                    }
                }
            }
            dialog.close();
        });
        dialog.present();
    }
    /// The foot's left side: words, reading time and the cursor's line and column.
    fn update_status(&self) {
        let Some(doc) = self.active() else { return };
        let (words, _) = doc.editor.statistics();
        let cursor = doc.editor.buffer.iter_at_offset(doc.editor.buffer.cursor_position());
        self.status.set_text(&format!(
            "{} words\u{2003}{} min read\u{2003}Ln {}, Col {}",
            thousands(words),
            words.div_ceil(200).max(1),
            cursor.line() + 1,
            cursor.line_offset() + 1
        ));
    }
    fn update_active(self: &Rc<Self>) {
        if let Some(doc) = self.active() {
            let folder = self
                .root
                .borrow()
                .as_ref()
                .and_then(|root| {
                    doc.path
                        .parent()?
                        .strip_prefix(root)
                        .ok()
                        .map(Path::to_path_buf)
                })
                .unwrap_or_default();
            self.doc_title.set(&folder, &doc.path, doc.dirty.get());
            self.update_status();
            self.save_state.set_text(if doc.read_only {
                "Read-only"
            } else if doc.conflict.get() {
                "Conflict"
            } else if doc.dirty.get() {
                "Unsaved"
            } else {
                "Saved"
            });
            for (path, button) in self.tree_notes.borrow().iter() {
                if let Some(button) = button.upgrade() {
                    if *path == doc.path {
                        button.add_css_class("active");
                    } else {
                        button.remove_css_class("active");
                    }
                }
            }
            let key = (doc.path.clone(), doc.editor.parsed_revision());
            if self.outline_key.borrow().as_ref() != Some(&key) {
                *self.outline_key.borrow_mut() = Some(key);
                let headings = doc.editor.outline();
                replace_strings(
                    &self.outline_model,
                    headings.iter().map(|(text, _, level)| {
                        format!("{}{text}", "\u{2003}".repeat(level.saturating_sub(1) as usize))
                    }),
                );
                *self.outline_offsets.borrow_mut() =
                    headings.into_iter().map(|(_, offset, _)| offset).collect();
                let outgoing = doc.editor.links();
                replace_strings(
                    &self.outgoing_model,
                    outgoing.iter().map(|link| {
                        format!(
                            "{}{}",
                            if link.embed { "Embed · " } else { "" },
                            link.target
                        )
                    }),
                );
                *self.outgoing.borrow_mut() = outgoing;
                let metadata = doc.editor.metadata();
                let mut properties = metadata
                    .properties
                    .into_iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect::<Vec<_>>();
                if let Some(error) = metadata.error {
                    properties.insert(0, error);
                }
                if properties.is_empty() {
                    properties.push("No frontmatter properties".into());
                }
                replace_strings(&self.properties_model, properties);
                let tags = doc.editor.tags();
                replace_strings(&self.tags_model, tags.iter().map(|tag| format!("#{tag}")));
                *self.tag_items.borrow_mut() = tags;
                self.apply_pending_jump(&doc);
            }
            if self.backlink_key.borrow().as_ref() != Some(&doc.path) {
                *self.backlink_key.borrow_mut() = Some(doc.path.clone());
                self.refresh_backlinks();
            }
        } else {
            self.doc_title.set(Path::new(""), Path::new(""), false);
            self.status.set_text("Open a note from the vault");
            self.save_state.set_text("");
            for model in [
                &self.outline_model,
                &self.outgoing_model,
                &self.backlinks_model,
                &self.properties_model,
                &self.tags_model,
            ] {
                model.splice(0, model.n_items(), &[]);
            }
            *self.outline_key.borrow_mut() = None;
            *self.backlink_key.borrow_mut() = None;
            self.outline_offsets.borrow_mut().clear();
            self.outgoing.borrow_mut().clear();
            self.backlink_items.borrow_mut().clear();
            self.tag_items.borrow_mut().clear();
        }
    }
    fn refresh_backlinks(self: &Rc<Self>) {
        let (Some(db), Some(root), Some(doc)) = (
            self.db.borrow().clone(),
            self.root.borrow().clone(),
            self.active(),
        ) else {
            return;
        };
        if !doc.path.starts_with(&root) {
            replace_strings(
                &self.backlinks_model,
                ["Select this note's vault to view backlinks".to_owned()],
            );
            return;
        }
        let request = self.backlink_request.get() + 1;
        self.backlink_request.set(request);
        let path = doc.path.clone();
        let expected = path.clone();
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        worker::background(
            move || {
                let links = vault::backlinks(&db, &root, &path)?;
                let mentions = vault::mentions(&db, &root, &path)?;
                // Each link's line, for its card.
                let snippets = links
                    .iter()
                    .map(|link| {
                        std::fs::read_to_string(&link.path)
                            .ok()
                            .and_then(|text| link_line(&text, link.offset))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>();
                Ok::<_, anyhow::Error>((links, mentions, snippets))
            },
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                if ui.backlink_request.get() != request
                    || ui.generation.get() != generation
                    || !ui.active().is_some_and(|d| d.path == expected)
                {
                    return;
                }
                match result {
                    Ok((links, mentions, snippets)) => {
                        replace_strings(
                            &ui.mentions_model,
                            mentions.iter().map(|hit| {
                                hit.path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            }),
                        );
                        if mentions.is_empty() {
                            replace_strings(&ui.mentions_model, ["No unlinked mentions".into()]);
                        }
                        *ui.mention_items.borrow_mut() = mentions;
                        let root = ui.root.borrow().clone().unwrap_or_default();
                        replace_strings(
                            &ui.backlinks_model,
                            links.iter().zip(&snippets).map(|(link, snippet)| {
                                format!(
                                    "{}{}\u{1f}{snippet}",
                                    if link.embed { "Embed · " } else { "" },
                                    link.path
                                        .strip_prefix(&root)
                                        .unwrap_or(&link.path)
                                        .with_extension("")
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                )
                            }),
                        );
                        if let Some(heading) = &ui.backlinks_heading {
                            heading.set_text(&format!("BACKLINKS · {}", links.len()));
                        }
                        if links.is_empty() {
                            replace_strings(&ui.backlinks_model, ["No backlinks".to_owned()]);
                        }
                        *ui.backlink_items.borrow_mut() = links;
                    }
                    Err(e) => {
                        ui.backlink_items.borrow_mut().clear();
                        replace_strings(
                            &ui.backlinks_model,
                            [format!("Backlinks unavailable: {e:#}")],
                        );
                    }
                }
            },
        );
    }
    fn follow_link(self: &Rc<Self>, link: notes_editor::Link) {
        if let Some(doc) = self.active() {
            self.open_target(doc.path.clone(), link.target, link.wiki);
        }
    }
    fn apply_pending_jump(&self, doc: &Doc) {
        if doc.editor.parsed_revision() != doc.editor.revision.get() {
            return;
        }
        let jump = self.pending_jumps.borrow_mut().remove(&doc.path);
        match jump {
            Some(Jump::Offset(offset)) => doc.editor.jump_to(offset),
            Some(Jump::Reference(reference)) if !doc.editor.jump_to_reference(&reference) => {
                self.message(&format!("Reference not found: {reference}"));
            }
            _ => {}
        }
    }
    fn rebuild(self: &Rc<Self>) {
        if self.indexing.replace(true) {
            self.reindex_needed.set(true);
            return;
        }
        let (Some(root), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone()) else {
            self.indexing.set(false);
            return;
        };
        let generation = self.generation.get();
        self.status.set_text("Indexing vault…");
        let weak = Rc::downgrade(self);
        worker::background(
            move || vault::reconcile_index(&db, &root),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                ui.indexing.set(false);
                if ui.generation.get() == generation {
                    match result {
                        Ok((count, skipped)) => {
                            ui.export_launcher_index();
                            ui.populate_tags();
                            #[cfg(feature = "ai")]
                            ui.reconcile_semantic();
                            for doc in ui.docs.borrow().iter() {
                                doc.editor.refresh_reading();
                            }
                            if ui.active().is_some() {
                                ui.update_active();
                                ui.refresh_backlinks();
                            } else {
                                ui.status.set_text(&format!("{count} notes indexed"));
                            }
                            if skipped > 0 {
                                ui.message(&format!("{skipped} unreadable or unsupported files were skipped during indexing."));
                            }
                        }
                        Err(e) => ui.message(&format!("Search indexing failed: {e:#}")),
                    }
                    if !ui.search.text().is_empty() {
                        ui.search_vault();
                    }
                }
                if ui.reindex_needed.replace(false) {
                    ui.rebuild();
                }
            },
        );
    }
    fn update_index(self: &Rc<Self>, path: PathBuf) {
        let Some(db) = self.db.borrow().clone() else {
            return;
        };
        if !self
            .root
            .borrow()
            .as_ref()
            .is_some_and(|root| path.starts_with(root))
        {
            return;
        }
        let weak = Rc::downgrade(self);
        worker::background(
            move || vault::update_index(&db, &path),
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(()) => ui.refresh_backlinks(),
                        Err(e) => ui.message(&format!("Search index update failed: {e:#}")),
                    }
                }
            },
        );
    }
    fn show_search(&self) {
        self.search.grab_focus();
    }
    fn search_vault(self: &Rc<Self>) {
        let Some(db) = self.db.borrow().clone() else {
            return;
        };
        let query = self.search.text().to_string();
        let requested = query.clone();
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        worker::background(
            move || vault::search(&db, &query),
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                if ui.generation.get() != generation || ui.search.text() != requested {
                    return;
                }
                clear(&ui.results);
                match result {
                    Err(e) => ui.message(&format!("Search failed: {e:#}")),
                    Ok(hits) => {
                        if hits.is_empty() {
                            ui.results
                                .append(&widgets::label("No matching notes", "dim"));
                        }
                        for hit in hits {
                            let button = gtk::Button::new();
                            let content = widgets::vbox(2);
                            let title = gtk::Label::new(Some(
                                &hit.path.file_name().unwrap_or_default().to_string_lossy(),
                            ));
                            title.set_xalign(0.0);
                            let snippet = gtk::Label::new(Some(&hit.snippet.replace('\n', " ")));
                            snippet.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            snippet.set_xalign(0.0);
                            snippet.add_css_class("dim");
                            content.append(&title);
                            content.append(&snippet);
                            button.set_child(Some(&content));
                            let weak = Rc::downgrade(&ui);
                            button.connect_clicked(move |_| {
                                if let Some(ui) = weak.upgrade() {
                                    if let Some(offset) = hit.offset {
                                        ui.pending_jumps
                                            .borrow_mut()
                                            .insert(hit.path.clone(), Jump::Offset(offset));
                                    }
                                    ui.open_file(hit.path.clone());
                                }
                            });
                            ui.results.append(&button);
                        }
                    }
                }
            },
        );
    }
    fn show_find(&self) {
        self.find_row.set_visible(true);
        self.find.grab_focus();
    }
    fn find_context(&self, doc: &Doc) -> sourceview5::SearchContext {
        let settings = sourceview5::SearchSettings::new();
        settings.set_search_text(Some(&self.find.text()));
        settings.set_wrap_around(true);
        sourceview5::SearchContext::new(&doc.editor.buffer, Some(&settings))
    }
    fn find_next(&self) {
        let Some(doc) = self.active() else { return };
        if self.find.text().is_empty() {
            return;
        }
        let context = self.find_context(&doc);
        let from = doc
            .editor
            .buffer
            .selection_bounds()
            .map(|(_, b)| b)
            .unwrap_or_else(|| {
                doc.editor
                    .buffer
                    .iter_at_offset(doc.editor.buffer.cursor_position())
            });
        if let Some((mut a, b, _)) = context.forward(&from) {
            doc.editor.buffer.select_range(&a, &b);
            doc.editor
                .view
                .scroll_to_iter(&mut a, 0.15, false, 0.0, 0.0);
        }
    }
    fn replace(&self) {
        let Some(doc) = self.active() else { return };
        if doc.read_only || self.find.text().is_empty() {
            return;
        }
        if let Some((mut a, mut b)) = doc.editor.buffer.selection_bounds() {
            let context = self.find_context(&doc);
            if let Err(e) = context.replace(&mut a, &mut b, &self.replacement.text()) {
                self.message(&format!("Replace failed: {e}"));
            }
        }
        self.find_next();
    }
    fn apply_typography(&self) {
        let p = prefs::get();
        let family = p.font_family.replace(['"', '\\', '\n', '\r'], "");
        let size = p.font_size.clamp(8.0, 32.0);
        // Source is code: the chosen font, monospace. Live and Reading are prose in the
        // theme's interface font, without "monospace" in the list (fontconfig would put a
        // monospace face first).
        let prose = nyx_theme::look()
            .font
            .ui
            .unwrap_or_default()
            .replace(['"', '\\', '\n', '\r'], "");
        let prose = prose
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(|f| format!("\"{f}\""))
            .chain(["sans-serif".to_string()])
            .collect::<Vec<_>>()
            .join(", ");
        self.css.load_from_string(&format!(
            "window.notes-window textview.notes-editor {{font-family: \"{family}\", monospace; font-size: {size}pt;}}\n\
             window.notes-window textview.notes-editor.prose, window.notes-window .notes-reader {{font-family: {prose}; font-size: {}pt;}}",
            // Prose a point under the code size: 12 pt code reads beside 11 pt text.
            size - 1.0
        ));
        let palette = nyx_theme::palette();
        for doc in self.docs.borrow().iter() {
            doc.editor.set_writing_width(p.writing_width);
            doc.editor
                .view
                .set_pixels_above_lines(p.line_spacing.clamp(0, 20));
            doc.editor
                .view
                .set_pixels_below_lines(p.line_spacing.clamp(0, 20));
            doc.editor.set_palette(
                &palette.accent,
                &palette.shadow,
                &palette.dim_text,
                palette.light,
            );
        }
    }
    fn shortcuts(&self) {
        let dialog = Prompt::new(
            &self.window,
            "Notes shortcuts",
            "Notes shortcuts",
            "Ctrl+O  Open vault\nCtrl+Enter  Follow local note link\nF4  Cycle source / live / reading\nF2  Rename or move note\nF11  Focus mode\nCtrl+T  Toggle task\nCtrl+Shift+D  Daily note\nCtrl+Alt+F  Fold heading\nCtrl+N  New note\nCtrl+S  Save\nCtrl+Shift+S  Save a copy\nCtrl+F  Find in note\nCtrl+Shift+F  Search vault\nCtrl+B  Collapse sidebar\nCtrl+Z / Ctrl+Shift+Z  Undo / redo\nCtrl+W  Close note\nCtrl+Q  Quit\nCtrl+,  Preferences\nF1  Shortcuts\nEsc  Close search or panels",
            480,
            520,
        );
        dialog.add_button("Close", gtk::ResponseType::Close);
        dialog.connect_response(|d, _| d.close());
        dialog.present();
    }
}
fn replace_strings(model: &gtk::StringList, items: impl IntoIterator<Item = String>) {
    let items = items.into_iter().collect::<Vec<_>>();
    let references = items.iter().map(String::as_str).collect::<Vec<_>>();
    model.splice(0, model.n_items(), &references);
}
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The sidebar's width and the tree's indent per level, in pixels.
const SIDEBAR: i32 = 236;
const INDENT: i32 = 16;

/// The line holding a link, without its brackets, for a backlink's card.
fn link_line(text: &str, offset: usize) -> Option<String> {
    let offset = offset.min(text.len());
    let start = text.get(..offset)?.rfind('\n').map_or(0, |n| n + 1);
    let end = text.get(offset..)?.find('\n').map_or(text.len(), |n| offset + n);
    let line = text.get(start..end)?.trim().trim_start_matches(['-', '*', '#', ' ']);
    Some(line.replace("[[", "").replace("]]", ""))
}

/// A list of cards: each item is `title\u{1f}text`.
fn card_list(height: i32) -> (gtk::StringList, gtk::ListView, gtk::ScrolledWindow) {
    let (model, view, scroll) = text_list(height);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let card = widgets::vbox(3);
        card.add_css_class("link-card");
        let title = gtk::Label::new(None);
        title.add_css_class("link-card-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let text = gtk::Label::new(None);
        text.add_css_class("link-card-text");
        text.set_xalign(0.0);
        text.set_wrap(true);
        text.set_lines(3);
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        card.append(&title);
        card.append(&text);
        item.set_child(Some(&card));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(value), Some(card)) = (
            item.item().and_downcast::<gtk::StringObject>(),
            item.child().and_downcast::<gtk::Box>(),
        ) else {
            return;
        };
        let value = value.string();
        let (title, text) = value.split_once('\u{1f}').unwrap_or((&value, ""));
        if let Some(t) = card.first_child().and_downcast::<gtk::Label>() {
            t.set_text(title);
        }
        if let Some(t) = card.last_child().and_downcast::<gtk::Label>() {
            t.set_text(text);
            t.set_visible(!text.is_empty());
        }
    });
    view.set_factory(Some(&factory));
    view.add_css_class("cards");
    (model, view, scroll)
}

fn text_list(height: i32) -> (gtk::StringList, gtk::ListView, gtk::ScrolledWindow) {
    let model = gtk::StringList::new(&[]);
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_margin_top(2);
        label.set_margin_bottom(2);
        label.set_margin_start(4);
        label.set_margin_end(4);
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(text), Some(label)) = (
            item.item().and_downcast::<gtk::StringObject>(),
            item.child().and_downcast::<gtk::Label>(),
        ) {
            label.set_text(&text.string());
            label.set_tooltip_text(Some(&text.string()));
        }
    });
    let view = gtk::ListView::new(Some(selection), Some(factory));
    view.set_single_click_activate(true);
    let scroll = scroll(&view);
    scroll.set_min_content_height(height);
    scroll.set_max_content_height(height);
    scroll.set_propagate_natural_height(false);
    (model, view, scroll)
}
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
fn scroll(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(child)
        .build()
}
fn readonly_text(text: &str) -> gtk::ScrolledWindow {
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(text);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_editable(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    scroll(&view)
}

/// A native window prompt, using the suite surface rather than deprecated GTK dialogs.
type ResponseHandler = Box<dyn Fn(&Prompt, gtk::ResponseType)>;
struct Prompt {
    window: gtk::Window,
    content: gtk::Box,
    buttons: gtk::Box,
    response: RefCell<Option<ResponseHandler>>,
    closing: Cell<bool>,
}
impl Prompt {
    fn new(
        parent: &gtk::ApplicationWindow,
        title: &str,
        heading: &str,
        description: &str,
        width: i32,
        height: i32,
    ) -> Rc<Self> {
        let window = gtk::Window::builder()
            .title(title)
            .transient_for(parent)
            .modal(true)
            .destroy_with_parent(true)
            .default_width(width)
            .default_height(height)
            .build();
        window.add_css_class("notes-window");
        let layout = widgets::vbox(12);
        layout.set_margin_top(20);
        layout.set_margin_bottom(20);
        layout.set_margin_start(20);
        layout.set_margin_end(20);
        if !heading.is_empty() {
            let label = gtk::Label::new(Some(heading));
            label.set_wrap(true);
            label.set_xalign(0.0);
            layout.append(&label);
        }
        if !description.is_empty() {
            let label = gtk::Label::new(Some(description));
            label.set_wrap(true);
            label.set_xalign(0.0);
            label.set_selectable(true);
            label.set_focusable(false);
            layout.append(&label);
        }
        let content = widgets::vbox(8);
        content.set_vexpand(true);
        layout.append(&content);
        let buttons = widgets::hbox(8);
        buttons.set_halign(gtk::Align::End);
        layout.append(&buttons);
        window.set_child(Some(&layout));
        let dialog = Rc::new(Self {
            window,
            content,
            buttons,
            response: RefCell::new(None),
            closing: Cell::new(false),
        });
        let weak = Rc::downgrade(&dialog);
        dialog.window.connect_close_request(move |_| {
            if let Some(dialog) = weak.upgrade()
                && !dialog.closing.get()
            {
                if let Some(callback) = dialog.response.borrow().as_ref() {
                    callback(&dialog, gtk::ResponseType::Cancel);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        let keep = RefCell::new(Some(dialog.clone()));
        dialog.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
        dialog
    }
    fn add_button(self: &Rc<Self>, text: &str, response: gtk::ResponseType) {
        let button = gtk::Button::with_label(text);
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            let Some(dialog) = weak.upgrade() else {
                return;
            };
            if let Some(callback) = dialog.response.borrow().as_ref() {
                callback(&dialog, response);
            }
        });
        if response == gtk::ResponseType::Accept {
            self.window.set_default_widget(Some(&button));
        }
        self.buttons.append(&button);
    }
    fn content_area(&self) -> &gtk::Box {
        &self.content
    }
    fn connect_response(&self, callback: impl Fn(&Prompt, gtk::ResponseType) + 'static) {
        *self.response.borrow_mut() = Some(Box::new(callback));
    }
    fn present(&self) {
        // Focus an input with its caret at the end; GTK's default can select the
        // first selectable label or the full entry before the user interacts.
        fn input(widget: &gtk::Widget) -> Option<gtk::Widget> {
            if widget.is::<gtk::Entry>()
                || widget.is::<gtk::TextView>()
                || widget.is::<sourceview5::View>()
            {
                return Some(widget.clone());
            }
            let mut child = widget.first_child();
            while let Some(w) = child {
                child = w.next_sibling();
                if let Some(found) = input(&w) {
                    return Some(found);
                }
            }
            None
        }
        let target = input(self.content.upcast_ref()).or_else(|| self.buttons.first_child());
        if let Some(target) = &target {
            gtk::prelude::GtkWindowExt::set_focus(&self.window, Some(target));
            if let Some(entry) = target.downcast_ref::<gtk::Entry>() {
                entry.select_region(-1, -1);
                entry.set_position(-1);
            }
        }
        self.window.present();
        if let Some(entry) = target.and_then(|w| w.downcast::<gtk::Entry>().ok()) {
            let weak = entry.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(entry) = weak.upgrade() {
                    entry.select_region(-1, -1);
                    entry.set_position(-1);
                }
            });
        }
    }
    fn close(&self) {
        self.closing.set(true);
        self.window.destroy();
        self.window.set_child(gtk::Widget::NONE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[track_caller]
    fn pump_until(mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(
                std::time::Instant::now() < deadline,
                "GTK operation timed out"
            );
            while glib::MainContext::default().pending() {
                glib::MainContext::default().iteration(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn descendants(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let widget = widget.as_ref();
        let mut all = vec![widget.clone()];
        let mut child = widget.first_child();
        while let Some(w) = child {
            all.extend(descendants(&w));
            child = w.next_sibling();
        }
        all
    }
    fn control_result(name: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        let result = Rc::new(RefCell::new(None));
        let output = result.clone();
        integration_call(
            name,
            args,
            Box::new(move |value| *output.borrow_mut() = Some(value)),
        );
        pump_until(|| result.borrow().is_some());
        result.borrow_mut().take().unwrap()
    }
    fn phase_four_views(ui: &Rc<Ui>) {
        let doc = ui.active().unwrap();
        let source = doc.editor.text();
        assert!(control_result("notes.vault_search", serde_json::json!({"query":"Café"})).is_err());
        prefs::update(|p| {
            p.integration_read = true;
            p.integration_write = true;
        });
        let read = control_result("notes.vault_read", serde_json::json!({"note":"Café"})).unwrap();
        assert_eq!(read["untrusted"], true);
        assert!(read["version"].as_str().is_some());
        let request = control_result(
            "notes.vault_append",
            serde_json::json!({"note":"Café","text":"fixture proposal"}),
        )
        .unwrap();
        assert_eq!(request["applied"], false);
        assert_eq!(doc.editor.text(), source);
        let review = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .find(|w| w.title().as_deref() == Some("Review suite edit"))
            .unwrap();
        descendants(&review)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Cancel"))
            .unwrap()
            .emit_clicked();
        let state = control_result(
            "notes.vault_proposal_status",
            serde_json::json!({"proposal_id":request["proposal_id"]}),
        )
        .unwrap();
        assert_eq!(state["status"], "not_applied");
        assert_eq!(doc.editor.text(), source);
        ui.integration_preferences();
        ui.clip_dialog();
        ui.capture_insert();
        #[cfg(feature = "ai")]
        {
            ui.intelligence_preferences();
            let end = source.len();
            ui.proposal(
                doc.clone(),
                doc.editor.revision.get(),
                end..end,
                String::new(),
                "\nfixture generated edit".into(),
            );
            let review = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.title().as_deref() == Some("Review proposed edit"))
                .unwrap();
            descendants(&review)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.label().as_deref() == Some("Apply to note"))
                .unwrap()
                .emit_clicked();
            assert!(doc.editor.text().ends_with("fixture generated edit"));
            doc.editor.buffer.undo();
            assert_eq!(doc.editor.text(), source);
        }
        prefs::update(|p| {
            p.integration_read = false;
            p.integration_write = false;
        });
        for window in gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
        {
            if window != ui.window {
                window.destroy();
            }
        }
    }
    fn phase_five_views(ui: &Rc<Ui>) {
        #[cfg(feature = "sync")]
        {
            ui.sync_preferences();
            assert!(prefs::get().sync_profiles.is_empty());
        }
        #[cfg(feature = "plugins")]
        {
            let doc = ui.active().unwrap();
            let source = doc.editor.text();
            let end = source.len();
            let allowed = Rc::new(Cell::new(true));
            let permission = allowed.clone();
            ui.proposal_guarded(
                doc.clone(),
                doc.editor.revision.get(),
                end..end,
                String::new(),
                "revoked fixture".into(),
                Rc::new(move || permission.get()),
            );
            allowed.set(false);
            let review = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.title().as_deref() == Some("Review proposed edit"))
                .unwrap();
            descendants(&review)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.label().as_deref() == Some("Apply to note"))
                .unwrap()
                .emit_clicked();
            assert_eq!(doc.editor.text(), source);
            review.destroy();
            ui.extensions();
            ui.extension_commands();
            pump_until(|| {
                gtk::Window::list_toplevels()
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Window>().ok())
                    .any(|w| w.title().as_deref() == Some("Extensions"))
            });
        }
    }
    fn phase_three_views(ui: &Rc<Ui>, temp: &std::path::Path) {
        let source = ui.active().unwrap().editor.text();
        ui.toggle_focus();
        assert!(!ui.nav.is_visible());
        ui.toggle_focus();
        assert!(ui.nav.is_visible());
        prefs::update(|p| p.typewriter = true);
        ui.apply_writing();
        assert_eq!(ui.active().unwrap().editor.text(), source);
        ui.graph(false);
        pump_until(|| {
            ui.window
                .application()
                .unwrap()
                .windows()
                .iter()
                .any(|w| w.title().as_deref() == Some("Knowledge graph"))
        });
        ui.collections();
        ui.projects();
        ui.writing_preferences();
        ui.workspaces();
        ui.history();
        let windows = ui.window.application().unwrap().windows();
        let collection = windows
            .iter()
            .find(|w| w.title().as_deref() == Some("Collections"))
            .unwrap();
        let layout = descendants(collection)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::DropDown>().ok())
            .find(|d| d.model().is_some_and(|m| m.n_items() == 4))
            .unwrap();
        // Realize all native collection layouts using the same indexed notes.
        for mode in 0..4 {
            layout.set_selected(mode);
            let until = std::time::Instant::now() + Duration::from_millis(35);
            while std::time::Instant::now() < until {
                while glib::MainContext::default().pending() {
                    glib::MainContext::default().iteration(false);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let path = temp.join("Workspace.canvas");
        std::fs::write(&path,r#"{"nodes":[{"id":"file","type":"file","file":"Embed.md","x":-300,"y":0,"width":280,"height":200},{"id":"image","type":"file","file":"Attachments/pixel.png","x":10,"y":0,"width":260,"height":200}],"edges":[{"id":"edge","fromNode":"file","toNode":"image","label":"native"}],"extension":"preserve"}"#).unwrap();
        ui.open_canvas(path.clone());
        pump_until(|| ui.canvases.borrow().contains_key(&path));
        let canvas = ui.canvases.borrow().get(&path).unwrap().0.clone();
        let buttons = descendants(&canvas)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .collect::<Vec<_>>();
        buttons
            .iter()
            .find(|b| b.label().as_deref() == Some("Text"))
            .unwrap()
            .emit_clicked();
        buttons
            .iter()
            .find(|b| b.label().as_deref() == Some("Save"))
            .unwrap()
            .emit_clicked();
        pump_until(|| {
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("New Markdown card")
        });
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("extension"));
        assert!(saved.contains("preserve"));
        // Destroy only isolated test surfaces; no desktop app is opened or restarted.
        for window in gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
        {
            if window != ui.window {
                window.destroy();
            }
        }
        assert_eq!(ui.active().unwrap().editor.text(), source);
    }
    #[test]
    #[ignore = "requires an isolated GTK display and XDG directories"]
    fn vault_folder_expansion() {
        fn folder(ui: &Ui, name: &str) -> Option<gtk::Expander> {
            descendants(&ui.tree)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Expander>().ok())
                .find(|e| {
                    e.label_widget().is_some_and(|row| {
                        descendants(&row)
                            .into_iter()
                            .filter_map(|w| w.downcast::<gtk::Label>().ok())
                            .any(|l| l.has_css_class("tree-name") && l.text() == name)
                    })
                })
        }
        gtk::init().unwrap();
        nyx_core::app::init(nyx_core::app::App {
            slug: "nyx-notes",
            name: "Notes",
        });
        prefs::update(|p| {
            p.expanded_folders.clear();
            p.restore_tabs = false;
        });
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for root in [first.path(), second.path()] {
            std::fs::create_dir_all(root.join("Research/Nested")).unwrap();
        }
        let first = first.path().canonicalize().unwrap();
        let second = second.path().canonicalize().unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.design_nexus.nyx.Notes.FoldersTest")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        present(&app, Some(first.clone()), Vec::new());
        let ui = current_ui().unwrap();
        pump_until(|| folder(&ui, "Research").is_some());
        folder(&ui, "Research").unwrap().set_expanded(true);
        pump_until(|| folder(&ui, "Nested").is_some());
        folder(&ui, "Nested").unwrap().set_expanded(true);
        ui.refresh();
        pump_until(|| {
            folder(&ui, "Research").is_some_and(|f| f.is_expanded())
                && folder(&ui, "Nested").is_some_and(|f| f.is_expanded())
        });
        ui.open_vault(second.clone(), Vec::new());
        pump_until(|| {
            ui.root.borrow().as_ref() == Some(&second) && folder(&ui, "Research").is_some()
        });
        assert!(!folder(&ui, "Research").unwrap().is_expanded());
        prefs::flush();
        let saved = prefs::get().expanded_folders;
        assert!(saved[&first].contains(std::path::Path::new("Research/Nested")));
        assert!(
            std::fs::read_to_string(nyx_core::paths::prefs_file())
                .unwrap()
                .contains("expanded_folders")
        );
        prefs::update(|p| p.expanded_folders.clear());
        prefs::reload();
        assert_eq!(prefs::get().expanded_folders, saved);
        ui.window.close();
        assert!(!ui.window.is_visible());
        UI.with(|s| s.borrow_mut().take());
        present(&app, Some(first), Vec::new());
        let ui = current_ui().unwrap();
        pump_until(|| {
            folder(&ui, "Research").is_some_and(|f| f.is_expanded())
                && folder(&ui, "Nested").is_some_and(|f| f.is_expanded())
        });
        folder(&ui, "Nested").unwrap().set_expanded(false);
        prefs::flush();
        ui.refresh();
        pump_until(|| {
            folder(&ui, "Research").is_some_and(|f| f.is_expanded())
                && folder(&ui, "Nested").is_some_and(|f| !f.is_expanded())
        });
        ui.window.close();
        UI.with(|s| s.borrow_mut().take());
    }
    #[test]
    #[ignore = "requires an isolated GTK display and XDG directories"]
    fn tabs_menus_dialog_focus_and_close() {
        fn window(title: &str) -> Option<gtk::Window> {
            gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.title().as_deref() == Some(title) && w.is_visible())
        }
        fn click(window: &gtk::Window, label: &str) {
            descendants(window)
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Button>().ok())
                .find(|b| b.label().as_deref() == Some(label))
                .unwrap()
                .emit_clicked();
        }
        gtk::init().unwrap();
        nyx_core::app::init(nyx_core::app::App {
            slug: "nyx-notes",
            name: "Notes",
        });
        prefs::update(|p| {
            p.focus_mode = false;
            p.split_enabled = false;
            p.sidebar_collapsed = false;
        });
        let vault = tempfile::tempdir().unwrap();
        let root = vault.path().canonicalize().unwrap();
        let path = root.join("Project Ideas.md");
        std::fs::write(&path, "# Ideas\n").unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.design_nexus.nyx.Notes.InteractionTest")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        present(&app, Some(root.clone()), vec![path.clone()]);
        let ui = current_ui().unwrap();
        pump_until(|| ui.active().is_some());
        let doc = ui.active().unwrap();
        ui.window.set_default_size(800, 700);
        pump_until(|| {
            ui.window.width() == 800 && doc.label.is_mapped() && !doc.label.layout().is_ellipsized()
        });
        assert_eq!(doc.label.text(), "Project Ideas");
        assert!(!doc.label.layout().is_ellipsized());
        assert!(gtk::prelude::TextViewExt::extra_menu(&doc.editor.view).is_some());
        let tab = ui.doc_notebook(&doc).tab_label(&doc.editor.root).unwrap();
        let controllers = tab.observe_controllers();
        let gesture = (0..controllers.n_items())
            .filter_map(|i| controllers.item(i).and_downcast::<gtk::GestureClick>())
            .find(|g| g.button() == 3)
            .unwrap();
        gesture.emit_by_name::<()>("pressed", &[&1i32, &12.0f64, &12.0f64]);
        let menu = descendants(&tab)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::PopoverMenu>().ok())
            .next()
            .unwrap();
        assert!(menu.is_visible());
        menu.activate_action("context.rename", None).unwrap();
        menu.popdown();
        pump_until(|| window("Rename or move").is_some());
        let rename = window("Rename or move").unwrap();
        let entry = descendants(&rename)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Entry>().ok())
            .next()
            .unwrap();
        pump_until(|| entry.is_mapped() && entry.selection_bounds().is_none());
        for label in descendants(&rename)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .filter(|l| l.is_selectable())
        {
            assert!(label.selection_bounds().is_none());
        }
        entry.set_text("Renamed.md");
        click(&rename, "Review changes");
        pump_until(|| window("Apply move").is_some());
        click(&window("Apply move").unwrap(), "Move and update links");
        let renamed = root.join("Renamed.md");
        pump_until(|| ui.docs.borrow().iter().any(|d| d.path == renamed));
        assert!(!path.exists());
        assert_eq!(
            ui.docs
                .borrow()
                .iter()
                .find(|d| d.path == renamed)
                .unwrap()
                .label
                .text(),
            "Renamed"
        );
        let (aux, _, _) = ui.workspace_window("Auxiliary view");
        aux.present();
        ui.window.close();
        pump_until(|| !ui.window.is_visible());
        assert!(!aux.is_visible());
        UI.with(|s| s.borrow_mut().take());
        present(&app, Some(root), vec![renamed.clone()]);
        let ui = current_ui().unwrap();
        pump_until(|| ui.active().is_some());
        let doc = ui.active().unwrap();
        doc.editor
            .buffer
            .insert(&mut doc.editor.buffer.end_iter(), "unsaved");
        ui.window.close();
        pump_until(|| window("Close Notes").is_some());
        click(&window("Close Notes").unwrap(), "Cancel");
        assert!(ui.window.is_visible());
        assert!(!ui.closing.get());
        ui.window.close();
        pump_until(|| window("Close Notes").is_some());
        click(&window("Close Notes").unwrap(), "Save all");
        pump_until(|| !ui.window.is_visible());
        assert!(
            std::fs::read_to_string(renamed)
                .unwrap()
                .ends_with("unsaved")
        );
        UI.with(|s| s.borrow_mut().take());
    }
    #[test]
    #[ignore = "requires an isolated GTK display and XDG directories"]
    fn settings_and_saved_vault() {
        gtk::init().unwrap();
        nyx_core::app::init(nyx_core::app::App {
            slug: "nyx-notes",
            name: "Notes",
        });
        let vault = tempfile::tempdir().unwrap();
        let root = vault.path().canonicalize().unwrap();
        prefs::update(|p| {
            p.last_vault = None;
            p.mode = nyx_theme::ThemeMode::Theme;
            p.restore_tabs = false;
        });
        let app = gtk::Application::builder()
            .application_id("io.github.design_nexus.nyx.Notes.SettingsTest")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        present(&app, Some(root.clone()), Vec::new());
        let ui = current_ui().unwrap();
        pump_until(|| ui.root.borrow().as_ref() == Some(&root));
        ui.window.set_default_size(800, 700);
        pump_until(|| ui.window.width() == 800);
        ui.settings();
        let page = nyx_ui::settings_card::page().unwrap();
        pump_until(|| page.is_mapped() && page.height() > 200);
        let theme = descendants(&page)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::DropDown>().ok())
            .find(|w| w.widget_name() == "notes-theme")
            .unwrap();
        pump_until(|| theme.is_mapped() && theme.height() > 0);
        let themes = nyx_theme::all();
        let other = themes
            .iter()
            .position(|t| t.id != prefs::get().theme)
            .unwrap();
        theme.set_selected(other as u32);
        assert_eq!(prefs::get().theme, themes[other].id);
        assert_eq!(prefs::get().mode, nyx_theme::ThemeMode::Theme);
        nyx_ui::settings_card::select("Editor");
        let size = descendants(&page)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::SpinButton>().ok())
            .find(|w| w.widget_name() == "notes-font-size")
            .unwrap();
        pump_until(|| size.is_mapped() && size.height() > 0);
        size.set_value(15.0);
        assert_eq!(prefs::get().font_size, 15.0);
        nyx_ui::settings_card::select("Vault");
        let location = descendants(&page)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Label>().ok())
            .find(|w| w.widget_name() == "notes-vault-path")
            .unwrap();
        pump_until(|| location.is_mapped() && location.height() > 0);
        assert_eq!(location.text().as_str(), root.to_str().unwrap());
        let folder = descendants(&page)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Entry>().ok())
            .find(|w| w.widget_name() == "notes-vault-folder-3")
            .unwrap();
        let apply = descendants(&page)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|w| w.widget_name() == "notes-apply-folders")
            .unwrap();
        folder.set_text("Images");
        apply.emit_clicked();
        assert_eq!(prefs::get().attachments_folder, "Images");
        folder.set_text("../outside");
        apply.emit_clicked();
        assert_eq!(prefs::get().attachments_folder, "Images");
        for group in ["Writing", "Integrations"] {
            nyx_ui::settings_card::select(group);
            pump_until(|| {
                descendants(&page)
                    .into_iter()
                    .any(|w| w.widget_name() == group && w.is_mapped() && w.height() > 0)
            });
        }
        prefs::flush();
        prefs::reload();
        assert_eq!(prefs::get().last_vault, Some(root.clone()));
        assert_eq!(prefs::get().font_size, 15.0);
        nyx_ui::settings_card::close();
        ui.allow_close.set(true);
        ui.window.destroy();
        UI.with(|s| s.borrow_mut().take());
        present(&app, None, Vec::new());
        let reopened = current_ui().unwrap();
        pump_until(|| reopened.root.borrow().as_ref() == Some(&root));
        assert!(
            reopened
                .setup_window
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade())
                .is_none()
        );
        assert_eq!(
            reopened.workspace.visible_child_name().as_deref(),
            Some("editor")
        );
        reopened.allow_close.set(true);
        reopened.window.destroy();
        UI.with(|s| s.borrow_mut().take());
    }
    #[test]
    #[ignore = "requires an isolated GTK display and XDG directories"]
    fn vault_connection_sidebar_and_resolve() {
        gtk::init().unwrap();
        nyx_core::app::init(nyx_core::app::App {
            slug: "nyx-notes",
            name: "Notes",
        });
        prefs::update(|p| {
            p.last_vault = None;
            p.recent_vaults.clear();
            p.sidebar_collapsed = false;
            p.focus_mode = false;
        });
        let app = gtk::Application::builder()
            .application_id("io.github.design_nexus.nyx.Notes.ConnectionTest")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        present(&app, None, Vec::new());
        let ui = current_ui().unwrap();
        let setup = ui
            .setup_window
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
            .unwrap();
        assert_eq!(
            ui.workspace.visible_child_name().as_deref(),
            Some("welcome")
        );
        assert!(!ui.notice_row.is_visible());
        assert!(!ui.resolve_button.is_visible());
        let entry = descendants(&setup)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Entry>().ok())
            .find(|w| w.widget_name() == "vault-path")
            .unwrap();
        let connect = descendants(&setup)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Connect"))
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        entry.set_text(&temp.path().join("missing").to_string_lossy());
        connect.emit_clicked();
        pump_until(|| ui.notice.text().contains("Could not open vault"));
        assert!(setup.is_visible());
        assert!(!ui.resolve_button.is_visible());
        std::fs::create_dir(temp.path().join(".obsidian")).unwrap();
        std::fs::write(temp.path().join(".obsidian/config"), "unchanged").unwrap();
        let path = temp.path().join("One.md");
        std::fs::write(&path, "original").unwrap();
        std::fs::write(temp.path().join("Two.md"), "other").unwrap();
        entry.set_text(&temp.path().to_string_lossy());
        connect.emit_clicked();
        pump_until(|| ui.root.borrow().is_some());
        assert!(!setup.is_visible());
        assert_eq!(ui.workspace.visible_child_name().as_deref(), Some("editor"));
        ui.window.set_default_size(800, 700);
        pump_until(|| ui.window.width() == 800);
        ui.fit();
        assert!(ui.compact.get());
        ui.head
            .first_child()
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        // Collapsed, the sidebar is hidden; shown again, it gets its width on the next
        // layout.
        pump_until(|| !ui.compact.get() && ui.tree.is_visible() && ui.nav.width() >= 200);
        assert_eq!(ui.window.width(), 800);
        assert!(ui.nav.is_visible());
        ui.toggle_sidebar();
        assert!(ui.compact.get());
        ui.open_file(path.clone());
        pump_until(|| ui.docs.borrow().iter().any(|d| d.path == path));
        let editor = ui
            .docs
            .borrow()
            .iter()
            .find(|d| d.path == path)
            .unwrap()
            .editor
            .clone();
        let gutter = sourceview5::prelude::ViewExt::gutter(&editor.view, gtk::TextWindowType::Left);
        let expected = gdk::RGBA::parse(nyx_theme::palette().dim_text).unwrap();
        pump_until(|| gutter.color() == expected);

        let doc = ui
            .docs
            .borrow()
            .iter()
            .find(|d| d.path == path)
            .unwrap()
            .clone();
        doc.editor
            .buffer
            .insert(&mut doc.editor.buffer.end_iter(), " mine");
        std::fs::write(&path, "external").unwrap();
        ui.save(doc.clone(), None);
        pump_until(|| doc.conflict.get() && !doc.saving.get());
        assert!(ui.resolve_button.is_visible());
        ui.open_file(temp.path().join("Two.md"));
        pump_until(|| ui.active().is_some_and(|d| d.path.ends_with("Two.md")));
        ui.resolve_button.emit_clicked();
        pump_until(|| {
            gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .any(|w| w.title().as_deref() == Some("Resolve external changes"))
        });
        let comparison = gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .find(|w| w.title().as_deref() == Some("Resolve external changes"))
            .unwrap();
        descendants(&comparison)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Use disk version"))
            .unwrap()
            .emit_clicked();
        assert_eq!(doc.editor.text(), "external");
        assert!(!doc.conflict.get());
        assert!(!ui.notice_row.is_visible());
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".obsidian/config")).unwrap(),
            "unchanged"
        );
        ui.allow_close.set(true);
        ui.window.destroy();
        UI.with(|s| s.borrow_mut().take());
    }
    #[test]
    #[ignore = "requires an isolated GTK display and XDG directories"]
    fn vault_editor_save_and_conflict() {
        gtk::init().unwrap();
        nyx_core::app::init(nyx_core::app::App {
            slug: "nyx-notes",
            name: "Notes",
        });
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Café.md");
        let original = include_bytes!("../../../tests/fixtures/obsidian/fidelity.md");
        std::fs::write(&path, original).unwrap();
        let linked = temp.path().join("Note.md");
        std::fs::write(&linked, "# Intro\n\n# Section\n[[Café]]").unwrap();
        std::fs::create_dir(temp.path().join("Attachments")).unwrap();
        let pixel = gdk::MemoryTexture::new(
            1,
            1,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from_static(&[0, 0, 0, 255]),
            4,
        );
        std::fs::write(
            temp.path().join("Attachments/pixel.png"),
            pixel.save_to_png_bytes(),
        )
        .unwrap();
        std::fs::write(temp.path().join("Embed.md"),"# Embedded note\n\n**Rendered text**\n\n![pixel](Attachments/pixel.png)\n\n> [!note] Native callout\n> Body\n").unwrap();
        let app = gtk::Application::builder()
            .application_id("io.github.design_nexus.nyx.Notes.Test")
            .build();
        app.register(gio::Cancellable::NONE).unwrap();
        present(&app, Some(temp.path().to_owned()), vec![path.clone()]);
        let ui = current_ui().unwrap();
        pump_until(|| !ui.docs.borrow().is_empty());
        let doc = ui.active().unwrap();
        assert_eq!(vault::encode(&doc.editor.text(), doc.bom.get()), original);
        pump_until(|| {
            doc.editor.parsed_revision() == doc.editor.revision.get()
                && !ui.backlink_items.borrow().is_empty()
        });
        assert!(ui.outline_model.n_items() > 0);
        assert!(ui.properties_model.n_items() > 0);
        assert!(ui.tags_model.n_items() > 0);
        let link = doc
            .editor
            .links()
            .into_iter()
            .find(|link| link.target == "Note#Section")
            .unwrap();
        ui.follow_link(link);
        pump_until(|| {
            ui.active().is_some_and(|d| {
                d.path == linked
                    && d.editor.parsed_revision() == d.editor.revision.get()
                    && d.editor.buffer.cursor_position() == 9
            })
        });
        assert_eq!(ui.outline_model.n_items(), 2);
        ui.open_file(path.clone());
        // The complete writing workspace uses the same authoritative source through every mode.
        ui.live.set_active(true);
        doc.editor.set_property("status", Some("draft")).unwrap();
        assert!(doc.editor.text().contains("status: draft"));
        doc.editor.buffer.undo();
        assert_eq!(vault::encode(&doc.editor.text(), doc.bom.get()), original);
        let task = doc.editor.text().find("[ ] Task").unwrap();
        doc.editor.buffer.place_cursor(
            &doc.editor
                .buffer
                .iter_at_offset(notes_editor::character_offset(&doc.editor.text(), task)),
        );
        assert!(doc.editor.toggle_task());
        assert!(doc.editor.text().contains("[x] Task"));
        doc.editor.buffer.undo();
        assert_eq!(vault::encode(&doc.editor.text(), doc.bom.get()), original);
        pump_until(|| doc.editor.parsed_revision() == doc.editor.revision.get());
        ui.reading.set_active(true);
        assert_eq!(doc.editor.mode(), notes_editor::Mode::Reading);
        assert!(!doc.editor.view.is_editable());
        let embed = doc.editor.text().find("![[Embed]]").unwrap();
        doc.editor.jump_to(embed);
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        while std::time::Instant::now() < deadline {
            while glib::MainContext::default().pending() {
                glib::MainContext::default().iteration(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(vault::encode(&doc.editor.text(), doc.bom.get()), original);
        ui.toggle_split();
        assert!(ui.secondary.is_visible());
        assert!(ui.secondary.page_num(&doc.editor.root).is_some());
        assert!(ui.notebook.n_pages() > 0);
        ui.toggle_split();
        assert!(!ui.secondary.is_visible());
        assert!(ui.notebook.page_num(&doc.editor.root).is_some());
        ui.reading.set_active(false);
        ui.live.set_active(false);
        ui.open_file(path.clone());
        ui.live.set_active(true);
        ui.live.set_active(false);
        doc.editor
            .buffer
            .insert(&mut doc.editor.buffer.end_iter(), "\nEdited in Notes");
        ui.save_active();
        pump_until(|| !doc.dirty.get() && !doc.saving.get());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("Edited in Notes")
        );
        assert!(
            vault::history::list(&nyx_core::paths::state_dir(), &path)
                .unwrap()
                .len()
                >= 2
        );
        phase_three_views(&ui, temp.path());
        phase_four_views(&ui);
        phase_five_views(&ui);
        let doc = ui.active().unwrap();
        std::fs::write(&path, "external version").unwrap();
        doc.editor
            .buffer
            .insert(&mut doc.editor.buffer.end_iter(), " local edit");
        ui.save_active();
        pump_until(|| doc.conflict.get() && !doc.saving.get());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external version");
        pump_until(|| {
            vault::read_draft(&nyx_core::paths::state_dir(), &path)
                .unwrap()
                .is_some()
        });
        ui.settings();
        assert!(nyx_ui::settings_card::is_open());
        nyx_ui::settings_card::close();
        ui.allow_close.set(true);
        ui.window.destroy();
        UI.with(|s| s.borrow_mut().take());
    }
}
