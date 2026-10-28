//! A source-preserving editor. Preview tags never replace text or touch undo history.
use gtk::{glib, pango};
use notes_markdown::{Kind, Parsed};
pub use notes_markdown::{Link, character_offset};
use sourceview5::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

mod reading;
use reading::Reader;
pub use reading::{Asset, AssetDone, AssetHandler, LinkHandler};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Source,
    Live,
    Reading,
}

type ParsedCallback = Box<dyn Fn(&Parsed)>;
pub struct Editor {
    pub buffer: sourceview5::Buffer,
    pub view: sourceview5::View,
    pub root: gtk::Box,
    pub scroll: gtk::ScrolledWindow,
    stack: gtk::Stack,
    reader: Rc<Reader>,
    preview: Rc<Reader>,
    preview_reveal: gtk::Revealer,
    preview_key: RefCell<Option<String>>,
    mode: Cell<Mode>,
    source_path: RefCell<std::path::PathBuf>,
    writable: Cell<bool>,
    hidden_tag: gtk::TextTag,
    fold_tag: gtk::TextTag,
    /// Live: the front matter, small and quiet above the note.
    frontmatter_tag: gtk::TextTag,
    fold_marks: RefCell<Vec<(gtk::TextMark, gtk::TextMark)>>,
    pub revision: Cell<u64>,
    live: Cell<bool>,
    writing_width: Cell<i32>,
    typewriter: Cell<bool>,
    parsing: Cell<bool>,
    pending: RefCell<Option<glib::SourceId>>,
    parsed: RefCell<Parsed>,
    parsed_revision: Cell<u64>,
    rendered: RefCell<Option<(gtk::TextMark, gtk::TextMark)>>,
    render_pending: Cell<bool>,
    render_cache: RefCell<Vec<(Kind, i32, i32)>>,
    tags: Vec<(Kind, gtk::TextTag)>,
    on_parsed: RefCell<Option<ParsedCallback>>,
}
impl Editor {
    pub fn new(text: &str) -> Rc<Self> {
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("markdown")
                .as_ref(),
        );
        buffer.set_text(text);
        buffer.set_modified(false);
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_auto_indent(true);
        view.set_indent_on_tab(true);
        view.set_tab_width(4);
        view.set_insert_spaces_instead_of_tabs(true);
        view.set_top_margin(24);
        view.set_bottom_margin(120);
        view.set_left_margin(28);
        view.set_right_margin(28);
        view.add_css_class("notes-editor");
        let mut tags = Vec::new();
        for (kind, name) in [
            (Kind::Heading, "heading"),
            (Kind::Strong, "strong"),
            (Kind::Emphasis, "emphasis"),
            (Kind::Link, "link"),
            (Kind::Code, "code"),
            (Kind::CodeBlock, "codeblock"),
            (Kind::Strike, "strike"),
        ] {
            let tag = gtk::TextTag::builder().name(name).build();
            match kind {
                Kind::Heading => {
                    tag.set_weight(700);
                    // Keep line geometry stable when formatting enters/leaves the viewport.
                }
                Kind::Strong => tag.set_weight(700),
                Kind::Emphasis => tag.set_style(pango::Style::Italic),
                Kind::Strike => tag.set_strikethrough(true),
                Kind::Link => tag.set_underline(pango::Underline::Single),
                Kind::Code | Kind::CodeBlock => tag.set_family(Some("monospace")),
            }
            buffer.tag_table().add(&tag);
            tags.push((kind, tag));
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .hexpand(true)
            .vexpand(true)
            .child(&view)
            .build();
        let reader = Reader::new(Vec::new());
        let stack = gtk::Stack::new();
        stack.add_named(&scroll, Some("edit"));
        stack.add_named(&reader.root, Some("read"));
        stack.set_vexpand(true);
        let preview = Reader::new(Vec::new());
        preview.root.set_min_content_height(100);
        preview.root.set_max_content_height(240);
        preview.root.set_vexpand(false);
        let preview_reveal = gtk::Revealer::new();
        preview_reveal.set_child(Some(&preview.root));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&stack);
        root.append(&preview_reveal);
        root.set_vexpand(true);
        root.set_hexpand(true);
        let hidden_tag = gtk::TextTag::builder()
            .name("markdown-delimiter")
            .invisible(true)
            .build();
        buffer.tag_table().add(&hidden_tag);
        let fold_tag = gtk::TextTag::builder()
            .name("folded-heading")
            .invisible(true)
            .build();
        buffer.tag_table().add(&fold_tag);
        let frontmatter_tag = gtk::TextTag::builder()
            .name("frontmatter")
            .scale(0.85)
            .build();
        buffer.tag_table().add(&frontmatter_tag);
        let editor = Rc::new(Self {
            buffer,
            view,
            root,
            scroll,
            stack,
            reader,
            preview,
            preview_reveal,
            preview_key: RefCell::new(None),
            mode: Cell::new(Mode::Source),
            source_path: RefCell::new(std::path::PathBuf::new()),
            writable: Cell::new(true),
            hidden_tag,
            fold_tag,
            frontmatter_tag,
            fold_marks: RefCell::new(Vec::new()),
            revision: Cell::new(0),
            live: Cell::new(false),
            writing_width: Cell::new(0),
            typewriter: Cell::new(false),
            parsing: Cell::new(false),
            pending: RefCell::new(None),
            parsed: RefCell::new(Parsed::default()),
            parsed_revision: Cell::new(u64::MAX),
            rendered: RefCell::new(None),
            render_pending: Cell::new(false),
            render_cache: RefCell::new(Vec::new()),
            tags,
            on_parsed: RefCell::new(None),
        });
        let weak = Rc::downgrade(&editor);
        editor.buffer.connect_changed(move |_| {
            if let Some(e) = weak.upgrade() {
                e.revision.set(e.revision.get() + 1);
                e.render_cache.borrow_mut().clear();
                e.schedule();
            }
        });
        let weak = Rc::downgrade(&editor);
        editor.view.add_tick_callback(move |view, _| {
            let Some(editor) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if editor.typewriter.get() {
                let margin = (view.height() / 2).max(120);
                if view.bottom_margin() != margin {
                    view.set_bottom_margin(margin);
                    view.set_top_margin(margin);
                }
            }
            let columns = editor.writing_width.get();
            let glyph = view.create_pango_layout(Some("M")).pixel_size().0;
            let margin = if columns > 0 {
                ((view.width() - columns * glyph) / 2).max(16)
            } else {
                28
            };
            if view.left_margin() != margin {
                view.set_left_margin(margin);
                view.set_right_margin(margin);
            }
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(&editor);
        editor.scroll.vadjustment().connect_value_changed(move |_| {
            if let Some(e) = weak.upgrade() {
                e.schedule_render();
            }
        });
        let weak = Rc::downgrade(&editor);
        editor.scroll.vadjustment().connect_changed(move |_| {
            if let Some(e) = weak.upgrade() {
                e.schedule_render();
            }
        });
        let weak = Rc::downgrade(&editor);
        editor.buffer.connect_mark_set(move |_, _, mark| {
            if matches!(mark.name().as_deref(), Some("insert" | "selection_bound"))
                && let Some(e) = weak.upgrade()
            {
                e.render_cache.borrow_mut().clear();
                e.schedule_render();
                if e.typewriter.get()
                    && mark.name().as_deref() == Some("insert")
                    && e.view.has_focus()
                {
                    e.view
                        .scroll_to_mark(&e.buffer.get_insert(), 0.0, true, 0.0, 0.5);
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&editor);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(e) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !e.writable.get() || e.mode.get() == Mode::Reading {
                return glib::Propagation::Proceed;
            }
            if key == gtk::gdk::Key::Return
                && !modifiers.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::SHIFT_MASK
                        | gtk::gdk::ModifierType::ALT_MASK,
                )
                && e.continue_list()
            {
                return glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::t
                && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                && e.toggle_task()
            {
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        editor.view.add_controller(keys);
        let weak = Rc::downgrade(&editor);
        *editor.reader.on_task.borrow_mut() = Some(Rc::new(move |at| {
            if let Some(e) = weak.upgrade() {
                e.toggle_task_at(at);
            }
        }));
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let weak = Rc::downgrade(&editor);
        click.connect_released(move |_, count, x, y| {
            let Some(e) = weak.upgrade() else { return };
            if count != 1
                || e.mode.get() != Mode::Live
                || !e.writable.get()
                || e.parsed_revision.get() != e.revision.get()
            {
                return;
            }
            let (x, y) =
                e.view
                    .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = e.view.iter_at_location(x, y) {
                let text = e.text();
                let byte = text
                    .char_indices()
                    .nth(iter.offset() as usize)
                    .map_or(text.len(), |(b, _)| b);
                let at = e
                    .parsed
                    .borrow()
                    .tasks
                    .iter()
                    .find(|t| byte >= t.marker.start.saturating_sub(1) && byte <= t.marker.end)
                    .map(|t| t.marker.start);
                if let Some(at) = at {
                    e.toggle_task_at(at);
                }
            }
        });
        editor.view.add_controller(click);
        editor.schedule();
        editor
    }
    pub fn scroll_position(&self) -> f64 {
        if self.mode.get() == Mode::Reading {
            self.reader.root.vadjustment().value()
        } else {
            self.scroll.vadjustment().value()
        }
    }
    pub fn restore_scroll(self: &Rc<Self>, position: f64) {
        let weak = Rc::downgrade(self);
        self.root.add_tick_callback(move |root, _| {
            let Some(editor) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if root.height() == 0 {
                return glib::ControlFlow::Continue;
            }
            let adjustment = if editor.mode.get() == Mode::Reading {
                editor.reader.root.vadjustment()
            } else {
                editor.scroll.vadjustment()
            };
            adjustment.set_value(position.max(0.0));
            glib::ControlFlow::Break
        });
    }
    pub fn replace_range(&self, range: std::ops::Range<usize>, replacement: &str) -> bool {
        self.apply_edit(notes_markdown::editing::Edit {
            range,
            replacement: replacement.to_owned(),
        })
    }
    pub fn set_typewriter(&self, enabled: bool) {
        self.typewriter.set(enabled);
        self.view.set_top_margin(if enabled {
            self.view.height().max(240) / 2
        } else {
            24
        });
        self.view.set_bottom_margin(if enabled {
            self.view.height().max(240) / 2
        } else {
            120
        });
    }
    pub fn set_writing_width(&self, columns: i32) {
        self.writing_width.set(columns);
    }
    pub fn text(&self) -> String {
        self.buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true)
            .to_string()
    }
    pub fn set_live(&self, live: bool) {
        self.set_mode(if live { Mode::Live } else { Mode::Source });
    }
    fn apply_live(&self, live: bool) {
        self.clear_rendered();
        self.live.set(live);
        self.buffer.set_highlight_syntax(!live);
        self.view.set_show_line_numbers(!live);
        // Live reads as prose in the interface font; code keeps a monospace face.
        self.view.set_monospace(!live);
        if live {
            self.view.add_css_class("prose");
        } else {
            self.view.remove_css_class("prose");
        }
        self.apply(&self.parsed.borrow());
    }
    pub fn refresh_reading(&self) {
        if self.mode.get() == Mode::Reading {
            self.reader.set(
                self.source_path.borrow().clone(),
                self.parsed.borrow().reading.clone(),
            );
        }
        *self.preview_key.borrow_mut() = None;
    }
    pub fn set_mode(&self, mode: Mode) {
        self.mode.set(mode);
        if mode != Mode::Live {
            self.preview_reveal.set_reveal_child(false);
        }
        self.apply_live(mode == Mode::Live);
        self.stack.set_visible_child_name(if mode == Mode::Reading {
            "read"
        } else {
            "edit"
        });
        self.view
            .set_editable(self.writable.get() && mode != Mode::Reading);
        if mode == Mode::Reading {
            self.reader.set(
                self.source_path.borrow().clone(),
                self.parsed.borrow().reading.clone(),
            );
            let source = self.text();
            let byte = source
                .char_indices()
                .nth(self.buffer.cursor_position() as usize)
                .map_or(source.len(), |(b, _)| b);
            self.reader.jump(byte);
        }
    }
    pub fn mode(&self) -> Mode {
        self.mode.get()
    }
    pub fn set_writable(&self, writable: bool) {
        self.writable.set(writable);
        self.view
            .set_editable(writable && self.mode.get() != Mode::Reading);
        self.reader.writable.set(writable);
    }
    pub fn set_location(&self, path: std::path::PathBuf) {
        *self.source_path.borrow_mut() = path;
    }
    pub fn set_asset_handler(&self, handler: AssetHandler) {
        *self.preview.on_asset.borrow_mut() = Some(handler.clone());
        *self.reader.on_asset.borrow_mut() = Some(handler);
    }
    pub fn set_link_handler(&self, handler: LinkHandler) {
        *self.preview.on_link.borrow_mut() = Some(handler.clone());
        *self.reader.on_link.borrow_mut() = Some(handler);
    }
    pub fn apply_edit(&self, edit: notes_markdown::editing::Edit) -> bool {
        if !self.writable.get() {
            return false;
        }
        let text = self.text();
        if edit.range.start > edit.range.end
            || edit.range.end > text.len()
            || !text.is_char_boundary(edit.range.start)
            || !text.is_char_boundary(edit.range.end)
        {
            return false;
        }
        let mut start = self
            .buffer
            .iter_at_offset(notes_markdown::character_offset(&text, edit.range.start));
        let mut end = self
            .buffer
            .iter_at_offset(notes_markdown::character_offset(&text, edit.range.end));
        self.buffer.begin_user_action();
        self.buffer.delete(&mut start, &mut end);
        self.buffer.insert(&mut start, &edit.replacement);
        self.buffer.place_cursor(&start);
        self.buffer.end_user_action();
        true
    }
    pub fn continue_list(&self) -> bool {
        if self.buffer.has_selection() {
            return false;
        }
        let text = self.text();
        let byte = text
            .char_indices()
            .nth(self.buffer.cursor_position() as usize)
            .map_or(text.len(), |(b, _)| b);
        notes_markdown::editing::continue_list(&text, byte)
            .is_some_and(|edit| self.apply_edit(edit))
    }
    pub fn toggle_task(&self) -> bool {
        let text = self.text();
        let byte = text
            .char_indices()
            .nth(self.buffer.cursor_position() as usize)
            .map_or(text.len(), |(b, _)| b);
        notes_markdown::editing::toggle_task(&text, byte).is_some_and(|edit| self.apply_edit(edit))
    }
    fn toggle_task_at(&self, byte: usize) {
        let text = self.text();
        if let Some(edit) = notes_markdown::editing::toggle_task(&text, byte) {
            self.apply_edit(edit);
        }
    }
    pub fn set_property(&self, key: &str, value: Option<&str>) -> Result<(), String> {
        let edit = notes_markdown::editing::set_property(&self.text(), key, value)?;
        if self.apply_edit(edit) {
            Ok(())
        } else {
            Err("This note is read-only".into())
        }
    }
    pub fn insert_text(&self, text: &str) -> bool {
        if !self.writable.get() {
            return false;
        }
        self.buffer.begin_user_action();
        self.buffer.delete_selection(true, true);
        self.buffer.insert_at_cursor(text);
        self.buffer.end_user_action();
        true
    }
    pub fn fold_heading(&self) {
        if let Some((a, b)) = self.fold_marks.borrow_mut().pop() {
            let start = self.buffer.iter_at_mark(&a);
            let end = self.buffer.iter_at_mark(&b);
            self.buffer.remove_tag(&self.fold_tag, &start, &end);
            self.buffer.delete_mark(&a);
            self.buffer.delete_mark(&b);
            return;
        }
        let parsed = self.parsed.borrow();
        let cursor = self.buffer.cursor_position();
        let text = self.text();
        if let Some((i, heading)) = parsed
            .headings
            .iter()
            .enumerate()
            .rev()
            .find(|(_, h)| notes_markdown::character_offset(&text, h.offset) <= cursor)
        {
            let start = text[heading.offset..]
                .find('\n')
                .map_or(text.len(), |n| heading.offset + n + 1);
            let end = parsed
                .headings
                .iter()
                .skip(i + 1)
                .find(|h| h.level <= heading.level)
                .map_or(text.len(), |h| h.offset);
            let a = self
                .buffer
                .iter_at_offset(notes_markdown::character_offset(&text, start));
            let b = self
                .buffer
                .iter_at_offset(notes_markdown::character_offset(&text, end));
            self.buffer.apply_tag(&self.fold_tag, &a, &b);
            self.fold_marks.borrow_mut().push((
                self.buffer.create_mark(None, &a, true),
                self.buffer.create_mark(None, &b, false),
            ));
        }
    }
    pub fn set_palette(&self, accent: &str, background: &str, dim: &str, light: bool) {
        self.frontmatter_tag.set_foreground(Some(dim));
        for (kind, tag) in &self.tags {
            match kind {
                Kind::Link => tag.set_foreground(Some(accent)),
                Kind::Code | Kind::CodeBlock => tag.set_background(Some(background)),
                _ => {}
            }
        }
        self.reader.light.set(light);
        self.preview.light.set(light);
        let manager = sourceview5::StyleSchemeManager::default();
        self.buffer.set_style_scheme(
            manager
                .scheme(if light { "Adwaita" } else { "Adwaita-dark" })
                .as_ref(),
        );
    }
    pub fn on_parsed(&self, callback: impl Fn(&Parsed) + 'static) {
        *self.on_parsed.borrow_mut() = Some(Box::new(callback));
    }
    pub fn outline(&self) -> Vec<(String, usize, u8)> {
        self.parsed
            .borrow()
            .headings
            .iter()
            .map(|h| (h.text.clone(), h.offset, h.level))
            .collect()
    }
    pub fn statistics(&self) -> (usize, usize) {
        let p = self.parsed.borrow();
        (p.words, p.characters)
    }
    pub fn jump_to(&self, byte: usize) {
        let text = self.text();
        let mut iter = self
            .buffer
            .iter_at_offset(notes_markdown::character_offset(&text, byte));
        let mut folds = self.fold_marks.borrow_mut();
        let mut index = 0;
        while index < folds.len() {
            let (a, b) = &folds[index];
            if iter.offset() >= self.buffer.iter_at_mark(a).offset()
                && iter.offset() < self.buffer.iter_at_mark(b).offset()
            {
                let (a, b) = folds.remove(index);
                self.buffer.remove_tag(
                    &self.fold_tag,
                    &self.buffer.iter_at_mark(&a),
                    &self.buffer.iter_at_mark(&b),
                );
                self.buffer.delete_mark(&a);
                self.buffer.delete_mark(&b);
            } else {
                index += 1;
            }
        }
        drop(folds);
        self.buffer.place_cursor(&iter);
        self.view.scroll_to_iter(&mut iter, 0.15, false, 0.0, 0.0);
        if self.mode.get() == Mode::Reading {
            self.reader.jump(byte);
            self.reader.view.grab_focus();
        } else {
            self.view.grab_focus();
        }
    }
    fn schedule(self: &Rc<Self>) {
        if let Some(id) = self.pending.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(140),
            move || {
                if let Some(e) = weak.upgrade() {
                    e.pending.borrow_mut().take();
                    e.parse();
                }
            },
        ));
    }
    fn parse(self: &Rc<Self>) {
        if self.parsing.replace(true) {
            return;
        }
        let revision = self.revision.get();
        let text = self.text();
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(notes_markdown::parse(&text));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(parsed) = receiver.recv().await else {
                return;
            };
            let Some(e) = weak.upgrade() else { return };
            e.parsing.set(false);
            if e.revision.get() != revision {
                e.schedule();
                return;
            }
            e.parsed_revision.set(revision);
            if e.mode.get() == Mode::Reading {
                e.reader
                    .set(e.source_path.borrow().clone(), parsed.reading.clone());
            }
            *e.parsed.borrow_mut() = parsed;
            e.apply(&e.parsed.borrow());
            if let Some(callback) = e.on_parsed.borrow().as_ref() {
                callback(&e.parsed.borrow());
            }
        });
    }
    pub fn parsed_revision(&self) -> u64 {
        self.parsed_revision.get()
    }
    pub fn property_value(&self, key: &str) -> Option<String> {
        notes_markdown::editing::property_value(&self.text(), key)
    }
    pub fn metadata(&self) -> notes_markdown::Metadata {
        self.parsed.borrow().metadata.clone()
    }
    pub fn tags(&self) -> Vec<String> {
        self.parsed.borrow().tags.clone()
    }
    pub fn links(&self) -> Vec<notes_markdown::Link> {
        self.parsed.borrow().links.clone()
    }
    pub fn link_at_cursor(&self) -> Option<notes_markdown::Link> {
        if self.parsed_revision.get() != self.revision.get() {
            return None;
        }
        let cursor = self.buffer.cursor_position();
        self.parsed
            .borrow()
            .links
            .iter()
            .find(|link| link.characters.contains(&cursor))
            .cloned()
    }
    pub fn jump_to_reference(&self, reference: &str) -> bool {
        let parsed = self.parsed.borrow();
        let offset = if let Some(block) = reference.strip_prefix('^') {
            parsed
                .blocks
                .iter()
                .find(|b| b.id == block)
                .map(|b| b.offset)
        } else {
            parsed
                .headings
                .iter()
                .find(|h| h.text.eq_ignore_ascii_case(reference))
                .map(|h| h.offset)
        };
        if let Some(offset) = offset {
            drop(parsed);
            self.jump_to(offset);
            true
        } else {
            false
        }
    }
    fn schedule_render(self: &Rc<Self>) {
        if self.render_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(e) = weak.upgrade() {
                e.render_pending.set(false);
                e.apply(&e.parsed.borrow());
            }
        });
    }
    fn clear_rendered(&self) {
        if let Some((start, end)) = self.rendered.borrow_mut().take() {
            let a = self.buffer.iter_at_mark(&start);
            let b = self.buffer.iter_at_mark(&end);
            for (_, tag) in &self.tags {
                self.buffer.remove_tag(tag, &a, &b);
            }
            self.buffer.remove_tag(&self.hidden_tag, &a, &b);
            self.buffer.remove_tag(&self.frontmatter_tag, &a, &b);
            self.buffer.delete_mark(&start);
            self.buffer.delete_mark(&end);
        }
        self.render_cache.borrow_mut().clear();
    }
    /// GTK work and allocation are bounded to the visible source window. Parser offsets
    /// are computed on the worker, so neither scrolling nor mode switching copies source.
    fn apply(&self, parsed: &Parsed) {
        if !self.live.get() || self.parsed_revision.get() != self.revision.get() {
            return;
        }
        let cursor = self.buffer.cursor_position();
        if let Some(link) = parsed
            .links
            .iter()
            .find(|link| link.embed && link.characters.contains(&cursor))
        {
            let key = format!("{}:{}", self.revision.get(), link.target);
            if self.preview_key.borrow().as_ref() != Some(&key) {
                self.preview.set(
                    self.source_path.borrow().clone(),
                    vec![notes_markdown::render::ReadBlock {
                        offset: link.range.start,
                        kind: notes_markdown::render::ReadKind::Embed {
                            target: link.target.clone(),
                            wiki: link.wiki,
                        },
                        markup: String::new(),
                        text: link.target.clone(),
                    }],
                );
                *self.preview_key.borrow_mut() = Some(key);
            }
            self.preview_reveal.set_reveal_child(true);
        } else {
            self.preview_reveal.set_reveal_child(false);
        }
        let visible = self.view.visible_rect();
        let first = self
            .view
            .iter_at_position(visible.x() + self.view.left_margin(), visible.y())
            .map(|(iter, _)| iter.offset())
            .unwrap_or(0);
        let last = self
            .view
            .iter_at_position(
                visible.x() + self.view.left_margin(),
                visible.y() + visible.height(),
            )
            .map(|(iter, _)| iter.offset())
            .unwrap_or(first + 8192);
        let start = (first - 1024).max(0);
        let end = (last + 1024)
            .max(start + 1)
            .min(start + 32768)
            .min(self.buffer.char_count());
        let spans = parsed
            .spans
            .iter()
            .filter_map(|span| {
                let a = span.characters.start.max(start);
                let b = span.characters.end.min(end);
                (a < b).then_some((span.kind, a, b))
            })
            .collect::<Vec<_>>();
        if *self.render_cache.borrow() == spans
            && self.rendered.borrow().as_ref().is_some_and(|(a, b)| {
                self.buffer.iter_at_mark(a).offset() == start
                    && self.buffer.iter_at_mark(b).offset() == end
            })
        {
            return;
        }
        self.clear_rendered();
        let a = self.buffer.iter_at_offset(start);
        let b = self.buffer.iter_at_offset(end);
        for (kind, start, end) in &spans {
            if let Some((_, tag)) = self.tags.iter().find(|(k, _)| k == kind) {
                self.buffer.apply_tag(
                    tag,
                    &self.buffer.iter_at_offset(*start),
                    &self.buffer.iter_at_offset(*end),
                );
            }
        }
        if let Some(range) = &parsed.frontmatter {
            let (from, to) = (range.start.max(start), range.end.min(end));
            if from < to {
                self.buffer.apply_tag(
                    &self.frontmatter_tag,
                    &self.buffer.iter_at_offset(from),
                    &self.buffer.iter_at_offset(to),
                );
            }
        }
        let cursor = self.buffer.iter_at_offset(self.buffer.cursor_position());
        let line = cursor.line();
        let selected = self.buffer.selection_bounds();
        for range in &parsed.hidden {
            let from = range.start.max(start);
            let to = range.end.min(end);
            if from >= to {
                continue;
            }
            let a = self.buffer.iter_at_offset(from);
            let b = self.buffer.iter_at_offset(to);
            if a.line() == line
                || selected
                    .as_ref()
                    .is_some_and(|(s, e)| from < e.offset() && to > s.offset())
            {
                continue;
            }
            self.buffer.apply_tag(&self.hidden_tag, &a, &b);
        }
        *self.rendered.borrow_mut() = Some((
            self.buffer.create_mark(None, &a, true),
            self.buffer.create_mark(None, &b, false),
        ));
        *self.render_cache.borrow_mut() = spans;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a GTK display; run under a headless compositor"]
    fn modes_preserve_source_cursor_selection_and_undo() {
        gtk::init().unwrap();
        let text = "# Café 🦀\r\n\r\n**bold** [[Note]]\n";
        let e = Editor::new(text);
        *e.parsed.borrow_mut() = notes_markdown::parse(text);
        e.parsed_revision.set(0);

        e.buffer
            .select_range(&e.buffer.iter_at_offset(3), &e.buffer.iter_at_offset(7));
        e.set_live(true);
        assert!(e.buffer.iter_at_offset(1).has_tag(&e.tags[0].1));
        e.set_live(false);
        assert_eq!(e.text(), text);
        let (a, b) = e.buffer.selection_bounds().unwrap();
        assert_eq!((a.offset(), b.offset()), (3, 7));
        e.buffer.insert(&mut e.buffer.end_iter(), "more");
        assert!(e.buffer.can_undo());
        e.set_live(true);
        e.buffer.undo();
        assert_eq!(e.text(), text);
        e.buffer.redo();
        assert!(e.text().ends_with("more"));
        let workflow = Editor::new("- [x] tâche");
        workflow.buffer.place_cursor(&workflow.buffer.end_iter());
        assert!(workflow.continue_list());
        assert_eq!(workflow.text(), "- [x] tâche\n- [ ] ");
        workflow.buffer.undo();
        assert_eq!(workflow.text(), "- [x] tâche");
        let body = workflow.text();
        workflow.set_property("status", Some("draft")).unwrap();
        assert!(workflow.text().ends_with(&body));
        workflow.buffer.undo();
        assert_eq!(workflow.text(), body);
        // Exercise bounded formatting and GTK insert cost on the same large fixture as the engine benchmark.
        let large = "# Large document\n\n**Bold** and *emphasis* with [[links]] and `code`.\n\n"
            .repeat(16000);
        let big = Editor::new(&large);
        let window = gtk::Window::builder()
            .default_width(800)
            .default_height(600)
            .child(&big.root)
            .build();
        window.present();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while big.view.width() == 0 && std::time::Instant::now() < deadline {
            glib::MainContext::default().iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(big.view.width() > 0);
        *big.parsed.borrow_mut() = notes_markdown::parse(&large);
        big.parsed_revision.set(0);
        let start = std::time::Instant::now();
        big.set_live(true);
        let render = start.elapsed();
        let (_, end) = big
            .rendered
            .borrow()
            .as_ref()
            .map(|(a, b)| {
                (
                    big.buffer.iter_at_mark(a).offset(),
                    big.buffer.iter_at_mark(b).offset(),
                )
            })
            .unwrap();
        assert!(end <= 32768);
        let before = big.buffer.iter_at_offset(2);
        assert!(before.has_tag(&big.tags[0].1));
        let mut typing = Vec::new();
        for _ in 0..30 {
            let start = std::time::Instant::now();
            big.buffer.insert(&mut big.buffer.iter_at_offset(36), "x");
            typing.push(start.elapsed().as_micros());
        }
        typing.sort_unstable();
        eprintln!(
            "GTK fixture_bytes={} preview_us={} insert_p50_us={} insert_max_us={} formatted_characters={}",
            large.len(),
            render.as_micros(),
            typing[typing.len() / 2],
            typing.last().unwrap(),
            end
        );
        // Move the insertion point into a formerly styled region: stale parses must never be rendered over new text.
        big.buffer.insert(&mut big.buffer.iter_at_offset(0), "é");
        big.set_live(false);
        big.set_live(true);
        assert!(big.rendered.borrow().is_none());
        window.destroy();
    }
}
