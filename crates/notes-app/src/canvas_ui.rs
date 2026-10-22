//! Native JSON Canvas workspace: Cairo geometry with source-preserving JSON attributes.
use super::views::{self, Camera};
use super::*;
use gdk::prelude::GdkCairoContextExt;
use vault::canvas::{self, Canvas, Node};
#[derive(Clone)]
enum Preview {
    Text(String),
    Image(gtk::gdk_pixbuf::Pixbuf),
    Error(String),
}
struct CanvasView {
    ui: std::rc::Weak<Ui>,
    window: gtk::Window,
    area: gtk::DrawingArea,
    status: gtk::Label,
    root: PathBuf,
    db: PathBuf,
    path: PathBuf,
    canvas: RefCell<Canvas>,
    version: RefCell<Version>,
    bom: Cell<bool>,
    read_only: bool,
    revision: Cell<u64>,
    dirty: Rc<Cell<bool>>,
    saving: Cell<bool>,
    conflict: Cell<bool>,
    allow_close: Cell<bool>,
    asking: Cell<bool>,
    selected: RefCell<Option<String>>,
    connect_from: RefCell<Option<String>>,
    camera: Rc<Cell<Camera>>,
    undo: RefCell<Vec<Canvas>>,
    redo: RefCell<Vec<Canvas>>,
    previews: RefCell<std::collections::HashMap<String, Preview>>,
    save_timer: RefCell<Option<glib::SourceId>>,
    draft_timer: RefCell<Option<glib::SourceId>>,
}
impl CanvasView {
    fn checkpoint(&self) {
        let mut undo = self.undo.borrow_mut();
        undo.push(self.canvas.borrow().clone());
        if undo.len() > 50 {
            undo.remove(0);
        }
        self.redo.borrow_mut().clear();
    }
    fn changed(self: &Rc<Self>) {
        self.dirty.set(true);
        self.revision.set(self.revision.get() + 1);
        self.area.queue_draw();
        self.window.set_title(Some(&format!(
            "{} • — Notes",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        )));
        if let Some(timer) = self.draft_timer.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        *self.draft_timer.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(250),
            move || {
                if let Some(view) = weak.upgrade() {
                    view.draft_timer.borrow_mut().take();
                    view.persist(|| {});
                }
            },
        ));
        if let Some(timer) = self.save_timer.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        *self.save_timer.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_secs(1),
            move || {
                if let Some(view) = weak.upgrade() {
                    view.save_timer.borrow_mut().take();
                    view.save(|_| {});
                }
            },
        ));
    }
    fn writable(&self) -> bool {
        if self.read_only {
            self.status
                .set_text("This canvas is read-only; save a copy.");
            false
        } else {
            true
        }
    }
    fn persist(self: &Rc<Self>, done: impl FnOnce() + 'static) {
        let model = self.canvas.borrow().clone();
        let version = self.version.borrow().clone();
        let path = self.path.clone();
        let bom = self.bom.get();
        let state = nixe_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<()> {
                let draft = Draft {
                    path,
                    text: serde_json::to_string_pretty(&model)?,
                    bom,
                    base_etag: version.etag,
                    base_digest: version.digest.to_hex().to_string(),
                };
                vault::write_draft(&state, &draft)
            },
            move |result| {
                if let Some(view) = weak.upgrade() {
                    match result {
                        Ok(()) => done(),
                        Err(e) => view
                            .status
                            .set_text(&format!("Recovery failed: {e:#}. Keep this window open.")),
                    }
                }
            },
        );
    }
    fn save(self: &Rc<Self>, done: impl FnOnce(bool) + 'static) {
        if !self.dirty.get() {
            done(true);
            return;
        }
        if self.saving.get() || self.conflict.get() || !self.writable() {
            done(false);
            return;
        }
        self.saving.set(true);
        let revision = self.revision.get();
        let model = self.canvas.borrow().clone();
        let version = self.version.borrow().clone();
        let root = self.root.clone();
        let path = self.path.clone();
        let bom = self.bom.get();
        let state = nixe_core::paths::state_dir();
        let p = prefs::get();
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<Version> {
                let text = serde_json::to_string_pretty(&model)?;
                vault::write_draft(
                    &state,
                    &Draft {
                        path: path.clone(),
                        text: text.clone(),
                        bom,
                        base_etag: version.etag.clone(),
                        base_digest: version.digest.to_hex().to_string(),
                    },
                )?;
                let disk = vault::load(&path)?;
                vault::history::capture(
                    &state,
                    &path,
                    &disk.text,
                    disk.bom,
                    p.history_keep,
                    p.history_days,
                )?;
                vault::history::capture(&state, &path, &text, bom, p.history_keep, p.history_days)?;
                canvas::save(&root, &path, &model, bom, &version)
            },
            move |result| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                view.saving.set(false);
                match result {
                    Ok(version) => {
                        *view.version.borrow_mut() = version;
                        if revision == view.revision.get() {
                            view.dirty.set(false);
                            view.window.set_title(Some(&format!(
                                "{} — Notes",
                                view.path.file_name().unwrap_or_default().to_string_lossy()
                            )));
                            view.status.set_text(
                                "Saved · scroll to zoom · middle-drag to pan · drag cards to move",
                            );
                            let state = nixe_core::paths::state_dir();
                            let path = view.path.clone();
                            worker::run(move || vault::remove_draft(&state, &path), |_| {});
                        } else {
                            view.changed();
                        }
                        done(!view.dirty.get());
                    }
                    Err(e) => {
                        view.conflict.set(true);
                        view.status.set_text(&format!(
                            "Save stopped: {e:#}. Draft retained. Save a copy or reload disk."
                        ));
                        done(false);
                    }
                }
            },
        );
    }
    fn copy(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Save canvas copy")
            .initial_name("Canvas copy.canvas")
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let model = view.canvas.borrow().clone();
                let bom = view.bom.get();
                let weak = Rc::downgrade(&view);
                worker::run(
                    move || -> anyhow::Result<()> {
                        vault::save_copy(&path, &serde_json::to_string_pretty(&model)?, bom)
                    },
                    move |result| {
                        if let Some(view) = weak.upgrade() {
                            view.status.set_text(&match result {
                                Ok(()) => "Canvas copy saved; the original is unchanged".into(),
                                Err(e) => format!("Save copy: {e:#}"),
                            });
                        }
                    },
                );
            }
        });
    }
    fn add(self: &Rc<Self>, kind: &str, value: &str) {
        if !self.writable() {
            return;
        }
        self.checkpoint();
        let c = self.camera.get();
        let id = self.canvas.borrow_mut().add(
            kind,
            value,
            (-c.x / c.zoom) as i64,
            (-c.y / c.zoom) as i64,
        );
        *self.selected.borrow_mut() = Some(id);
        self.changed();
        self.assets();
    }
    fn remove(self: &Rc<Self>) {
        if !self.writable() {
            return;
        }
        let Some(id) = self.selected.borrow().clone() else {
            return;
        };
        self.checkpoint();
        self.canvas.borrow_mut().remove(&id);
        self.selected.borrow_mut().take();
        self.changed();
    }
    fn undo(self: &Rc<Self>, redo: bool) {
        if !self.writable() {
            return;
        }
        let previous = if redo {
            self.redo.borrow_mut().pop()
        } else {
            self.undo.borrow_mut().pop()
        };
        if let Some(previous) = previous {
            let current = self.canvas.replace(previous);
            if redo {
                self.undo.borrow_mut().push(current);
            } else {
                self.redo.borrow_mut().push(current);
            }
            self.changed();
            self.assets();
        }
    }
    fn connect(self: &Rc<Self>) {
        if !self.writable() {
            return;
        }
        if let Some(id) = self.selected.borrow().clone()
            && self.canvas.borrow().nodes.iter().any(|n| n.id == id)
        {
            *self.connect_from.borrow_mut() = Some(id);
            self.status
                .set_text("Click the destination card to create a connection");
        } else {
            self.status.set_text("Select the source card first");
        }
    }
    fn hit(&self, x: f64, y: f64) -> Option<String> {
        let model = self.canvas.borrow();
        model
            .nodes
            .iter()
            .rev()
            .find(|n| {
                x >= n.x as f64
                    && y >= n.y as f64
                    && x <= n.x as f64 + n.width as f64
                    && y <= n.y as f64 + n.height as f64
            })
            .map(|n| n.id.clone())
            .or_else(|| {
                model
                    .edges
                    .iter()
                    .find(|edge| {
                        let Some(a) = model.nodes.iter().find(|n| n.id == edge.from) else {
                            return false;
                        };
                        let Some(b) = model.nodes.iter().find(|n| n.id == edge.to) else {
                            return false;
                        };
                        let (ax, ay) = port(
                            a,
                            edge.fields
                                .get("fromSide")
                                .and_then(|v| v.as_str())
                                .unwrap_or("right"),
                        );
                        let (bx, by) = port(
                            b,
                            edge.fields
                                .get("toSide")
                                .and_then(|v| v.as_str())
                                .unwrap_or("left"),
                        );
                        let t = ((x - ax) * (bx - ax) + (y - ay) * (by - ay))
                            / ((bx - ax).powi(2) + (by - ay).powi(2)).max(1.0);
                        let t = t.clamp(0.0, 1.0);
                        (x - (ax + t * (bx - ax))).hypot(y - (ay + t * (by - ay)))
                            < 8.0 / self.camera.get().zoom
                    })
                    .map(|e| e.id.clone())
            })
    }
    fn open(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(id) = self.selected.borrow().clone() else {
            return;
        };
        let model = self.canvas.borrow();
        if let Some(node) = model.nodes.iter().find(|n| n.id == id) {
            match node.kind.as_str() {
                "file" => {
                    let mut target = node.field("file").to_owned();
                    target.push_str(node.field("subpath"));
                    ui.open_target(self.root.join("Canvas.md"), target, false);
                }
                "link" => {
                    let url = node.field("url");
                    if url.starts_with("https://") || url.starts_with("http://") {
                        gtk::UriLauncher::new(url).launch(
                            Some(&self.window),
                            gio::Cancellable::NONE,
                            |_| {},
                        );
                    }
                }
                _ => {
                    drop(model);
                    self.edit();
                }
            }
        }
    }
    fn edit(self: &Rc<Self>) {
        if !self.writable() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(id) = self.selected.borrow().clone() else {
            return;
        };
        let model = self.canvas.borrow();
        let node = model.nodes.iter().find(|n| n.id == id).cloned();
        let edge = model.edges.iter().find(|e| e.id == id).cloned();
        drop(model);
        if node.is_none() && edge.is_none() {
            return;
        }
        let dialog = Prompt::new(
            &ui.window,
            "Edit canvas card",
            "Edit canvas card",
            "Text cards use Markdown. File paths are relative to the vault; external URLs open only through Open.",
            620,
            440,
        );
        dialog.window.set_transient_for(Some(&self.window));
        let input = gtk::TextView::builder()
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        let key = node
            .as_ref()
            .map(|n| match n.kind.as_str() {
                "text" => "text",
                "file" => "file",
                "link" => "url",
                _ => "label",
            })
            .unwrap_or("label")
            .to_owned();
        input.buffer().set_text(
            node.as_ref()
                .map(|n| n.field(&key))
                .or_else(|| {
                    edge.as_ref()
                        .and_then(|e| e.fields.get("label").and_then(|v| v.as_str()))
                })
                .unwrap_or(""),
        );
        let scroller = scroll(&input);
        scroller.set_min_content_height(170);
        scroller.set_vexpand(true);
        dialog.content_area().append(&scroller);
        let size = widgets::hbox(8);
        let width = gtk::SpinButton::with_range(80.0, 1000000.0, 20.0);
        let height = gtk::SpinButton::with_range(60.0, 1000000.0, 20.0);
        width.set_value(node.as_ref().map_or(320, |n| n.width) as f64);
        height.set_value(node.as_ref().map_or(220, |n| n.height) as f64);
        size.append(&widgets::label("Width", ""));
        size.append(&width);
        size.append(&widgets::label("Height", ""));
        size.append(&height);
        size.set_visible(node.is_some());
        dialog.content_area().append(&size);
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Apply", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        let base = self.revision.get();
        dialog.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let Some(view) = weak.upgrade()
            {
                if view.revision.get() != base {
                    view.status
                        .set_text("Canvas changed; reopen the card editor");
                    d.close();
                    return;
                }
                let b = input.buffer();
                let text = b.text(&b.start_iter(), &b.end_iter(), true);
                view.checkpoint();
                let mut model = view.canvas.borrow_mut();
                if let Some(n) = model.nodes.iter_mut().find(|n| n.id == id) {
                    n.set(&key, &text);
                    n.width = width.value_as_int() as i64;
                    n.height = height.value_as_int() as i64;
                } else if let Some(e) = model.edges.iter_mut().find(|e| e.id == id) {
                    e.fields
                        .insert("label".into(), serde_json::Value::String(text.to_string()));
                }
                drop(model);
                view.previews.borrow_mut().remove(&id);
                view.changed();
                view.assets();
            }
            d.close();
        });
        dialog.present();
    }
    fn files(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Add note or image cards")
            .build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            if let Ok(files) = result {
                let paths = (0..files.n_items())
                    .filter_map(|i| {
                        files
                            .item(i)
                            .and_downcast::<gio::File>()
                            .and_then(|f| f.path())
                    })
                    .collect();
                view.import(paths);
            }
        });
    }
    fn import(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if !self.writable() {
            return;
        }
        let root = self.root.clone();
        let folder = prefs::get().attachments_folder;
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<Vec<String>> {
                let mut names = Vec::new();
                for path in paths {
                    let canonical = path.canonicalize()?;
                    let path = if canonical.starts_with(&root) {
                        canonical
                    } else {
                        vault::workspace::attach(&root, &folder, &canonical)?
                    };
                    if path.components().any(|c| c.as_os_str() == ".obsidian") {
                        anyhow::bail!("Obsidian configuration is protected");
                    }
                    names.push(path.strip_prefix(&root)?.to_string_lossy().to_string());
                }
                Ok(names)
            },
            move |result| {
                if let Some(view) = weak.upgrade() {
                    match result {
                        Ok(names) => {
                            for name in names {
                                view.add("file", &name);
                            }
                        }
                        Err(e) => view.status.set_text(&format!("Add cards: {e:#}")),
                    }
                }
            },
        );
    }
    fn assets(self: &Rc<Self>) {
        let model = self.canvas.borrow().clone();
        for node in model.nodes {
            if node.kind != "file" || self.previews.borrow().contains_key(&node.id) {
                continue;
            }
            self.previews
                .borrow_mut()
                .insert(node.id.clone(), Preview::Text("Loading preview…".into()));
            let root = self.root.clone();
            let db = self.db.clone();
            let source = root.join("Canvas.md");
            let mut target = node.field("file").to_owned();
            target.push_str(node.field("subpath"));
            let weak = Rc::downgrade(self);
            worker::background(
                move || vault::workspace::asset(&root, &db, &source, &target, false),
                move |result| {
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let preview = match result {
                        Ok(vault::workspace::AssetData::Image { bytes, .. }) => {
                            let stream =
                                gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
                            match gtk::gdk_pixbuf::Pixbuf::from_stream(
                                &stream,
                                gio::Cancellable::NONE,
                            ) {
                                Ok(p) => Preview::Image(p),
                                Err(e) => Preview::Error(e.to_string()),
                            }
                        }
                        Ok(vault::workspace::AssetData::Markdown { blocks, .. }) => Preview::Text(
                            blocks
                                .into_iter()
                                .map(|b| b.text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ),
                        Ok(vault::workspace::AssetData::File { label, .. }) => Preview::Text(label),
                        Err(e) => Preview::Error(format!("{e:#}")),
                    };
                    view.previews.borrow_mut().insert(node.id, preview);
                    view.area.queue_draw();
                },
            );
        }
    }
    fn reload(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let d = Prompt::new(
            &ui.window,
            "Reload canvas",
            "Reload disk contents?",
            "This discards unsaved canvas changes. Save a copy first to keep them.",
            480,
            180,
        );
        d.window.set_transient_for(Some(&self.window));
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        d.add_button("Reload", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        d.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept
                && let Some(view) = weak.upgrade()
            {
                let root = view.root.clone();
                let path = view.path.clone();
                let state = nixe_core::paths::state_dir();
                let weak = Rc::downgrade(&view);
                worker::run(
                    move || -> anyhow::Result<_> {
                        let result = canvas::load(&root, &path)?;
                        vault::remove_draft(&state, &path)?;
                        Ok(result)
                    },
                    move |result| {
                        if let Some(v) = weak.upgrade() {
                            match result {
                                Ok((canvas, loaded)) => {
                                    v.checkpoint();
                                    *v.canvas.borrow_mut() = canvas;
                                    *v.version.borrow_mut() = loaded.version;
                                    v.bom.set(loaded.bom);
                                    v.conflict.set(false);
                                    v.dirty.set(false);
                                    v.previews.borrow_mut().clear();
                                    v.assets();
                                    v.area.queue_draw();
                                    v.status.set_text("Disk contents reloaded");
                                }
                                Err(e) => v.status.set_text(&format!("Reload: {e:#}")),
                            }
                        }
                    },
                );
            }
            d.close();
        });
        d.present();
    }
    fn close(self: &Rc<Self>) -> glib::Propagation {
        if self.allow_close.get() || !self.dirty.get() {
            return glib::Propagation::Proceed;
        }
        if self.asking.replace(true) {
            return glib::Propagation::Stop;
        }
        let Some(ui) = self.ui.upgrade() else {
            self.asking.set(false);
            return glib::Propagation::Stop;
        };
        let dialog = Prompt::new(
            &ui.window,
            "Close canvas",
            "Save canvas before closing?",
            "Your changes can also be kept as a local recovery draft.",
            480,
            180,
        );
        dialog.window.set_transient_for(Some(&self.window));
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button("Keep draft", gtk::ResponseType::Other(1));
        dialog.add_button("Save", gtk::ResponseType::Accept);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d, r| {
            if let Some(view) = weak.upgrade() {
                view.asking.set(false);
                match r {
                    gtk::ResponseType::Accept => {
                        let weak = Rc::downgrade(&view);
                        view.save(move |ok| {
                            if let Some(v) = weak.upgrade() {
                                if ok {
                                    v.allow_close.set(true);
                                    v.window.close();
                                } else if let Some(ui) = v.ui.upgrade() {
                                    ui.closing.set(false);
                                }
                            }
                        });
                    }
                    gtk::ResponseType::Other(1) => {
                        let weak = Rc::downgrade(&view);
                        view.persist(move || {
                            if let Some(v) = weak.upgrade() {
                                v.allow_close.set(true);
                                v.window.close();
                            }
                        });
                    }
                    _ => {
                        if let Some(ui) = view.ui.upgrade() {
                            ui.closing.set(false);
                        }
                    }
                }
            }
            d.close();
        });
        dialog.present();
        glib::Propagation::Stop
    }
    fn draw(&self, cr: &gtk::cairo::Context, w: i32, h: i32) {
        let palette = nixe_theme::palette();
        views::color(cr, &palette.bg, 1.0);
        let _ = cr.paint();
        let camera = self.camera.get();
        camera.transform(cr, w, h);
        let canvas = self.canvas.borrow();
        let selected = self.selected.borrow();
        let node_map = canvas
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect::<std::collections::HashMap<_, _>>();
        for edge in &canvas.edges {
            let (Some(a), Some(b)) = (
                node_map.get(edge.from.as_str()),
                node_map.get(edge.to.as_str()),
            ) else {
                continue;
            };
            let (ax, ay) = port(
                a,
                edge.fields
                    .get("fromSide")
                    .and_then(|v| v.as_str())
                    .unwrap_or("right"),
            );
            let (bx, by) = port(
                b,
                edge.fields
                    .get("toSide")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left"),
            );
            views::color(
                cr,
                if selected.as_deref() == Some(&edge.id) {
                    &palette.accent
                } else {
                    &palette.dim_text
                },
                0.8,
            );
            cr.set_line_width(1.4 / camera.zoom);
            cr.move_to(ax, ay);
            cr.line_to(bx, by);
            let _ = cr.stroke();
            if edge.fields.get("toEnd").and_then(|v| v.as_str()) != Some("none") {
                let angle = (by - ay).atan2(bx - ax);
                let size = 10.0 / camera.zoom.sqrt();
                cr.move_to(bx, by);
                cr.line_to(
                    bx - size * (angle - 0.4).cos(),
                    by - size * (angle - 0.4).sin(),
                );
                cr.line_to(
                    bx - size * (angle + 0.4).cos(),
                    by - size * (angle + 0.4).sin(),
                );
                cr.close_path();
                let _ = cr.fill();
            }
            if let Some(label) = edge.fields.get("label").and_then(|v| v.as_str()) {
                views::text(
                    cr,
                    label,
                    (ax + bx) / 2.0,
                    (ay + by) / 2.0,
                    200.0,
                    40.0,
                    12.0,
                );
            }
        }
        let left = (-w as f64 / 2.0 - camera.x) / camera.zoom;
        let top = (-h as f64 / 2.0 - camera.y) / camera.zoom;
        let right = left + w as f64 / camera.zoom;
        let bottom = top + h as f64 / camera.zoom;
        for node in &canvas.nodes {
            let (x, y, width, height) = (
                node.x as f64,
                node.y as f64,
                node.width as f64,
                node.height as f64,
            );
            if x + width < left || x > right || y + height < top || y > bottom {
                continue;
            }
            let _ = cr.save();
            cr.rectangle(x, y, width, height);
            views::color(
                cr,
                &palette.bg,
                if node.kind == "group" { 0.4 } else { 1.0 },
            );
            let _ = cr.fill_preserve();
            let border = if selected.as_deref() == Some(&node.id) {
                &palette.accent
            } else {
                &palette.border
            };
            views::color(cr, border, 1.0);
            cr.set_line_width(if selected.as_deref() == Some(&node.id) {
                2.0 / camera.zoom
            } else {
                1.0 / camera.zoom
            });
            let _ = cr.stroke();
            cr.rectangle(
                x + 8.0,
                y + 6.0,
                (width - 16.0).max(1.0),
                (height - 12.0).max(1.0),
            );
            cr.clip();
            views::color(cr, &palette.text, 1.0);
            if node.kind == "group" {
                views::text(
                    cr,
                    node.field("label"),
                    x + 12.0,
                    y + 8.0,
                    width - 24.0,
                    30.0,
                    14.0,
                );
            } else if node.kind == "file" {
                let file = node.field("file");
                views::text(cr, file, x + 10.0, y + 8.0, width - 20.0, 24.0, 12.0);
                let previews = self.previews.borrow();
                match previews.get(&node.id) {
                    Some(Preview::Image(pixbuf)) => {
                        let scale = ((width - 20.0) / pixbuf.width() as f64)
                            .min((height - 44.0) / pixbuf.height() as f64)
                            .max(0.001);
                        cr.translate(x + 10.0, y + 36.0);
                        cr.scale(scale, scale);
                        cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
                        let _ = cr.paint();
                    }
                    Some(Preview::Text(text)) => views::text(
                        cr,
                        text,
                        x + 10.0,
                        y + 36.0,
                        width - 20.0,
                        height - 46.0,
                        13.0,
                    ),
                    Some(Preview::Error(text)) => {
                        views::color(cr, &palette.danger, 1.0);
                        views::text(
                            cr,
                            text,
                            x + 10.0,
                            y + 36.0,
                            width - 20.0,
                            height - 46.0,
                            12.0,
                        );
                    }
                    None => {}
                }
            } else {
                let text = node.field(if node.kind == "link" { "url" } else { "text" });
                views::text(
                    cr,
                    text,
                    x + 12.0,
                    y + 12.0,
                    width - 24.0,
                    height - 24.0,
                    14.0,
                );
            }
            let _ = cr.restore();
        }
    }
}
fn port(node: &Node, side: &str) -> (f64, f64) {
    let (x, y, w, h) = (
        node.x as f64,
        node.y as f64,
        node.width as f64,
        node.height as f64,
    );
    match side {
        "top" => (x + w / 2.0, y),
        "bottom" => (x + w / 2.0, y + h),
        "left" => (x, y + h / 2.0),
        _ => (x + w, y + h / 2.0),
    }
}
impl Ui {
    pub(super) fn new_canvas(self: &Rc<Self>) {
        let Some(root) = self.root.borrow().clone() else {
            self.message("Open a vault first");
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title("Create JSON Canvas")
            .initial_name("Untitled.canvas")
            .initial_folder(&gio::File::for_path(&root))
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Ok(file) = result
                && let Some(mut path) = file.path()
            {
                if path.extension().is_none() {
                    path.set_extension("canvas");
                }
                let requested = path.clone();
                let weak = Rc::downgrade(&ui);
                worker::run(
                    move || -> anyhow::Result<()> {
                        let parent = path
                            .parent()
                            .context("No canvas directory")?
                            .canonicalize()?;
                        if !parent.starts_with(&root)
                            || path.components().any(|c| c.as_os_str() == ".obsidian")
                        {
                            anyhow::bail!("Create the canvas inside the vault");
                        }
                        vault::save_copy(&path, "{\"nodes\": [], \"edges\": []}\n", false)
                    },
                    move |result| {
                        if let Some(ui) = weak.upgrade() {
                            match result {
                                Ok(()) => {
                                    ui.populate_root();
                                    ui.open_canvas(requested);
                                }
                                Err(e) => ui.message(&format!("Create canvas: {e:#}")),
                            }
                        }
                    },
                );
            }
        });
    }
    pub(super) fn choose_canvas(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open JSON Canvas").build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if let (Some(ui), Ok(file)) = (weak.upgrade(), result)
                && let Some(path) = file.path()
            {
                ui.open_canvas(path);
            }
        });
    }
    pub(super) fn open_canvas(self: &Rc<Self>, path: PathBuf) {
        if let Some((window, _)) = self.canvases.borrow().get(&path) {
            window.present();
            return;
        }
        let (Some(root), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone()) else {
            self.message("Open the canvas's vault first");
            return;
        };
        if self.opening.borrow().contains(&path) {
            return;
        }
        self.opening.borrow_mut().push(path.clone());
        let requested = path.clone();
        let r = root.clone();
        let state = nixe_core::paths::state_dir();
        let weak = Rc::downgrade(self);
        worker::run(
            move || -> anyhow::Result<_> {
                let (canvas, loaded) = canvas::load(&r, &path)?;
                let draft = vault::read_draft(&state, &path)?;
                Ok((canvas, loaded, draft))
            },
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    ui.opening.borrow_mut().retain(|p| p != &requested);
                    match result {
                        Ok((canvas, loaded, draft)) => {
                            ui.canvas_window(root, db, canvas, loaded, draft)
                        }
                        Err(e) => ui.message(&format!("Canvas: {e:#}")),
                    }
                }
            },
        );
    }
    fn canvas_window(
        self: &Rc<Self>,
        root_path: PathBuf,
        db: PathBuf,
        canvas: Canvas,
        loaded: Loaded,
        draft: Option<Draft>,
    ) {
        let (window, root, toolbar) = self.workspace_window(&format!(
            "{} — Notes",
            loaded
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        let area = gtk::DrawingArea::new();
        area.set_vexpand(true);
        area.set_hexpand(true);
        area.set_focusable(true);
        root.append(&area);
        let status = gtk::Label::new(Some(
            "Scroll: zoom · middle-drag: pan · drag cards: move · double-click: edit/open · Delete: remove",
        ));
        status.add_css_class("status-bar");
        root.append(&status);
        let view = Rc::new(CanvasView {
            ui: Rc::downgrade(self),
            window: window.clone(),
            area: area.clone(),
            status,
            root: root_path,
            db,
            path: loaded.path.clone(),
            canvas: RefCell::new(canvas),
            version: RefCell::new(loaded.version),
            bom: Cell::new(loaded.bom),
            read_only: loaded.read_only,
            revision: Cell::new(0),
            dirty: Rc::new(Cell::new(false)),
            saving: Cell::new(false),
            conflict: Cell::new(false),
            allow_close: Cell::new(false),
            asking: Cell::new(false),
            selected: RefCell::new(None),
            connect_from: RefCell::new(None),
            camera: Rc::new(Cell::new(Camera::default())),
            undo: RefCell::new(Vec::new()),
            redo: RefCell::new(Vec::new()),
            previews: RefCell::new(std::collections::HashMap::new()),
            save_timer: RefCell::new(None),
            draft_timer: RefCell::new(None),
        });
        for (label, action) in [
            ("Text", 0),
            ("Files…", 1),
            ("Link", 2),
            ("Group", 3),
            ("Connect", 4),
            ("Edit", 5),
            ("Open", 6),
            ("Delete", 7),
            ("Undo", 8),
            ("Redo", 9),
            ("Save", 10),
            ("Copy…", 11),
            ("Reload", 12),
            ("Fit", 13),
        ] {
            let button = gtk::Button::with_label(label);
            button.set_has_frame(false);
            let weak = Rc::downgrade(&view);
            button.connect_clicked(move |_| {
                if let Some(v) = weak.upgrade() {
                    match action {
                        0 => v.add("text", "New Markdown card"),
                        1 => v.files(),
                        2 => v.add("link", "https://"),
                        3 => v.add("group", "Group"),
                        4 => v.connect(),
                        5 => v.edit(),
                        6 => v.open(),
                        7 => v.remove(),
                        8 => v.undo(false),
                        9 => v.undo(true),
                        10 => v.save(|_| {}),
                        11 => v.copy(),
                        12 => v.reload(),
                        _ => {
                            let model = v.canvas.borrow();
                            if model.nodes.is_empty() {
                                v.camera.set(Camera::default());
                            } else {
                                let minx = model.nodes.iter().map(|n| n.x).min().unwrap() as f64;
                                let miny = model.nodes.iter().map(|n| n.y).min().unwrap() as f64;
                                let maxx =
                                    model.nodes.iter().map(|n| n.x + n.width).max().unwrap() as f64;
                                let maxy = model.nodes.iter().map(|n| n.y + n.height).max().unwrap()
                                    as f64;
                                let zoom = ((v.area.width() as f64 - 80.0) / (maxx - minx))
                                    .min((v.area.height() as f64 - 80.0) / (maxy - miny))
                                    .clamp(0.05, 3.0);
                                v.camera.set(Camera {
                                    zoom,
                                    x: -(minx + maxx) / 2.0 * zoom,
                                    y: -(miny + maxy) / 2.0 * zoom,
                                });
                            }
                            v.area.queue_draw();
                        }
                    }
                }
            });
            toolbar.append(&button);
        }
        // Keep controls usable when tiled: toolbar scrolls without visible bars.
        toolbar.set_hexpand(false);
        root.remove(&toolbar);
        let toolbar_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&toolbar)
            .build();
        root.prepend(&toolbar_scroll);
        let weak = Rc::downgrade(&view);
        area.set_draw_func(move |_, cr, w, h| {
            if let Some(v) = weak.upgrade() {
                v.draw(cr, w, h);
            }
        });
        views::pan_zoom(&area, view.camera.clone());
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let weak = Rc::downgrade(&view);
        click.connect_pressed(move |_, count, x, y| {
            if let Some(v) = weak.upgrade() {
                v.area.grab_focus();
                let (x, y) = v.camera.get().world(&v.area, x, y);
                let hit = v.hit(x, y);
                if let Some(from) = v.connect_from.borrow_mut().take()
                    && let Some(to) = hit.clone()
                {
                    v.checkpoint();
                    match v.canvas.borrow_mut().connect(&from, &to) {
                        Ok(()) => v.changed(),
                        Err(e) => v.status.set_text(&e.to_string()),
                    }
                }
                *v.selected.borrow_mut() = hit;
                v.area.queue_draw();
                if count == 2 {
                    let file = v.selected.borrow().as_ref().is_some_and(|id| {
                        v.canvas
                            .borrow()
                            .nodes
                            .iter()
                            .any(|n| &n.id == id && n.kind == "file")
                    });
                    if file {
                        v.open();
                    } else {
                        v.edit();
                    }
                }
            }
        });
        area.add_controller(click);
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        type DragState = (Canvas, Vec<(String, i64, i64)>);
        let origin = Rc::new(RefCell::new(None::<DragState>));
        let weak = Rc::downgrade(&view);
        let initial = origin.clone();
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(v) = weak.upgrade()
                && v.writable()
            {
                let (x, y) = v.camera.get().world(&v.area, x, y);
                let Some(id) = v.hit(x, y) else {
                    return;
                };
                let model = v.canvas.borrow();
                let Some(node) = model.nodes.iter().find(|n| n.id == id) else {
                    return;
                };
                let positions = model
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.id == id
                            || (node.kind == "group"
                                && n.x >= node.x
                                && n.y >= node.y
                                && n.x + n.width <= node.x + node.width
                                && n.y + n.height <= node.y + node.height)
                    })
                    .map(|n| (n.id.clone(), n.x, n.y))
                    .collect();
                *initial.borrow_mut() = Some((model.clone(), positions));
            }
        });
        let weak = Rc::downgrade(&view);
        let initial = origin.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(v) = weak.upgrade()
                && let Some((_, positions)) = initial.borrow().as_ref()
            {
                let zoom = v.camera.get().zoom;
                let mut model = v.canvas.borrow_mut();
                for (id, x, y) in positions {
                    if let Some(n) = model.nodes.iter_mut().find(|n| &n.id == id) {
                        n.x = (*x as f64 + dx / zoom).clamp(-1e9, 1e9) as i64;
                        n.y = (*y as f64 + dy / zoom).clamp(-1e9, 1e9) as i64;
                    }
                }
                v.area.queue_draw();
            }
        });
        let weak = Rc::downgrade(&view);
        drag.connect_drag_end(move |_, dx, dy| {
            if let Some(v) = weak.upgrade()
                && let Some((before, _)) = origin.borrow_mut().take()
            {
                if dx.hypot(dy) > 2.0 {
                    v.undo.borrow_mut().push(before);
                    v.redo.borrow_mut().clear();
                    v.changed();
                } else {
                    *v.canvas.borrow_mut() = before;
                    v.area.queue_draw();
                }
            }
        });
        area.add_controller(drag);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(v) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Delete => v.remove(),
                gdk::Key::Return if ctrl => v.open(),
                gdk::Key::Return => v.edit(),
                gdk::Key::Tab => {
                    let model = v.canvas.borrow();
                    let current = v
                        .selected
                        .borrow()
                        .as_ref()
                        .and_then(|id| model.nodes.iter().position(|n| &n.id == id));
                    if !model.nodes.is_empty() {
                        let next = if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                            current.map_or(0, |i| (i + model.nodes.len() - 1) % model.nodes.len())
                        } else {
                            current.map_or(0, |i| (i + 1) % model.nodes.len())
                        };
                        *v.selected.borrow_mut() = Some(model.nodes[next].id.clone());
                        let node = &model.nodes[next];
                        let mut camera = v.camera.get();
                        camera.x = -(node.x as f64 + node.width as f64 / 2.0) * camera.zoom;
                        camera.y = -(node.y as f64 + node.height as f64 / 2.0) * camera.zoom;
                        v.camera.set(camera);
                        v.area.queue_draw();
                    }
                }
                gdk::Key::z | gdk::Key::Z if ctrl => {
                    v.undo(mods.contains(gdk::ModifierType::SHIFT_MASK))
                }
                gdk::Key::s | gdk::Key::S if ctrl => v.save(|_| {}),
                gdk::Key::Escape => {
                    v.selected.borrow_mut().take();
                    v.connect_from.borrow_mut().take();
                    v.area.queue_draw();
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        area.add_controller(keys);
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(&view);
        drop.connect_drop(move |_, value, _, _| {
            if let (Some(view), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) {
                view.import(files.files().into_iter().filter_map(|f| f.path()).collect());
                true
            } else {
                false
            }
        });
        area.add_controller(drop);
        let weak = Rc::downgrade(&view);
        window.connect_close_request(move |_| {
            weak.upgrade()
                .map_or(glib::Propagation::Proceed, |v| v.close())
        });
        let weak = Rc::downgrade(self);
        let path = loaded.path;
        self.canvases
            .borrow_mut()
            .insert(path.clone(), (window.clone(), view.dirty.clone()));
        let keep = RefCell::new(Some(view.clone()));
        window.connect_destroy(move |_| {
            if let Some(v) = keep.borrow_mut().take() {
                for timer in [&v.save_timer, &v.draft_timer] {
                    if let Some(t) = timer.borrow_mut().take() {
                        t.remove();
                    }
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.canvases.borrow_mut().remove(&path);
            }
        });
        let weak = area.downgrade();
        nixe_theme::subscribe(&area, move || {
            if let Some(a) = weak.upgrade() {
                a.queue_draw();
            }
        });
        view.assets();
        window.present();
        if let Some(draft) = draft.filter(|d| {
            d.text != serde_json::to_string_pretty(&*view.canvas.borrow()).unwrap_or_default()
        }) {
            let dialog = Prompt::new(
                &self.window,
                "Canvas recovery",
                "A canvas draft is available",
                "Restore the retained draft, or keep the current disk version. External changes always require a separate copy.",
                520,
                200,
            );
            dialog.window.set_transient_for(Some(&window));
            dialog.add_button("Keep disk", gtk::ResponseType::Cancel);
            dialog.add_button("Restore draft", gtk::ResponseType::Accept);
            let weak = Rc::downgrade(&view);
            dialog.connect_response(move |d,r|{if r==gtk::ResponseType::Accept&&let Some(v)=weak.upgrade(){match Canvas::parse(&draft.text){Ok(model)=>{v.checkpoint();*v.canvas.borrow_mut()=model;let conflict=v.version.borrow().digest.to_hex().as_str()!=draft.base_digest;v.conflict.set(conflict);v.bom.set(draft.bom);v.previews.borrow_mut().clear();v.changed();v.assets();if conflict{v.status.set_text("Draft restored; disk has external changes. Save a copy to retain both versions.");}},Err(e)=>v.status.set_text(&format!("Draft could not be parsed: {e:#}. It remains on disk."))}}d.close();});
            dialog.present();
        }
    }
}
