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

/// The tags that shape Live's lines.
struct Layout {
    /// Level 1, 2, and 3 and below.
    headings: [gtk::TextTag; 3],
    blank: gtk::TextTag,
    bullet: gtk::TextTag,
    check: gtk::TextTag,
    done: gtk::TextTag,
    hidden_properties: gtk::TextTag,
    /// Code blocks, a little smaller than prose, like the mockup's cards.
    code: gtk::TextTag,
    /// A task's first line, indented to make room for its checkbox.
    task: gtk::TextTag,
    /// Room under the title (or above the first line) for the property chips.
    chip_room: gtk::TextTag,
}

/// Space kept for the chips' row, in pixels.
const CHIP_ROOM: i32 = 40;
/// A task's checkbox and the gap after it, in pixels.
const CHECK: i32 = 16;
const CHECK_GAP: i32 = 10;
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
    /// Live: the front matter, small and quiet while you edit it.
    frontmatter_tag: gtk::TextTag,
    /// Live: what is sized across the whole note rather than the viewport, so line
    /// heights stay put while you scroll (see [`Editor::apply_layout`]).
    layout: Layout,
    layout_revision: Cell<u64>,
    /// Live: one checkbox drawn over each visible task's `[ ]`.
    checks: RefCell<Vec<(gtk::Button, Rc<Cell<usize>>)>>,
    /// Live: the front matter as chips under the title, until you choose to edit it.
    chips: gtk::Box,
    editing_properties: Cell<bool>,
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
                // Sizes come from the whole-note layout tags.
                Kind::Heading => tag.set_weight(600),
                Kind::Strong => tag.set_weight(700),
                Kind::Emphasis => tag.set_style(pango::Style::Italic),
                Kind::Strike => tag.set_strikethrough(true),
                Kind::Link => tag.set_underline(pango::Underline::Single),
                Kind::Code | Kind::CodeBlock => tag.set_family(Some("monospace")),
            }
            if kind == Kind::Code {
                tag.set_scale(0.9);
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
        let tag = |name: &str| {
            let tag = gtk::TextTag::builder().name(name).build();
            buffer.tag_table().add(&tag);
            tag
        };
        let layout = Layout {
            headings: [tag("live-h1"), tag("live-h2"), tag("live-h3")],
            blank: tag("live-blank"),
            bullet: tag("live-task-bullet"),
            check: tag("live-task-check"),
            done: tag("live-task-done"),
            hidden_properties: tag("live-properties-hidden"),
            code: tag("live-code"),
            task: tag("live-task"),
            chip_room: tag("live-chip-room"),
        };
        for (tag, scale, above, below) in [
            (&layout.headings[0], 1.8, 4, 8),
            (&layout.headings[1], 1.12, 18, 4),
            (&layout.headings[2], 1.0, 12, 2),
        ] {
            tag.set_scale(scale);
            tag.set_pixels_above_lines(above);
            tag.set_pixels_below_lines(below);
        }
        // An empty line between blocks is a small gap, like a paragraph margin.
        layout.blank.set_scale(0.3);
        layout.blank.set_pixels_above_lines(0);
        layout.blank.set_pixels_below_lines(0);
        layout.bullet.set_invisible(true);
        // The checkbox is drawn in the room the indent leaves.
        layout.check.set_invisible(true);
        layout.task.set_indent(CHECK + CHECK_GAP);
        layout.code.set_scale(0.86);
        layout.code.set_pixels_above_lines(0);
        layout.code.set_pixels_below_lines(1);
        layout.code.set_pixels_inside_wrap(1);
        layout.code.set_indent(14);
        layout.done.set_strikethrough(true);
        layout.hidden_properties.set_invisible(true);
        layout.chip_room.set_pixels_below_lines(CHIP_ROOM);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chips.add_css_class("property-chips");
        chips.set_tooltip_text(Some("Click to edit the properties"));
        chips.set_visible(false);
        view.add_overlay(&chips, 0, 0);
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
            layout,
            layout_revision: Cell::new(u64::MAX),
            checks: RefCell::new(Vec::new()),
            chips,
            editing_properties: Cell::new(false),
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
            // A column is the width of "0", like CSS's `ch`.
            let glyph = view.create_pango_layout(Some("0")).pixel_size().0;
            // The text starts a fixed step from the sidebar; a writing width only
            // limits how far right it runs, the rest of the room stays on the right.
            let left = if view.width() < 600 { 24 } else { 48 };
            let right = if columns > 0 {
                (view.width() - left - columns * glyph).max(left)
            } else {
                left
            };
            if view.left_margin() != left || view.right_margin() != right {
                view.set_left_margin(left);
                view.set_right_margin(right);
                editor.render_cache.borrow_mut().clear();
                editor.schedule_render();
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
        let chip_click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&editor);
        chip_click.connect_released(move |_, _, _, _| {
            if let Some(e) = weak.upgrade() {
                e.edit_properties();
            }
        });
        editor.chips.add_controller(chip_click);
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
        // Prose spacing: wrapped lines breathe, blocks sit a little apart.
        let (wrap, above, below) = if live { (7, 3, 3) } else { (0, 0, 0) };
        self.view.set_pixels_inside_wrap(wrap);
        self.view.set_pixels_above_lines(above);
        self.view.set_pixels_below_lines(below);
        self.clear_layout();
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
        self.layout.done.set_foreground(Some(dim));
        for (kind, tag) in &self.tags {
            match kind {
                Kind::Link => tag.set_foreground(Some(accent)),
                Kind::Code => tag.set_background(Some(background)),
                // A code block reads as one card, edge to edge of the column.
                Kind::CodeBlock => tag.set_paragraph_background(Some(background)),
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
    /// Show the front matter's source to edit it; it folds back into chips once the
    /// cursor leaves it.
    pub fn edit_properties(&self) {
        let Some(range) = self.parsed.borrow().frontmatter.clone() else {
            return;
        };
        self.editing_properties.set(true);
        self.layout_revision.set(u64::MAX);
        // After the opening `---`.
        let mut iter = self.buffer.iter_at_offset(range.start);
        iter.forward_line();
        self.buffer.place_cursor(&iter);
        self.view.grab_focus();
        self.render_cache.borrow_mut().clear();
        self.apply(&self.parsed.borrow());
    }

    fn clear_layout(&self) {
        let (a, b) = self.buffer.bounds();
        for tag in self.layout.headings.iter().chain([
            &self.layout.blank,
            &self.layout.bullet,
            &self.layout.check,
            &self.layout.done,
            &self.layout.hidden_properties,
            &self.layout.chip_room,
            &self.layout.code,
            &self.layout.task,
        ]) {
            self.buffer.remove_tag(tag, &a, &b);
        }
        self.layout_revision.set(u64::MAX);
        self.chips.set_visible(false);
        for (check, _) in self.checks.borrow().iter() {
            check.set_visible(false);
        }
    }

    /// Size headings and empty lines, hide task bullets, and fold the front matter
    /// into chips, across the whole note once per parse: changing line heights only
    /// near the viewport would make the text jump as you scroll.
    fn apply_layout(&self, parsed: &Parsed) {
        // Leaving the front matter folds it back.
        if self.editing_properties.get()
            && let Some(range) = &parsed.frontmatter
            && !(range.start..=range.end).contains(&self.buffer.cursor_position())
        {
            self.editing_properties.set(false);
            self.layout_revision.set(u64::MAX);
        }
        if self.layout_revision.get() == self.parsed_revision.get() {
            return;
        }
        self.clear_layout();
        self.layout_revision.set(self.parsed_revision.get());
        let iter = |at: i32| self.buffer.iter_at_offset(at);
        let line_end = |at: i32| {
            let mut i = iter(at);
            if !i.ends_line() {
                i.forward_to_line_end();
            }
            i
        };
        for (range, level) in &parsed.heading_lines {
            let tag = &self.layout.headings[(*level as usize).clamp(1, 3) - 1];
            self.buffer.apply_tag(tag, &iter(range.start), &line_end(range.start));
        }
        for at in &parsed.blank_lines {
            self.buffer.apply_tag(&self.layout.blank, &iter(*at), &iter(*at + 1));
        }
        for span in parsed.spans.iter().filter(|s| s.kind == Kind::CodeBlock) {
            self.buffer.apply_tag(
                &self.layout.code,
                &iter(span.characters.start),
                &iter(span.characters.end),
            );
        }
        for task in &parsed.task_marks {
            self.buffer
                .apply_tag(&self.layout.task, &iter(task.bullet.start), &line_end(task.bullet.start));
            self.buffer
                .apply_tag(&self.layout.bullet, &iter(task.bullet.start), &iter(task.bullet.end));
            self.buffer
                .apply_tag(&self.layout.check, &iter(task.check.start), &iter(task.check.end));
            if task.checked {
                self.buffer
                    .apply_tag(&self.layout.done, &iter(task.text.start), &iter(task.text.end));
            }
        }
        self.fill_chips(parsed);
        if let Some(range) = &parsed.frontmatter
            && !self.editing_properties.get()
            && self.chips.first_child().is_some()
        {
            self.buffer
                .apply_tag(&self.layout.hidden_properties, &iter(range.start), &iter(range.end));
            // Typing must never land in hidden text.
            if self.buffer.cursor_position() < range.end && !self.buffer.has_selection() {
                self.buffer.place_cursor(&iter(range.end));
            }
            // The chips go under the title when the note starts with one, else above
            // the first line.
            let blanks = parsed.blank_lines.iter().filter(|b| **b >= range.end).count();
            let mut first = iter(range.end);
            for _ in 0..blanks {
                if first.ends_line() && first.starts_line() {
                    first.forward_line();
                } else {
                    break;
                }
            }
            let title = parsed
                .heading_lines
                .iter()
                .find(|(r, l)| *l == 1 && r.start == first.offset())
                .is_some();
            self.layout.chip_room.set_pixels_below_lines(if title { CHIP_ROOM } else { 0 });
            self.layout.chip_room.set_pixels_above_lines(if title { 0 } else { CHIP_ROOM });
            self.buffer
                .apply_tag(&self.layout.chip_room, &first, &line_end(first.offset()));
            self.chips.set_visible(true);
        }
    }

    fn fill_chips(&self, parsed: &Parsed) {
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let chip = |key: Option<&str>, value: &str, accent: bool| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.add_css_class("property-chip");
            if accent {
                row.add_css_class("tag");
            }
            if let Some(key) = key {
                let k = gtk::Label::new(Some(key));
                k.add_css_class("property-key");
                row.append(&k);
            }
            let v = gtk::Label::new(Some(value));
            v.add_css_class("property-value");
            row.append(&v);
            self.chips.append(&row);
        };
        let meta = &parsed.metadata;
        let mut shown = 0;
        for (key, value) in &meta.properties {
            if shown == 5 || matches!(key.as_str(), "tags" | "tag" | "aliases" | "alias") {
                continue;
            }
            let mut name = key.replace(['_', '-'], " ");
            if let Some(c) = name.get(..1) {
                name = c.to_uppercase() + &name[1..];
            }
            chip(Some(&name), value.trim_matches(['"', '\'']), false);
            shown += 1;
        }
        for tag in meta.tags.iter().take(6) {
            chip(None, &format!("#{tag}"), true);
        }
        if self.chips.first_child().is_some()
            && let Ok(modified) = std::fs::metadata(&*self.source_path.borrow())
                .and_then(|m| m.modified())
            && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
            && let Ok(when) = glib::DateTime::from_unix_local(since.as_secs() as i64)
            && let Ok(now) = glib::DateTime::now_local()
        {
            let format = if when.ymd() == now.ymd() {
                "Today %H:%M"
            } else if when.year() == now.year() {
                "%b %-d"
            } else {
                "%b %-d, %Y"
            };
            if let Ok(text) = when.format(format) {
                chip(Some("Updated"), &text, false);
            }
        }
    }

    /// Put the chips and the checkboxes where their text is laid out. Only lines in
    /// the viewport have settled positions, so only their tasks get a checkbox.
    fn place_overlays(&self, parsed: &Parsed, first: i32, last: i32) {
        if self.chips.is_visible()
            && let Some((_, range)) = self.layout.chip_room_range(&self.buffer)
        {
            let rect = self.view.iter_location(&range);
            let (y, height) = self.view.line_yrange(&range);
            let top = if self.layout.chip_room.pixels_below_lines() > 0 {
                y + height - CHIP_ROOM + 6
            } else {
                y + 6
            };
            self.view.move_overlay(&self.chips, rect.x(), top);
        }
        let mut checks = self.checks.borrow_mut();
        let mut used = 0;
        for task in &parsed.task_marks {
            if task.check.start < first || task.check.start > last {
                continue;
            }
            if used == checks.len() {
                let button = gtk::Button::new();
                button.add_css_class("task-check");
                button.set_focusable(false);
                button.set_can_focus(false);
                button.set_child(Some(&gtk::Image::from_icon_name("object-select-symbolic")));
                let marker = Rc::new(Cell::new(0));
                let buffer = self.buffer.clone();
                let at = marker.clone();
                let writable = self.view.clone();
                button.connect_clicked(move |_| {
                    if !writable.is_editable() {
                        return;
                    }
                    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
                    if let Some(edit) = notes_markdown::editing::toggle_task(&text, at.get()) {
                        let mut a = buffer
                            .iter_at_offset(notes_markdown::character_offset(&text, edit.range.start));
                        let mut b = buffer
                            .iter_at_offset(notes_markdown::character_offset(&text, edit.range.end));
                        buffer.begin_user_action();
                        buffer.delete(&mut a, &mut b);
                        buffer.insert(&mut a, &edit.replacement);
                        buffer.end_user_action();
                    }
                });
                self.view.add_overlay(&button, 0, 0);
                checks.push((button, marker));
            }
            let (button, marker) = &checks[used];
            used += 1;
            marker.set(task.marker);
            if task.checked {
                button.add_css_class("checked");
            } else {
                button.remove_css_class("checked");
            }
            // In the indent's room at the line's start, past any leading spaces of a
            // nested task. (A position after hidden text still counts its width.)
            let mut start = self.buffer.iter_at_offset(task.bullet.start);
            start.set_line_offset(0);
            let rect = self.view.iter_location(&start);
            let spaces = self
                .buffer
                .text(&start, &self.buffer.iter_at_offset(task.bullet.start), false);
            let lead = if spaces.is_empty() {
                0
            } else {
                self.view.create_pango_layout(Some(&spaces)).pixel_size().0
            };
            self.view.move_overlay(
                button,
                // The line's start already counts the indent.
                rect.x() + lead - CHECK - CHECK_GAP,
                rect.y() + (rect.height() - CHECK) / 2,
            );
            button.set_visible(true);
        }
        for (button, _) in checks.iter().skip(used) {
            button.set_visible(false);
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
        self.apply_layout(parsed);
        self.place_overlays(parsed, first, last);
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
        if let Some(range) = &parsed.frontmatter
            && self.editing_properties.get()
        {
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

impl Layout {
    /// The line carrying the chips' room, as (start, start).
    fn chip_room_range(&self, buffer: &sourceview5::Buffer) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let mut iter = buffer.start_iter();
        if !iter.starts_tag(Some(&self.chip_room)) && !iter.forward_to_tag_toggle(Some(&self.chip_room)) {
            return None;
        }
        Some((iter, iter))
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
