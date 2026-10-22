# Synthetic baseline — 2026-10-21

Measured locally with Rust 1.99.0 and `cargo run --release --locked -p notes-vault --example benchmark`. These are single-run wall-clock measurements on this machine, not portable guarantees. “Initial index” means a new database; generated source files may already be in the operating-system page cache.

Each vault contains the stated number of small Markdown notes with frontmatter, a heading, text, and three internal links, plus 1,000 dummy attachments. The current index includes parsed YAML metadata, aliases, tags, properties, heading offsets, and outgoing relationships. Graph model construction includes link resolution, properties/tags, and stable folder-cluster coordinates. Local filtering measures depth two with petgraph traversal; rendering/frame timing is excluded. Fixture creation is excluded from the timings. Search returns the first 100 FTS-ranked matches.

| Notes | Initial index | Warm reconciliation | Search | Graph model | Local graph |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 60 ms | 3 ms | <1 ms | 3 ms | <1 ms |
| 10,000 | 392 ms | 25 ms | 3 ms | 38 ms | 3 ms |
| 50,000 | 2,003 ms | 128 ms | 16 ms | 215 ms | 27 ms |

A 1,120,000-byte Markdown fixture with 16,000 headings parsed in 45 ms. Peak benchmark-process memory reported by `/proc/self/status` was 102,344 KiB; this includes parser/index/graph work and excludes the GTK application.

This Phase 3 run includes native reader block derivation, semantic metadata, and relationships. An initial measurement exposed quadratic heading-table deletion (25,898 ms at 50,000 notes); a path/offset index removed that regression. FTS deletions also use direct row-ID lookups.

## GTK buffer and preview microbenchmark

Run `NOTES_TEST_RELEASE=1 python3 tools/check-gtk.py`. The release test uses an isolated Broadway display with a realized 800 × 600 GTK window and the same 1,120,000-byte / 16,000-heading source fixture.

| Operation | Single-run result |
| --- | ---: |
| Applying visible preview tags | 477 µs |
| Buffer insertion, median of 30 calls | 2 µs |
| Buffer insertion, maximum of 30 calls | 25 µs |
| Source characters formatted in this viewport | 1,514 |

Tagging has a hard 32,768-character source-window limit and does not allocate a byte-to-character table on GTK. The test also verifies that outdated parser offsets are not rendered after edits. Inspector/outline lists are virtualized.

These insert measurements exercise GTK buffer calls near visible text. They exclude hardware keyboard/IME dispatch, full app-shell callbacks, autosave, and presentation/frame-paint latency. They do not establish an end-to-end typing or scrolling guarantee. Full launch/memory, graph rendering, attachment throughput, and synchronization remain unmeasured. Parsing still processes full documents after a debounce on a worker; incremental parsing is pending.
