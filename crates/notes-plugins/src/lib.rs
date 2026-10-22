//! Versioned, capability-limited note processors. No WASI, filesystem, network, or process imports.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
pub const MAX_TEXT: usize = 128 * 1024;
const MAX_MODULE: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub module: String,
    pub permissions: Vec<String>,
    pub commands: Vec<Command>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub title: String,
}
#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub digest: String,
    pub directory: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub id: String,
    pub digest: String,
    pub enabled: bool,
    pub permissions: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Request<'a> {
    pub api_version: u32,
    pub command: &'a str,
    pub selection: &'a str,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub api_version: u32,
    pub text: Option<String>,
    pub message: Option<String>,
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn bounded(path: &Path, max: u64) -> Result<Vec<u8>> {
    ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Extension files cannot be symlinks"
    );
    let f = std::fs::File::open(path)?;
    ensure!(
        f.metadata()?.is_file(),
        "Extension component must be a file"
    );
    let mut bytes = Vec::new();
    f.take(max + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= max,
        "Extension component exceeds its size limit"
    );
    Ok(bytes)
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.api_version == 1, "Unsupported extension API version");
        ensure!(
            id(&self.id)
                && !self.name.trim().is_empty()
                && self.name.len() <= 128
                && self.version.len() <= 64,
            "Invalid extension identity"
        );
        ensure!(
            Path::new(&self.module).components().count() == 1
                && self.module.ends_with(".wasm")
                && !self.module.starts_with('.')
                && !self.module.contains(['/', '\\']),
            "Choose one local .wasm module"
        );
        ensure!(
            !self.commands.is_empty() && self.commands.len() <= 32,
            "An extension needs 1–32 commands"
        );
        let mut commands = std::collections::BTreeSet::new();
        for c in &self.commands {
            ensure!(
                id(&c.id)
                    && !c.title.trim().is_empty()
                    && c.title.len() <= 128
                    && commands.insert(&c.id),
                "Invalid or duplicate command"
            );
        }
        ensure!(
            self.permissions
                .iter()
                .all(|p| matches!(p.as_str(), "selection.read" | "edit.propose")),
            "Unknown extension permission"
        );
        Ok(())
    }
}
impl Package {
    pub fn load(manifest: &Path) -> Result<Self> {
        let directory = manifest
            .parent()
            .context("Manifest needs a directory")?
            .canonicalize()?;
        let raw = bounded(manifest, 64 * 1024)?;
        let definition: Manifest = serde_json::from_slice(&raw)?;
        definition.validate()?;
        let wasm = bounded(&directory.join(&definition.module), MAX_MODULE)?;
        let (engine, _) = runtime();
        let module = Module::new(&engine, &wasm)?;
        ensure!(
            module.imports().next().is_none(),
            "Extensions cannot import host, WASI, network, or filesystem functions"
        );
        let mut hash = blake3::Hasher::new();
        hash.update(&raw);
        hash.update(&wasm);
        Ok(Self {
            manifest: definition,
            digest: hash.finalize().to_hex().to_string(),
            directory,
        })
    }
    /// Install immutable package content. An existing package is never silently replaced.
    pub fn install(source: &Path, storage: &Path) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let package = Self::load(source)?;
        std::fs::create_dir_all(storage)?;
        std::fs::set_permissions(storage, std::fs::Permissions::from_mode(0o700))?;
        let dir = tempfile::tempdir_in(storage)?;
        for (from, to) in [
            (source, dir.path().join("manifest.json")),
            (
                &package.directory.join(&package.manifest.module),
                dir.path().join(&package.manifest.module),
            ),
        ] {
            let bytes = bounded(from, MAX_MODULE)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        let installed = Self::load(&dir.path().join("manifest.json"))?;
        ensure!(
            installed.digest == package.digest,
            "Extension changed during installation"
        );
        let target = storage.join(format!("{}-{}", package.manifest.id, &package.digest[..16]));
        // The content-addressed directory is immutable; existing content must validate identically.
        if target.exists() {
            let existing = Self::load(&target.join("manifest.json"))?;
            ensure!(
                existing.digest == package.digest,
                "Installed package content differs"
            );
            return Ok(existing);
        }
        std::fs::rename(dir.path(), &target)?;
        std::fs::File::open(storage)?.sync_all()?;
        Self::load(&target.join("manifest.json"))
    }
    pub fn run(&self, grant: &Grant, command: &str, selection: &str) -> Result<Output> {
        ensure!(
            grant.enabled && grant.id == self.manifest.id && grant.digest == self.digest,
            "Extension is disabled or changed; review permissions again"
        );
        ensure!(
            self.manifest
                .permissions
                .iter()
                .all(|p| grant.permissions.contains(p)),
            "Required extension permission is missing"
        );
        ensure!(
            self.manifest.commands.iter().any(|c| c.id == command),
            "Unknown extension command"
        );
        ensure!(
            selection.len() <= MAX_TEXT,
            "Select a passage below 128 KiB"
        );
        let selection = if grant.permissions.iter().any(|p| p == "selection.read") {
            selection
        } else {
            ""
        };
        let input = serde_json::to_vec(&Request {
            api_version: 1,
            command,
            selection,
        })?;
        let current = Self::load(&self.directory.join("manifest.json"))?;
        ensure!(
            current.digest == self.digest,
            "Extension changed since approval"
        );
        let wasm = bounded(&self.directory.join(&self.manifest.module), MAX_MODULE)?;
        // Hash the exact execution bytes to close the validate/read replacement window.
        let raw = bounded(&self.directory.join("manifest.json"), 64 * 1024)?;
        let mut hash = blake3::Hasher::new();
        hash.update(&raw);
        hash.update(&wasm);
        ensure!(
            hash.finalize().to_hex().as_str() == grant.digest,
            "Extension changed before execution"
        );
        let (engine, limits) = runtime();
        let module = Module::new(&engine, &wasm)?;
        ensure!(
            module.imports().next().is_none(),
            "Host imports are forbidden"
        );
        let mut store = Store::new(&engine, limits);
        store.limiter(|s| s);
        store.set_fuel(10_000_000)?;
        let instance =
            Linker::<StoreLimits>::new(&engine).instantiate_and_start(&mut store, &module)?;
        let memory = instance
            .get_memory(&store, "memory")
            .context("ABI v1 requires exported memory")?;
        let alloc = instance.get_typed_func::<i32, i32>(&store, "notes_alloc")?;
        let run = instance.get_typed_func::<(i32, i32), i64>(&store, "notes_run")?;
        let ptr = alloc.call(&mut store, input.len() as i32)?;
        ensure!(ptr >= 0, "Invalid allocation pointer");
        memory.write(&mut store, ptr as usize, &input)?;
        let packed = run.call(&mut store, (ptr, input.len() as i32))? as u64;
        let offset = (packed >> 32) as usize;
        let length = (packed & 0xffff_ffff) as usize;
        ensure!(length <= MAX_TEXT, "Extension output exceeds 128 KiB");
        let mut bytes = vec![0; length];
        memory.read(&store, offset, &mut bytes)?;
        let output: Output = serde_json::from_slice(&bytes)?;
        ensure!(output.api_version == 1, "Unsupported output version");
        ensure!(
            output.text.as_ref().is_none_or(|s| s.len() <= MAX_TEXT)
                && output.message.as_ref().is_none_or(|s| s.len() <= 16 * 1024),
            "Extension output exceeds limits"
        );
        ensure!(
            output.text.is_none() || grant.permissions.iter().any(|p| p == "edit.propose"),
            "Extension has no permission to propose edits"
        );
        Ok(output)
    }
}
fn runtime() -> (Engine, StoreLimits) {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let limits = StoreLimitsBuilder::new()
        .memory_size(32 * 1024 * 1024)
        .memories(1)
        .tables(1)
        .table_elements(4096)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    (engine, limits)
}
pub fn discover(storage: &Path) -> Result<Vec<Package>> {
    let mut out = Vec::new();
    if !storage.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(storage)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Ok(p) = Package::load(&entry.path().join("manifest.json"))
        {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(dir: &Path, body: &str) -> Package {
        let manifest = Manifest {
            api_version: 1,
            id: "fixture".into(),
            name: "Fixture".into(),
            version: "1".into(),
            module: "processor.wasm".into(),
            permissions: vec!["selection.read".into(), "edit.propose".into()],
            commands: vec![Command {
                id: "run".into(),
                title: "Run".into(),
            }],
        };
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("processor.wasm"), wat::parse_str(body).unwrap()).unwrap();
        Package::load(&dir.join("manifest.json")).unwrap()
    }
    #[test]
    fn isolation_grants_and_bounded_execution() {
        let dir = tempfile::tempdir().unwrap();
        let output = r#"{"api_version":1,"text":"Reviewed only"}"#;
        let escaped = output
            .bytes()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>();
        let package = fixture(
            dir.path(),
            &format!(
                r#"(module (memory (export "memory") 1) (data (i32.const 4096) "{escaped}") (func (export "notes_alloc") (param i32)(result i32) i32.const 0) (func (export "notes_run") (param i32 i32)(result i64) i64.const {}))"#,
                (4096u64 << 32) | output.len() as u64
            ),
        );
        let mut grant = Grant {
            id: "fixture".into(),
            digest: package.digest.clone(),
            enabled: false,
            permissions: package.manifest.permissions.clone(),
        };
        assert!(package.run(&grant, "run", "source").is_err());
        grant.enabled = true;
        assert_eq!(
            package
                .run(&grant, "run", "source")
                .unwrap()
                .text
                .as_deref(),
            Some("Reviewed only")
        );
        grant.permissions.retain(|p| p != "edit.propose");
        assert!(package.run(&grant, "run", "source").is_err());
        let infinite = fixture(
            dir.path(),
            r#"(module (memory (export "memory") 1) (func (export "notes_alloc") (param i32)(result i32) i32.const 0) (func (export "notes_run") (param i32 i32)(result i64) (loop $l br $l) i64.const 0))"#,
        );
        grant.digest = infinite.digest.clone();
        grant.permissions = infinite.manifest.permissions.clone();
        assert!(infinite.run(&grant, "run", "x").is_err());
        assert!(package.run(&grant, "run", "x").is_err());
        std::fs::write(
            dir.path().join("processor.wasm"),
            wat::parse_str(
                r#"(module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32))))"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(Package::load(&dir.path().join("manifest.json")).is_err());
    }
}
