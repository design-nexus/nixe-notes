# Extension API v1

Notes supports small third-party WASM commands/note processors. This is an initial versioned API, not an Obsidian JavaScript compatibility layer or a marketplace. Native first-party suite integrations use their separate public interfaces.

File → Extensions → Install manifest imports a JSON manifest and its adjacent `.wasm` file into private application data. Installation copies immutable, content-addressed packages and leaves them disabled. Review the package name, version, digest, and requested capabilities, then explicitly check each permission before enabling it. A new digest/version needs fresh approval. Disable revokes commands and pending edit proposals. Older package copies remain available on disk and never auto-enable.

Supported permissions:

| Permission | Capability |
| --- | --- |
| `selection.read` | Receive only the passage explicitly confirmed before the command; if nothing is selected, the confirmation says that the entire active note will be sent. No path, vault listing, or other note is included. |
| `edit.propose` | Return replacement text for native editable diff review. Apply checks document revision/state and current extension permission; changes form one undoable source edit and use ordinary autosave/history/conflict protection. |

Without `selection.read`, the input passage is empty. Unknown permissions, imports, and API versions are refused. Plugins have **no imports**, including WASI, filesystem, network, processes, clocks, suite tools, or host callbacks. The maintained MIT/Apache-2.0 `wasmi` 1.1 interpreter runs on a separate worker with ten million fuel units, one instance/memory/table, 32 MiB linear memory, 4,096 table elements, 8 MiB module size, and bounded input/output. The interpreter also meters start/allocation functions. Compilation is bounded by module size but has no separate wall-clock preemption. A trap, invalid output, changed package, stale note, or revoked permission cannot apply an edit. Interpreter limits do not provide protection from a vulnerability in the interpreter itself.

## Manifest

```json
{
  "api_version": 1,
  "id": "example-writing",
  "name": "Writing example",
  "version": "1.0.0",
  "module": "processor.wasm",
  "permissions": ["selection.read", "edit.propose"],
  "commands": [{"id": "uppercase", "title": "Uppercase passage"}]
}
```

IDs use lowercase ASCII letters, numbers, and hyphens, at most 64 bytes. Names/titles are at most 128 bytes; 1–32 uniquely identified commands are allowed. Module paths must be one visible filename ending in `.wasm`; symlink files are not installed. Unknown manifest fields are rejected for API v1. Hashes identify reviewed content; they are not publisher signatures.

## Memory ABI

Export `memory`, `notes_alloc(i32 length) -> i32 pointer`, and `notes_run(i32 pointer, i32 length) -> i64 result`. No imports are allowed. The host allocates and writes UTF-8 JSON:

```json
{"api_version":1,"command":"uppercase","selection":"untrusted note text"}
```

`notes_run` returns `(output_pointer << 32) | output_byte_length`. Output is UTF-8 JSON, at most 128 KiB:

```json
{"api_version":1,"text":"PROPOSED REPLACEMENT","message":"Optional plain-text status"}
```

Both `text` and `message` are optional. A message is at most 16 KiB. Unknown output fields are rejected. Text replaces the confirmed source range **only after native review**; an empty string proposes deleting that passage. Do not treat note content as executable instructions. Each invocation has fresh memory and no persistent extension storage. Return buffers need not be freed because the entire instance is dropped afterward.

[The Rust example](../examples/extension/src/lib.rs) provides source-word counting and a reviewed uppercase processor. Build with Rust's `wasm32-unknown-unknown` target (not WASI):

```sh
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/extension/Cargo.toml
cp examples/extension/target/wasm32-unknown-unknown/release/notes_example_processor.wasm examples/extension/processor.wasm
```

Install its adjacent `manifest.json` through Extensions and review the permissions. The example source is supplied for extension development; its compiled module is not installed or enabled automatically.

The runtime fixture verifies disabled/missing grants, valid proposed output, package changes, forbidden WASI imports, and fuel exhaustion. Native acceptance constructs the manager/command views without executing third-party code. Additional panels, template/import/export callbacks, persistent plugin state, signatures, and a marketplace are deliberately outside API v1.
