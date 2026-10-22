//! Long-form projects compile ordered Markdown chapters without modifying their sources.
use super::*;
use vault::knowledge::{self, Project};
impl Ui {
    pub(super) fn projects(self: &Rc<Self>) {
        let Some(vault_root) = self.root.borrow().clone() else {
            self.message("Open a vault first");
            return;
        };
        let (window, root, toolbar) = self.workspace_window("Writing projects");
        let project = Rc::new(RefCell::new(Project {
            root: vault_root,
            ..Default::default()
        }));
        let saved = prefs::get().projects;
        let names = std::iter::once("New project".to_owned())
            .chain(saved.iter().map(|p| p.name.clone()))
            .collect::<Vec<_>>();
        let chooser =
            gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
        toolbar.append(&chooser);
        let name = gtk::Entry::builder()
            .placeholder_text("Project name")
            .text("Writing project")
            .hexpand(true)
            .build();
        let goal = gtk::SpinButton::with_range(0.0, 10000000.0, 1000.0);
        goal.set_value(50000.0);
        goal.set_tooltip_text(Some("Manuscript word goal"));
        let settings = widgets::hbox(8);
        settings.add_css_class("notes-find");
        settings.append(&name);
        settings.append(&widgets::label("Word goal", ""));
        settings.append(&goal);
        root.append(&settings);
        let actions = widgets::hbox(6);
        actions.add_css_class("notes-find");
        let add = gtk::Button::with_label("Add active note");
        let files = gtk::Button::with_label("Add chapters…");
        let up = gtk::Button::with_label("Up");
        let down = gtk::Button::with_label("Down");
        let remove = gtk::Button::with_label("Remove");
        let open = gtk::Button::with_label("Open chapter");
        for b in [&add, &files, &open, &up, &down, &remove] {
            actions.append(b);
        }
        root.append(&actions);
        let save = gtk::Button::with_label("Save project");
        let export = gtk::Button::with_label("Compile / export…");
        let stats = gtk::Button::with_label("Refresh progress");
        toolbar.append(&save);
        toolbar.append(&stats);
        toolbar.append(&export);
        let (model, list, scroller) = text_list(0);
        scroller.set_vexpand(true);
        root.append(&scroller);
        let status = gtk::Label::new(Some(
            "Chapter sources stay in their original Markdown files.",
        ));
        status.add_css_class("status-bar");
        root.append(&status);
        let refresh: Rc<dyn Fn()> = Rc::new({
            let project = project.clone();
            let model = model.clone();
            let status = status.downgrade();
            move || {
                let p = project.borrow().clone();
                replace_strings(
                    &model,
                    p.chapters.iter().enumerate().map(|(i, path)| {
                        format!(
                            "{}. {}",
                            i + 1,
                            path.strip_prefix(&p.root).unwrap_or(path).display()
                        )
                    }),
                );
                let goal = p.goal;
                let status = status.clone();
                worker::background(
                    move || knowledge::manuscript(&p).map(|(_, words)| words),
                    move |result| {
                        if let Some(status) = status.upgrade() {
                            match result {
                                Ok(words) => status.set_text(&format!(
                                    "{words} words · goal {goal} · {:.0}%",
                                    if goal > 0 {
                                        words as f64 / goal as f64 * 100.0
                                    } else {
                                        0.0
                                    }
                                )),
                                Err(e) => status.set_text(&format!("Project: {e:#}")),
                            }
                        }
                    },
                );
            }
        });
        let p = project.clone();
        let f = refresh.clone();
        let n = name.clone();
        let g = goal.clone();
        chooser.connect_selected_notify(move |c| {
            if let Some(saved) = saved.get(c.selected().wrapping_sub(1) as usize) {
                *p.borrow_mut() = saved.clone();
                n.set_text(&saved.name);
                g.set_value(saved.goal as f64);
                f();
            }
        });
        let p = project.clone();
        name.connect_changed(move |n| p.borrow_mut().name = n.text().to_string());
        let p = project.clone();
        goal.connect_value_changed(move |g| p.borrow_mut().goal = g.value_as_int().max(0) as usize);
        let weak = Rc::downgrade(self);
        let p = project.clone();
        let f = refresh.clone();
        add.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade()
                && let Some(doc) = ui.active()
            {
                let mut p = p.borrow_mut();
                if !doc.path.starts_with(&p.root) {
                    ui.message("Choose a chapter in this project's vault");
                    return;
                }
                if !p.chapters.contains(&doc.path) {
                    p.chapters.push(doc.path.clone());
                }
                drop(p);
                f();
            }
        });
        let p = project.clone();
        let f = refresh.clone();
        let parent = window.downgrade();
        let weak = Rc::downgrade(self);
        files.connect_clicked(move |_| {
            let Some(parent) = parent.upgrade() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title("Add Markdown chapters")
                .build();
            let p = p.clone();
            let f = f.clone();
            let weak = weak.clone();
            dialog.open_multiple(Some(&parent), gio::Cancellable::NONE, move |result| {
                if let Ok(files) = result {
                    let mut project = p.borrow_mut();
                    for i in 0..files.n_items() {
                        if let Some(path) = files
                            .item(i)
                            .and_downcast::<gio::File>()
                            .and_then(|f| f.path())
                        {
                            if path.starts_with(&project.root) && vault::is_markdown(&path) {
                                if !project.chapters.contains(&path) {
                                    project.chapters.push(path);
                                }
                            } else if let Some(ui) = weak.upgrade() {
                                ui.message(
                                    "Chapters must be Markdown files in the project's vault",
                                );
                            }
                        }
                    }
                    drop(project);
                    f();
                }
            });
        });
        let selection = list.model().and_downcast::<gtk::SingleSelection>().unwrap();
        for (button, direction) in [(&up, -1i32), (&down, 1), (&remove, 0)] {
            let p = project.clone();
            let f = refresh.clone();
            let selection = selection.clone();
            button.connect_clicked(move |_| {
                let i = selection.selected() as usize;
                let mut p = p.borrow_mut();
                if i >= p.chapters.len() {
                    return;
                }
                if direction == 0 {
                    p.chapters.remove(i);
                } else {
                    let next = i as i64 + direction as i64;
                    if next < 0 || next >= p.chapters.len() as i64 {
                        return;
                    }
                    p.chapters.swap(i, next as usize);
                    drop(p);
                    f();
                    selection.set_selected(next as u32);
                    return;
                }
                drop(p);
                f();
            });
        }
        let weak = Rc::downgrade(self);
        let p = project.clone();
        let select = selection.clone();
        open.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade()
                && let Some(path) = p.borrow().chapters.get(select.selected() as usize)
            {
                ui.open_file(path.clone());
            }
        });
        let weak = Rc::downgrade(self);
        let p = project.clone();
        list.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade()
                && let Some(path) = p.borrow().chapters.get(position as usize)
            {
                ui.open_file(path.clone());
            }
        });
        let p = project.clone();
        let weak = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            let p = p.borrow().clone();
            if p.name.trim().is_empty() {
                return;
            }
            prefs::update(|prefs| {
                prefs
                    .projects
                    .retain(|v| v.name != p.name || v.root != p.root);
                prefs.projects.push(p);
            });
            if let Some(ui) = weak.upgrade() {
                ui.message("Writing project saved");
            }
        });
        let f = refresh.clone();
        stats.connect_clicked(move |_| f());
        let p = project.clone();
        let parent = window.downgrade();
        let weak = Rc::downgrade(self);
        export.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if !ui.clean_for_operation() {
                return;
            }
            let Some(parent) = parent.upgrade() else {
                return;
            };
            let p = p.borrow().clone();
            let title = format!("{}.md", p.name.replace(['/', '\\'], "-"));
            let dialog = gtk::FileDialog::builder()
                .title("Export Markdown manuscript")
                .initial_name(&title)
                .build();
            let weak = Rc::downgrade(&ui);
            dialog.save(Some(&parent), gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    worker::run(
                        move || {
                            let (text, _) = knowledge::manuscript(&p)?;
                            vault::save_copy(&path, &text, false)
                        },
                        move |result| {
                            if let Some(ui) = weak.upgrade() {
                                ui.message(&match result {
                                    Ok(()) => {
                                        "Manuscript exported; chapter sources were preserved".into()
                                    }
                                    Err(e) => format!("Export: {e:#}"),
                                });
                            }
                        },
                    );
                }
            });
        });
        refresh();
        window.present();
    }
}
