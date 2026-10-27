# Notes

A native Rust/GTK4 Markdown workspace for Linux and the Nyx suite. Notes opens ordinary Markdown directories and existing Obsidian vaults directly. MIT licensed. Phases 1–5 writing, knowledge, optional local integrations, personal synchronization, and isolated extension commands are implemented. See the [implementation status and limits](PLAN.md).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-notes/main/install.sh | bash
```

## Available now

- Source, live-preview, and native reading modes sharing one authoritative Markdown buffer. Live preview hides delimiters away from the cursor; reading mode renders headings, emphasis, links, highlighted code, tables, callouts, tasks, images, and note transclusions.
- Multiple vaults, lazy file explorer, tabs, two editor panes, outline/heading navigation, find/replace, list continuation, checkbox editing, heading folding, and writing statistics.
- Wikilinks and aliases, heading/block references, outgoing links, backlinks, unlinked mentions, nested tags, and editable YAML properties.
- Background SQLite FTS5 search with phrases, exclusions, `tag:`, `path:`, `property:name=value`, `[name:value]`, and `heading:` filters. Heading matches navigate to their source positions; searches can be saved.
- New-note and insertion templates with `{{title}}`, `{{date}}`, `{{time}}`, and `{{selection}}`; daily notes; configurable template, daily-note, and attachment folders through File → Vault preferences.
- File attachments through the picker or drag/drop, clipboard image insertion, native image previews, and explicit attachment opening. Existing attachment names are never overwritten.
- Reviewed rename/move operations update affected source link ranges and retain recovery journals. Duplicate files/folders or move them to the desktop trash through explorer actions.
- Explicit save, autosave, recovery drafts, external modification checks, conflict comparison, and atomic save-copy without overwriting existing files.
- Shared Nyx controls and Inspector, configurable typography/writing width, light/dark palettes, and automatic Omarchy theme following.

Phase 3 is available through the File menu:

- Document history with automatic deduplicated snapshots, revision diffs, and full/selective restoration. Set retention under Writing workspace.
- F11 focus mode, typewriter scrolling, and named workspace presets retaining document/pane state.
- Global/local knowledge graphs with native zoom/pan, filtering, folder groups, and keyboard navigation through the node list.
- Table, list, kanban, and gallery collections derived from frontmatter. Click table properties or use card actions to edit the Markdown; drag kanban cards between columns.
- Native JSON Canvas cards, note/image previews, groups/connections, drag/drop, resize/edit, undo/redo, autosave, and recovery. Scroll to zoom, middle-drag to pan, Tab to select cards, Enter to edit, Ctrl+Enter to open, and Delete to remove.
- Writing projects with ordered chapters, word goals, and Markdown manuscript export.
- Advanced search: `tag:research OR property:status=draft`, `property:rating>=4`, `after:2026-10-01`, `before:2026-11-01`, and independent negative terms. Date boundaries use UTC file modification time; `=` matches a property exactly, `:` searches within its display value.

Optional Phase 4 tools are also in File: local AI writing previews, vault questions, semantic/hybrid search, related notes/backlink suggestions, Capture insertion, web clipping, and suite integration preferences. Everything is off by default. Local models remain Assistant-owned; Notes never downloads models or automatically uploads a vault. Enable the bundled Notes plugin separately in Glimpse for `note QUERY`, `note new TITLE`, `note append QUERY`, and `note heading QUERY`.

All AI/suite edits require visible review. See [integration contracts and setup](docs/INTEGRATIONS.md), including compatibility notes for already running Assistant/Glimpse processes. Notes remains usable when companions or models are absent; `cargo build --release --locked -p nyx-notes --no-default-features` omits AI, sync, and third-party execution entirely.

Phase 5 adds optional personal Nextcloud/WebDAV sync and isolated WASM processors. File → Personal sync stores app passwords in the desktop keyring, supports excluded folders, and runs in the background. Divergent edits and deletions require explicit conflict review; recovery versions include attachments. See [sync setup and limits](docs/SYNC.md). File → Extensions imports disabled packages, reviews explicit permissions, and exposes commands whose edits require native diff approval. See [extension API v1](docs/EXTENSIONS.md). Both capabilities are optional build features and remain inactive until configured.

Notes does not serialize Markdown from an imported document model. Unrecognized syntax, frontmatter, BOM, line endings, and trailing newlines remain in the source. `.obsidian` is untouched. Invalid UTF-8 files are rejected rather than converted. Symlinked notes are read-only and symlinked directories are not traversed. Local link resolution and attachment access stay within the vault.

Reading mode is a derived native view, not an HTML browser. Inline note/image embeds also have a live-preview panel at the cursor. Recursive embeds are bounded. Math has basic text/Unicode presentation rather than complete TeX layout; arbitrary HTML and community plugin rendering are not supported. Full-document parsing runs on a worker with bounded viewport styling; affected-block parsing remains an optimization. See [PLAN.md](PLAN.md) for architectural limits.

## Build and install

Requires Rust, GTK4 >= 4.18, and GtkSourceView 5. On Arch Linux the native packages are `gtk4` and `gtksourceview5`.

```sh
cargo build --release --locked -p nyx-notes
./install.sh
nyx-notes --vault /path/to/vault
nyx-notes /path/to/note.md
```

On first launch, Connect a vault opens a setup popup. Browse or enter your existing vault folder, choose a Markdown file, or enter a new folder path and select Create folder. The welcome screen and File → Open vault reopen setup. The sidebar toggle and Ctrl+B can expand the explorer even when tiled to half-screen. Resolve is reserved for external file conflicts.

The installer writes the binary to `~/.local/bin`, and the launcher and icon under `$XDG_DATA_HOME` (default `~/.local/share`). It does not start Notes, change default file associations, or modify desktop configuration. A checkout-based Arch package recipe is in `packaging/PKGBUILD`.

Shared Nyx components are pinned Git dependencies. `STYLE.md` is a reference snapshot from nyx-design revision `609c1d2a5af41a8ee3b76ba23611eb30d95a4743`; update it alongside that dependency. The launcher icon reuses Nyx's canonical icon template.

## Keyboard

| Shortcut | Action |
| --- | --- |
| Ctrl+O | Open vault |
| Ctrl+N | New note |
| Ctrl+S | Save |
| Ctrl+Shift+S | Save a copy |
| Ctrl+Enter | Follow link at cursor |
| Ctrl+Shift+D | Open today's daily note |
| Ctrl+T | Toggle current task |
| Ctrl+Alt+F | Fold/unfold current heading |
| F4 | Cycle source/live/reading mode |
| F2 | Rename or move the active note |
| F11 | Toggle focus mode |
| Ctrl+F | Find in note |
| Ctrl+Shift+F | Search vault |
| Ctrl+B | Collapse sidebar |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+W | Close note |
| Ctrl+Q | Quit |
| Ctrl+, | Preferences |
| F1 | Shortcuts |
| Esc | Close search or panels |

Use the toolbar to enable split editing, then open or drag tabs into either pane. File actions include templates, attachments, properties, saved searches, and interrupted-operation recovery. Save modified notes before a bulk file move; the confirmation lists affected files before applying link changes.

## Local state and checks

Application preferences use `$XDG_CONFIG_HOME/nyx-notes`. Document history, recovery drafts, and file-operation journals use `$XDG_STATE_HOME/nyx-notes`, outside the vault. Search databases use `$XDG_CACHE_HOME/nyx-notes` and can be rebuilt. No account, cloud service, AI model, or Git installation is needed to use Notes.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked -p notes-markdown -p notes-vault
NOTES_TEST_RELEASE=1 python3 tools/check-gtk.py
cargo run --release --locked -p notes-vault --example benchmark
```

GTK checks use an isolated, invisible Broadway display and temporary XDG directories. Benchmarks generate temporary vaults and never access user notes. Dependency licenses are recorded by their upstream packages; the principal stack uses MIT, Apache-2.0, and gtk-rs MIT-compatible licenses.
