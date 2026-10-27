# Notes

Standalone Rust/GTK4 app. Read PLAN.md for current implementation status and STYLE.md for the pinned canonical Nyx style. Use public pinned Nyx dependencies; never import sibling application internals.

Markdown source fidelity and conflict protection are invariants. Never silently overwrite external changes or modify `.obsidian`. Keep vault I/O and indexing off GTK. Keep derived caches rebuildable.

Use essential targeted checks only: cargo fmt, cargo clippy, relevant domain tests, and tools/check-gtk.py when GTK behavior changes. Do not download AI models for routine verification. Install application updates with ./install.sh without opening or restarting Notes.

Display name: Notes. Binary/package: nyx-notes. Application ID: io.github.design_nexus.nyx.Notes.

Tabs and vault entries must provide native right-click and keyboard context menus. Dialog descriptions must not receive automatic focus/selection. Verify actual one-request window closing, not only direct destruction.
