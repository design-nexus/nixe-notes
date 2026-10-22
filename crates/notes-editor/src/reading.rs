use gtk::{gdk, gio, glib, prelude::*};
use notes_markdown::render::{ReadBlock, ReadKind};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};
#[derive(Clone)]
pub enum Asset {
    Image(Vec<u8>),
    Markdown {
        path: PathBuf,
        blocks: Vec<ReadBlock>,
    },
    File {
        path: PathBuf,
        label: String,
    },
    Error(String),
}
pub type AssetDone = Box<dyn FnOnce(Asset)>;
pub type AssetHandler = Rc<dyn Fn(PathBuf, String, bool, AssetDone)>;
type TaskHandler = Rc<dyn Fn(usize)>;
pub type LinkHandler = Rc<dyn Fn(PathBuf, String, bool)>;
pub struct Reader {
    pub root: gtk::ScrolledWindow,
    pub view: gtk::ListView,
    model: gtk::StringList,
    blocks: RefCell<Vec<ReadBlock>>,
    path: RefCell<PathBuf>,
    ancestors: Vec<PathBuf>,
    assets: RefCell<HashMap<String, Asset>>,
    children: RefCell<Vec<Rc<Reader>>>,
    pub writable: std::cell::Cell<bool>,
    pub light: std::cell::Cell<bool>,
    pub on_asset: RefCell<Option<AssetHandler>>,
    pub on_link: RefCell<Option<LinkHandler>>,
    pub on_task: RefCell<Option<TaskHandler>>,
}
impl Reader {
    pub fn new(ancestors: Vec<PathBuf>) -> Rc<Self> {
        let model = gtk::StringList::new(&[]);
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::new(Some(selection), Some(factory.clone()));
        view.add_css_class("notes-reader");
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .hexpand(true)
            .vexpand(true)
            .child(&view)
            .build();
        let reader = Rc::new(Self {
            root,
            view,
            model,
            blocks: RefCell::new(Vec::new()),
            path: RefCell::new(PathBuf::new()),
            ancestors,
            assets: RefCell::new(HashMap::new()),
            children: RefCell::new(Vec::new()),
            writable: std::cell::Cell::new(true),
            light: std::cell::Cell::new(false),
            on_asset: RefCell::new(None),
            on_link: RefCell::new(None),
            on_task: RefCell::new(None),
        });
        let weak = Rc::downgrade(&reader);
        factory.connect_bind(move |_, item| {
            if let (Some(reader), Some(item)) =
                (weak.upgrade(), item.downcast_ref::<gtk::ListItem>())
            {
                let block = reader
                    .blocks
                    .borrow()
                    .get(item.position() as usize)
                    .cloned();
                if let Some(block) = block {
                    item.set_child(Some(&reader.widget(&block)));
                }
            }
        });
        reader
    }
    pub fn set(&self, path: PathBuf, blocks: Vec<ReadBlock>) {
        self.children.borrow_mut().clear();
        self.assets.borrow_mut().clear();
        *self.path.borrow_mut() = path;
        let ids = (0..blocks.len()).map(|i| i.to_string()).collect::<Vec<_>>();
        *self.blocks.borrow_mut() = blocks;
        self.model.splice(
            0,
            self.model.n_items(),
            &ids.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }
    pub fn jump(&self, byte: usize) {
        if let Some(position) = self.blocks.borrow().iter().rposition(|b| b.offset <= byte) {
            self.view
                .scroll_to(position as u32, gtk::ListScrollFlags::FOCUS, None);
        }
    }
    fn widget(self: &Rc<Self>, block: &ReadBlock) -> gtk::Widget {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
        container.set_margin_top(6);
        container.set_margin_bottom(6);
        container.set_margin_start(24);
        container.set_margin_end(24);
        match &block.kind {
            ReadKind::Rule => container.append(&gtk::Separator::new(gtk::Orientation::Horizontal)),
            ReadKind::Code(language) => {
                let buffer = sourceview5::Buffer::new(None);
                use sourceview5::prelude::*;
                buffer.set_text(&block.text);
                buffer.set_language(
                    sourceview5::LanguageManager::default()
                        .language(language.split_whitespace().next().unwrap_or(""))
                        .as_ref(),
                );
                buffer.set_style_scheme(
                    sourceview5::StyleSchemeManager::default()
                        .scheme(if self.light.get() {
                            "Adwaita"
                        } else {
                            "Adwaita-dark"
                        })
                        .as_ref(),
                );
                let view = sourceview5::View::with_buffer(&buffer);
                view.add_css_class("notes-editor");
                view.set_editable(false);
                view.set_monospace(true);
                view.set_wrap_mode(gtk::WrapMode::WordChar);
                let scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vscrollbar_policy(gtk::PolicyType::External)
                    .min_content_height(48)
                    .max_content_height(280)
                    .propagate_natural_height(true)
                    .child(&view)
                    .build();
                container.append(&scroll);
            }
            ReadKind::Embed { target, wiki } => {
                let title = gtk::Label::new(Some(&format!("Loading {target}…")));
                title.set_wrap(true);
                container.append(&title);
                let key = target.clone();
                let cached = self.assets.borrow().get(&key).cloned();
                if let Some(asset) = cached {
                    self.asset_widget(&container, &title, asset);
                } else if let Some(handler) = self.on_asset.borrow().clone() {
                    let weak = Rc::downgrade(self);
                    let slot = container.clone();
                    let title = title.clone();
                    handler(
                        self.path.borrow().clone(),
                        target.clone(),
                        *wiki,
                        Box::new(move |asset| {
                            if let Some(reader) = weak.upgrade() {
                                reader.assets.borrow_mut().insert(key, asset.clone());
                                reader.asset_widget(&slot, &title, asset);
                            }
                        }),
                    );
                }
            }
            ReadKind::Task { checked, offset } => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let button = gtk::CheckButton::new();
                button.set_active(*checked);
                button.set_sensitive(self.writable.get() && self.on_task.borrow().is_some());
                let action = self.on_task.borrow().clone();
                let at = *offset;
                button.connect_toggled(move |_| {
                    if let Some(action) = &action {
                        action(at);
                    }
                });
                row.append(&button);
                row.append(&self.label(block));
                container.append(&row);
            }
            _ => {
                let label = self.label(block);
                if let ReadKind::Heading(level) = block.kind {
                    label.add_css_class(&format!("notes-heading-{level}"));
                }
                if matches!(block.kind, ReadKind::Quote) {
                    container.add_css_class("notes-callout");
                }
                if matches!(block.kind, ReadKind::Table) {
                    let grid = gtk::Grid::new();
                    grid.set_column_spacing(0);
                    grid.set_row_spacing(0);
                    for (row, line) in block
                        .text
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .enumerate()
                    {
                        for (column, cell) in
                            line.trim_end_matches("  │  ").split("  │  ").enumerate()
                        {
                            let value = gtk::Label::new(Some(cell));
                            value.set_xalign(0.0);
                            value.set_wrap(true);
                            value.set_hexpand(true);
                            value.add_css_class("notes-table-cell");
                            if row == 0 {
                                value.add_css_class("heading");
                            }
                            grid.attach(&value, column as i32, row as i32, 1, 1);
                        }
                    }
                    let scroll = gtk::ScrolledWindow::builder()
                        .hscrollbar_policy(gtk::PolicyType::External)
                        .vscrollbar_policy(gtk::PolicyType::Never)
                        .child(&grid)
                        .build();
                    container.append(&scroll);
                } else {
                    container.append(&label);
                }
            }
        }
        container.upcast()
    }
    fn label(&self, block: &ReadBlock) -> gtk::Label {
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_selectable(true);
        label.set_markup(&block.markup);
        let handler = self.on_link.borrow().clone();
        let source = self.path.borrow().clone();
        label.connect_activate_link(move |_, target| {
            if let Some(handler) = &handler {
                let (wiki, target) = if let Some(target) = target.strip_prefix("notes-wiki:") {
                    (true, target)
                } else {
                    (false, target.strip_prefix("notes-md:").unwrap_or(target))
                };
                handler(source.clone(), target.to_owned(), wiki);
            }
            glib::Propagation::Stop
        });
        label
    }
    fn asset_widget(self: &Rc<Self>, slot: &gtk::Box, title: &gtk::Label, asset: Asset) {
        if title.parent().is_some() {
            slot.remove(title);
        }
        match asset {
            Asset::Image(bytes) => {
                match gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                    Ok(texture) => {
                        let picture = gtk::Picture::for_paintable(&texture);
                        picture.set_can_shrink(true);
                        picture.set_content_fit(gtk::ContentFit::Contain);
                        picture.set_height_request(texture.height().min(360));
                        slot.append(&picture);
                    }
                    Err(error) => slot.append(&gtk::Label::new(Some(&format!(
                        "Image could not be decoded: {error}"
                    )))),
                }
            }
            Asset::Markdown { path, blocks } => {
                if self.ancestors.len() >= 4 || self.ancestors.contains(&path) {
                    slot.append(&gtk::Label::new(Some(
                        "Recursive embed — open the note to continue",
                    )));
                    return;
                }
                let mut ancestors = self.ancestors.clone();
                ancestors.push(self.path.borrow().clone());
                let child = Reader::new(ancestors);
                *child.on_asset.borrow_mut() = self.on_asset.borrow().clone();
                *child.on_link.borrow_mut() = self.on_link.borrow().clone();
                child.set(path, blocks);
                child.root.set_min_content_height(120);
                child.root.set_max_content_height(360);
                child.root.set_vexpand(false);
                slot.append(&child.root);
                self.children.borrow_mut().push(child);
            }
            Asset::File { path, label } => {
                let button = gtk::Button::with_label(&label);
                button.connect_clicked(move |_| {
                    let file = gio::File::for_path(&path);
                    gtk::FileLauncher::new(Some(&file)).launch(
                        gtk::Window::NONE,
                        gio::Cancellable::NONE,
                        |_| {},
                    );
                });
                slot.append(&button);
            }
            Asset::Error(error) => {
                let label = gtk::Label::new(Some(&error));
                label.set_wrap(true);
                slot.append(&label);
            }
        }
    }
}
