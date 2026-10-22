//! Shared human review for optional model and third-party processor edits.
use super::*;
impl Ui {
    pub(super) fn proposal(
        self: &Rc<Self>,
        doc: Document,
        revision: u64,
        range: std::ops::Range<usize>,
        previous: String,
        proposed: String,
    ) {
        self.proposal_guarded(doc, revision, range, previous, proposed, Rc::new(|| true));
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn proposal_guarded(
        self: &Rc<Self>,
        doc: Document,
        revision: u64,
        range: std::ops::Range<usize>,
        previous: String,
        proposed: String,
        guard: Rc<dyn Fn() -> bool>,
    ) {
        let dialog = Prompt::new(
            &self.window,
            "Review proposed edit",
            "Review proposed edit",
            "Review the Markdown and diff before applying. Nothing is written until you apply; undo, history, and normal conflict protection remain active.",
            900,
            650,
        );
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("markdown")
                .as_ref(),
        );
        buffer.set_text(&proposed);
        let view = sourceview5::View::with_buffer(&buffer);
        view.add_css_class("notes-editor");
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        notebook.append_page(
            &scroll(&view),
            Some(&gtk::Label::new(Some("Proposed text"))),
        );
        let diff = sourceview5::Buffer::new(None);
        diff.set_language(
            sourceview5::LanguageManager::default()
                .language("diff")
                .as_ref(),
        );
        let diff_view = sourceview5::View::with_buffer(&diff);
        diff_view.set_editable(false);
        diff_view.add_css_class("notes-editor");
        notebook.append_page(&scroll(&diff_view), Some(&gtk::Label::new(Some("Diff"))));
        dialog.content_area().append(&notebook);
        dialog.add_button("Discard", gtk::ResponseType::Cancel);
        dialog.add_button("Copy", gtk::ResponseType::Other(1));
        dialog.add_button("Apply to note", gtk::ResponseType::Accept);
        let seq = Rc::new(Cell::new(0u64));
        let update: Rc<dyn Fn()> = Rc::new({
            let weak = buffer.downgrade();
            let diff = diff.downgrade();
            let seq = seq.clone();
            move || {
                let Some(buffer) = weak.upgrade() else {
                    return;
                };
                let source = previous.clone();
                let text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), true)
                    .to_string();
                let revision = seq.get() + 1;
                seq.set(revision);
                let seq = seq.clone();
                let diff = diff.clone();
                worker::background(
                    move || vault::history::compare(&source, &text).0,
                    move |text| {
                        if seq.get() == revision
                            && let Some(diff) = diff.upgrade()
                        {
                            diff.set_text(&text);
                        }
                    },
                );
            }
        });
        let u = update.clone();
        buffer.connect_changed(move |_| u());
        update();
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |d,r|{if let Some(ui)=weak.upgrade(){let text=buffer.text(&buffer.start_iter(),&buffer.end_iter(),true);match r{gtk::ResponseType::Accept=>{if !guard(){ui.message("The extension permission was revoked; this proposal cannot be applied");return;}
if doc.closed.get()||doc.read_only||doc.conflict.get()||doc.recovery_pending.get()||doc.editor.revision.get()!=revision{ui.message("The note changed while the proposal was generated or reviewed. Discard it and generate a fresh comparison.");return;}let changed=doc.editor.replace_range(range.clone(),&text);if changed{ui.message("Proposed edit applied as one undoable source edit");}},gtk::ResponseType::Other(1)=>{if let Some(display)=gdk::Display::default(){display.clipboard().set_text(&text);}return;},_=>{}}}d.close();});
        dialog.present();
    }
}
