use super::views::{self, Camera};
use super::*;
use vault::knowledge::Graph;
impl Ui {
    pub(super) fn graph(self: &Rc<Self>, local: bool) {
        let (Some(root), Some(db)) = (self.root.borrow().clone(), self.db.borrow().clone()) else {
            self.message("Open a vault first");
            return;
        };
        let center = if local {
            self.active().map(|d| d.path.clone())
        } else {
            None
        };
        let weak = Rc::downgrade(self);
        self.message("Building the graph from the local index…");
        worker::background(
            move || Graph::load(&db, &root),
            move |result| {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(graph) => ui.graph_window(graph, center),
                        Err(e) => ui.message(&format!("Graph: {e:#}")),
                    }
                }
            },
        );
    }
    fn graph_window(self: &Rc<Self>, graph: Graph, center: Option<PathBuf>) {
        let (window, root, toolbar) = self.workspace_window("Knowledge graph");
        let p = prefs::get().graph;
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Find nodes")
            .text(&p.query)
            .build();
        let tag = gtk::Entry::builder()
            .placeholder_text("Tag filter")
            .text(&p.tag)
            .width_chars(14)
            .build();
        let local = gtk::CheckButton::with_label("Local");
        local.set_active(center.is_some());
        local.set_sensitive(self.active().is_some());
        let depth = gtk::SpinButton::with_range(0.0, 10.0, 1.0);
        depth.set_value(p.depth as f64);
        let orphans = gtk::CheckButton::with_label("Orphans");
        orphans.set_active(p.orphans);
        let grouped = gtk::CheckButton::with_label("Group folders");
        grouped.set_active(p.grouped);
        let labels = gtk::CheckButton::with_label("Labels");
        labels.set_active(p.labels);
        let reset = gtk::Button::with_label("Fit");
        let filters = widgets::hbox(6);
        filters.add_css_class("notes-find");
        filter.set_hexpand(true);
        for w in [
            filter.upcast_ref::<gtk::Widget>(),
            tag.upcast_ref(),
            local.upcast_ref(),
            depth.upcast_ref(),
            orphans.upcast_ref(),
        ] {
            filters.append(w);
        }
        root.append(&filters);
        toolbar.append(&grouped);
        toolbar.append(&labels);
        toolbar.append(&reset);
        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_focusable(true);
        area.set_tooltip_text(Some("Click a node to open it. Scroll to zoom; middle-drag to pan. Use the node list for keyboard navigation."));
        let (model, list, list_scroll) = text_list(0);
        list_scroll.set_vexpand(true);
        list_scroll.set_width_request(220);
        let pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        pane.set_start_child(Some(&list_scroll));
        pane.set_end_child(Some(&area));
        pane.set_resize_start_child(false);
        pane.set_position(220);
        pane.set_vexpand(true);
        root.append(&pane);
        let status = gtk::Label::new(Some(
            "Scroll: zoom · middle-drag: pan · node list: keyboard navigation",
        ));
        status.add_css_class("status-bar");
        root.append(&status);
        let graph = Rc::new(graph);
        let camera = Rc::new(Cell::new(Camera {
            zoom: p.zoom.clamp(0.05, 8.0),
            x: p.x,
            y: p.y,
        }));
        let visible = Rc::new(RefCell::new(std::collections::HashSet::<usize>::new()));
        let indices = Rc::new(RefCell::new(Vec::<usize>::new()));
        let active = center.or_else(|| self.active().map(|d| d.path.clone()));
        let update: Rc<dyn Fn()> = Rc::new({
            let graph = graph.clone();
            let visible = visible.clone();
            let indices = indices.clone();
            let model = model.clone();
            let area = area.downgrade();
            let status = status.downgrade();
            let filter = filter.downgrade();
            let tag = tag.downgrade();
            let local = local.downgrade();
            let depth = depth.downgrade();
            let orphans = orphans.downgrade();
            move || {
                let (
                    Some(area),
                    Some(status),
                    Some(filter),
                    Some(tag),
                    Some(local),
                    Some(depth),
                    Some(orphans),
                ) = (
                    area.upgrade(),
                    status.upgrade(),
                    filter.upgrade(),
                    tag.upgrade(),
                    local.upgrade(),
                    depth.upgrade(),
                    orphans.upgrade(),
                )
                else {
                    return;
                };
                let set = graph.visible(
                    &filter.text(),
                    &tag.text(),
                    if local.is_active() {
                        active.as_deref()
                    } else {
                        None
                    },
                    depth.value_as_int() as usize,
                    orphans.is_active(),
                );
                let mut order = set.iter().copied().collect::<Vec<_>>();
                order.sort_by_key(|&i| graph.nodes[i].note.path.clone());
                replace_strings(
                    &model,
                    order.iter().map(|&i| {
                        format!("{} · {}", graph.nodes[i].note.title, graph.nodes[i].degree)
                    }),
                );
                status.set_text(&format!(
                    "{} / {} nodes · {} relationships · scroll to zoom · middle-drag to pan",
                    set.len(),
                    graph.nodes.len(),
                    graph.edges.len()
                ));
                *visible.borrow_mut() = set;
                *indices.borrow_mut() = order;
                area.queue_draw();
            }
        });
        let u = update.clone();
        filter.connect_search_changed(move |_| u());
        let u = update.clone();
        tag.connect_changed(move |_| u());
        let u = update.clone();
        local.connect_toggled(move |_| u());
        let u = update.clone();
        depth.connect_value_changed(move |_| u());
        let u = update.clone();
        orphans.connect_toggled(move |_| u());
        for check in [&grouped, &labels] {
            let weak = area.downgrade();
            check.connect_toggled(move |_| {
                if let Some(a) = weak.upgrade() {
                    a.queue_draw();
                }
            });
        }
        let coordinate = |i: usize, n: &vault::knowledge::GraphNode, grouped: bool| {
            if grouped {
                (n.x, n.y)
            } else {
                let r = 36.0 * (i as f64).sqrt();
                let t = i as f64 * 2.399963229728653;
                (r * t.cos(), r * t.sin())
            }
        };
        let graph_draw = graph.clone();
        let set = visible.clone();
        let c = camera.clone();
        let group_draw = grouped.clone();
        let label_draw = labels.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let p = nyx_theme::palette();
            views::color(cr, &p.bg, 1.0);
            let _ = cr.paint();
            let c = c.get();
            c.transform(cr, w, h);
            let set = set.borrow();
            let graph = &graph_draw;
            let grouped = group_draw.is_active();
            let on_screen = |x: f64, y: f64| {
                let x = x * c.zoom + w as f64 / 2.0 + c.x;
                let y = y * c.zoom + h as f64 / 2.0 + c.y;
                x >= -100.0 && y >= -40.0 && x <= w as f64 + 100.0 && y <= h as f64 + 40.0
            };
            views::color(cr, &p.border, 0.65);
            cr.set_line_width(0.7 / c.zoom);
            for &(a, b) in &graph.edges {
                if !set.contains(&a) || !set.contains(&b) {
                    continue;
                }
                let a = coordinate(a, &graph.nodes[a], grouped);
                let b = coordinate(b, &graph.nodes[b], grouped);
                if !on_screen(a.0, a.1) && !on_screen(b.0, b.1) {
                    continue;
                }
                cr.move_to(a.0, a.1);
                cr.line_to(b.0, b.1);
            }
            let _ = cr.stroke();
            for &i in set.iter() {
                let n = &graph.nodes[i];
                let (x, y) = coordinate(i, n, grouped);
                if !on_screen(x, y) {
                    continue;
                }
                views::color(cr, if n.degree > 0 { &p.accent } else { &p.dim_text }, 1.0);
                cr.arc(
                    x,
                    y,
                    (3.0 + (n.degree as f64).sqrt()).min(12.0) / c.zoom.sqrt(),
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.fill();
                if label_draw.is_active() && c.zoom > 0.35 {
                    views::color(cr, &p.text, 0.9);
                    views::text(
                        cr,
                        &n.note.title,
                        x + 10.0,
                        y - 8.0,
                        180.0 / c.zoom,
                        24.0 / c.zoom,
                        11.0 / c.zoom,
                    );
                }
            }
        });
        views::pan_zoom(&area, camera.clone());
        let c = camera.clone();
        let graph_click = graph.clone();
        let set = visible.clone();
        let groups = grouped.clone();
        let weak = Rc::downgrade(self);
        let a = area.downgrade();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, x, y| {
            if let (Some(ui), Some(a)) = (weak.upgrade(), a.upgrade()) {
                let c = c.get();
                let (x, y) = c.world(&a, x, y);
                if let Some(i) = set
                    .borrow()
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let p = coordinate(i, &graph_click.nodes[i], groups.is_active());
                        (p.0 - x).hypot(p.1 - y) < 15.0 / c.zoom
                    })
                    .min_by(|&i, &j| {
                        let a = coordinate(i, &graph_click.nodes[i], groups.is_active());
                        let b = coordinate(j, &graph_click.nodes[j], groups.is_active());
                        (a.0 - x)
                            .hypot(a.1 - y)
                            .total_cmp(&(b.0 - x).hypot(b.1 - y))
                    })
                {
                    ui.open_file(graph_click.nodes[i].note.path.clone());
                }
            }
        });
        area.add_controller(click);
        let weak = Rc::downgrade(self);
        let graph_list = graph.clone();
        list.connect_activate(move |_, position| {
            if let Some(ui) = weak.upgrade()
                && let Some(&i) = indices.borrow().get(position as usize)
            {
                ui.open_file(graph_list.nodes[i].note.path.clone());
            }
        });
        let c = camera.clone();
        let a = area.downgrade();
        let set = visible.clone();
        let graph_fit = graph.clone();
        let group_fit = grouped.clone();
        reset.connect_clicked(move |_| {
            if let Some(area) = a.upgrade() {
                let set = set.borrow();
                if set.is_empty() {
                    return;
                }
                let mut bounds = (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                );
                for &i in set.iter() {
                    let (x, y) = coordinate(i, &graph_fit.nodes[i], group_fit.is_active());
                    bounds.0 = bounds.0.min(x);
                    bounds.1 = bounds.1.min(y);
                    bounds.2 = bounds.2.max(x);
                    bounds.3 = bounds.3.max(y);
                }
                let zoom = ((area.width() as f64 - 100.0) / (bounds.2 - bounds.0 + 60.0))
                    .min((area.height() as f64 - 100.0) / (bounds.3 - bounds.1 + 60.0))
                    .clamp(0.05, 3.0);
                c.set(Camera {
                    zoom,
                    x: -(bounds.0 + bounds.2) / 2.0 * zoom,
                    y: -(bounds.1 + bounds.3) / 2.0 * zoom,
                });
                area.queue_draw();
            }
        });
        let camera_save = camera;
        window.connect_close_request(move |_| {
            let c = camera_save.get();
            prefs::update(|p| {
                p.graph = prefs::GraphSettings {
                    query: filter.text().to_string(),
                    tag: tag.text().to_string(),
                    depth: depth.value_as_int() as u32,
                    orphans: orphans.is_active(),
                    grouped: grouped.is_active(),
                    labels: labels.is_active(),
                    zoom: c.zoom,
                    x: c.x,
                    y: c.y,
                }
            });
            glib::Propagation::Proceed
        });
        let weak = area.downgrade();
        nyx_theme::subscribe(&area, move || {
            if let Some(a) = weak.upgrade() {
                a.queue_draw();
            }
        });
        update();
        window.present();
    }
}
