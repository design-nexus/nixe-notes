//! Vault-native templates, attachments, and recoverable multi-file operations.
use crate::{Version, children, is_markdown, load, save, save_copy};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn directory(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.components().any(|c| c.as_os_str() == ".obsidian") {
        bail!("Obsidian configuration is protected");
    }
    if path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        bail!("Use a vault-relative directory without parent traversal");
    }
    let mut target = root.to_owned();
    for component in path.components() {
        target.push(component);
        if target.exists() {
            if !target.canonicalize()?.starts_with(root) {
                bail!("Directory leaves the vault");
            }
        } else {
            std::fs::create_dir(&target)?;
        }
    }
    Ok(target)
}
pub fn templates(root: &Path, folder: &str) -> Result<Vec<PathBuf>> {
    let folder = directory(root, folder)?;
    Ok(children(root, &folder)?
        .into_iter()
        .filter(|e| !e.directory && !e.symlink)
        .map(|e| e.path)
        .collect())
}
pub fn expand_template(
    source: &str,
    title: &str,
    date: &str,
    time: &str,
    selection: &str,
) -> String {
    source
        .replace("{{title}}", title)
        .replace("{{date}}", date)
        .replace("{{time}}", time)
        .replace("{{selection}}", selection)
}
pub fn template_note(
    root: &Path,
    folder: &str,
    title: &str,
    template: Option<&Path>,
    date: &str,
    time: &str,
) -> Result<PathBuf> {
    if title.is_empty() || title.contains(['/', '\\', '\0']) || title.starts_with('.') {
        bail!("Use a plain note name");
    }
    let folder = directory(root, folder)?;
    let name = if is_markdown(Path::new(title)) {
        title.to_owned()
    } else {
        format!("{title}.md")
    };
    let target = folder.join(name);
    let source = if let Some(template) = template {
        let path = template.canonicalize()?;
        if !path.starts_with(root) {
            bail!("Template is outside the vault");
        }
        load(&path)?.text
    } else {
        format!("# {title}\n\n")
    };
    save_copy(
        &target,
        &expand_template(&source, title, date, time, ""),
        false,
    )?;
    Ok(target)
}
pub fn daily_note(
    root: &Path,
    folder: &str,
    date: &str,
    template: Option<&Path>,
    time: &str,
) -> Result<PathBuf> {
    let dir = directory(root, folder)?;
    let target = dir.join(format!("{date}.md"));
    if target.exists() {
        if !target.canonicalize()?.starts_with(root) || !is_markdown(&target) {
            bail!("Invalid daily note path");
        }
        return Ok(target);
    }
    template_note(root, folder, date, template, date, time)
}
fn safe_name(name: &str) -> String {
    let name = Path::new(name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['\0', '/', '\\'], "_");
    if name.is_empty() || name.starts_with('.') {
        "attachment".into()
    } else {
        name
    }
}
pub fn attach(root: &Path, folder: &str, source: &Path) -> Result<PathBuf> {
    let canonical = source.canonicalize()?;
    if !canonical.is_file() {
        bail!("Choose an attachment file");
    }
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    let dir = directory(root, folder)?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    std::io::copy(&mut std::fs::File::open(canonical)?, &mut temp)?;
    temp.as_file().sync_all()?;
    persist_unique(temp, &dir, &safe_name(&name))
}
pub fn attach_bytes(root: &Path, folder: &str, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = directory(root, folder)?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    persist_unique(temp, &dir, &safe_name(name))
}
fn persist_unique(mut file: tempfile::NamedTempFile, dir: &Path, name: &str) -> Result<PathBuf> {
    for index in 0..10000 {
        let base = Path::new(name);
        let target = dir.join(if index == 0 {
            name.to_owned()
        } else {
            format!(
                "{}-{index}{}",
                base.file_stem().unwrap_or_default().to_string_lossy(),
                base.extension()
                    .map_or(String::new(), |e| format!(".{}", e.to_string_lossy()))
            )
        });
        match file.persist_noclobber(&target) {
            Ok(_) => {
                std::fs::File::open(dir)?.sync_all()?;
                return Ok(target);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                file = error.file
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    bail!("Too many attachments share this name")
}
pub fn relative_link(source: &Path, target: &Path, embed: bool) -> String {
    let relative = pathdiff::diff_paths(target, source.parent().unwrap_or(Path::new("/")))
        .unwrap_or_else(|| target.to_owned());
    let path = relative
        .to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('(', "%28")
        .replace(')', "%29");
    let label = target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!("{}[{label}]({path})", if embed { "!" } else { "" })
}
#[derive(Clone, Debug)]
pub enum AssetData {
    Image {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Markdown {
        path: PathBuf,
        blocks: Vec<notes_markdown::render::ReadBlock>,
    },
    File {
        path: PathBuf,
        label: String,
    },
}
pub fn asset(root: &Path, db: &Path, source: &Path, target: &str, wiki: bool) -> Result<AssetData> {
    if let Ok(link) = crate::resolve_link(db, root, source, target, wiki) {
        let doc = load(&link.path)?;
        let parsed = notes_markdown::parse(&doc.text);
        let mut blocks = parsed.reading;
        if let Some(fragment) = link.fragment {
            let range = if let Some(id) = fragment.strip_prefix('^') {
                let at = parsed
                    .blocks
                    .iter()
                    .find(|b| b.id == id)
                    .context("Embedded block not found")?
                    .offset;
                let from = doc.text[..at].rfind("\n\n").map_or(0, |n| n + 2);
                let end = doc.text[at..]
                    .find("\n\n")
                    .map_or(doc.text.len(), |n| at + n);
                from..end
            } else {
                let heading = parsed
                    .headings
                    .iter()
                    .find(|h| h.text.eq_ignore_ascii_case(&fragment))
                    .context("Embedded heading not found")?;
                let end = parsed
                    .headings
                    .iter()
                    .find(|h| h.offset > heading.offset && h.level <= heading.level)
                    .map_or(doc.text.len(), |h| h.offset);
                heading.offset..end
            };
            blocks.retain(|b| range.contains(&b.offset));
        }
        return Ok(AssetData::Markdown {
            path: link.path,
            blocks,
        });
    }
    let (target, _) = crate::links::local_target(target, wiki)?;
    let bases = if Path::new(&target).is_absolute() {
        vec![PathBuf::from(&target)]
    } else {
        vec![
            source.parent().unwrap_or(root).join(&target),
            root.join(&target),
        ]
    };
    let mut found = None;
    for path in bases {
        if let Ok(path) = path.canonicalize() {
            if !path.starts_with(root) {
                bail!("Attachment leaves the vault");
            }
            if path.is_file() {
                found = Some(path);
                break;
            }
        }
    }
    if found.is_none() && !target.contains('/') {
        let mut pending = vec![root.to_owned()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if entry.file_name().to_string_lossy().starts_with('.') || kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if entry.file_name().to_string_lossy() == target {
                    if found.is_some() {
                        bail!("Attachment name is ambiguous; use its folder path");
                    }
                    found = Some(entry.path());
                }
            }
        }
    }
    let path = found.context("Embedded note or attachment not found")?;
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif"
    ) {
        if std::fs::metadata(&path)?.len() > 64 * 1024 * 1024 {
            bail!("Image is too large to preview; open the attachment instead");
        }
        Ok(AssetData::Image {
            bytes: std::fs::read(&path)?,
            path,
        })
    } else {
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Ok(AssetData::File { path, label })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Change {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub bom: bool,
    pub etag: String,
    pub digest: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MovePlan {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub changes: Vec<Change>,
    pub moved: bool,
    pub journal: PathBuf,
}
fn write_plan(plan: &MovePlan) -> Result<()> {
    let folder = plan.journal.parent().context("No journal directory")?;
    std::fs::create_dir_all(folder)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    serde_json::to_writer(&mut file, plan)?;
    file.as_file().sync_all()?;
    file.persist(&plan.journal).map_err(|e| e.error)?;
    std::fs::File::open(folder)?.sync_all()?;
    Ok(())
}
fn notes(root: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_owned()];
    let mut result = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in crate::children_all(root, &dir)? {
            if entry.symlink {
                continue;
            }
            if entry.directory {
                pending.push(entry.path);
            } else if is_markdown(&entry.path)
                || entry.path.extension().is_some_and(|e| e == "canvas")
            {
                result.push(entry.path);
            }
        }
    }
    Ok(result)
}
fn moved_path(path: &Path, source: &Path, target: &Path) -> PathBuf {
    if path == source {
        target.to_owned()
    } else if let Ok(relative) = path.strip_prefix(source) {
        target.join(relative)
    } else {
        path.to_owned()
    }
}
pub fn plan_move(
    root: &Path,
    db: &Path,
    state: &Path,
    source: &Path,
    destination: &Path,
) -> Result<MovePlan> {
    if std::fs::symlink_metadata(source)?.file_type().is_symlink() {
        bail!("Move the symlink in Files; Notes does not rename its target");
    }
    let source = source.canonicalize()?;
    if source.components().any(|c| c.as_os_str() == ".obsidian")
        || destination
            .components()
            .any(|c| c.as_os_str() == ".obsidian")
    {
        bail!("Obsidian configuration is protected");
    }
    if source == root || !source.starts_with(root) {
        bail!("Choose a file or folder inside the vault");
    }
    let parent = destination
        .parent()
        .context("Destination needs a directory")?
        .canonicalize()?;
    if !parent.starts_with(root) || parent.starts_with(&source) {
        bail!("Destination must stay inside the vault and outside the moved folder");
    }
    let destination = parent.join(
        destination
            .file_name()
            .context("Destination needs a name")?,
    );
    if destination.exists() || std::fs::symlink_metadata(&destination).is_ok() {
        bail!("Destination already exists");
    }
    let conn = crate::search::connect(db)?;
    let mut changes = Vec::new();
    for path in notes(root)? {
        let loaded = load(&path)?;
        if path.extension().is_some_and(|e| e == "canvas") {
            if let Ok(mut canvas) = crate::canvas::Canvas::parse(&loaded.text) {
                let mut changed = false;
                for node in &mut canvas.nodes {
                    for key in ["file", "background"] {
                        let value = node.field(key).to_owned();
                        if value.is_empty() {
                            continue;
                        }
                        let raw = Path::new(&value);
                        let target = if raw.is_absolute() {
                            raw.to_owned()
                        } else {
                            root.join(raw)
                        };
                        let Ok(target) = target.canonicalize() else {
                            continue;
                        };
                        if !target.starts_with(root) {
                            continue;
                        }
                        let new_target = moved_path(&target, &source, &destination);
                        if new_target != target {
                            let value = if raw.is_absolute() {
                                new_target.to_string_lossy().to_string()
                            } else {
                                new_target.strip_prefix(root)?.to_string_lossy().to_string()
                            };
                            node.set(key, &value);
                            changed = true;
                        }
                    }
                }
                if changed {
                    changes.push(Change {
                        path,
                        before: loaded.text,
                        after: serde_json::to_string_pretty(&canvas)?,
                        bom: loaded.bom,
                        etag: loaded.version.etag,
                        digest: loaded.version.digest.to_hex().to_string(),
                    });
                }
            }
            continue;
        }
        let parsed = notes_markdown::parse(&loaded.text);
        let new_source = moved_path(&path, &source, &destination);
        let mut replacements = Vec::new();
        for link in &parsed.links {
            let resolved_path =
                match crate::links::resolve(&conn, root, &path, &link.target, link.wiki) {
                    Ok(link) => link.path,
                    Err(_) => match asset(root, db, &path, &link.target, link.wiki) {
                        Ok(AssetData::Image { path, .. }) | Ok(AssetData::File { path, .. }) => {
                            path
                        }
                        _ => continue,
                    },
                };
            if link.target.starts_with('#') {
                continue;
            }
            let new_target = moved_path(&resolved_path, &source, &destination);
            if new_source == path && new_target == resolved_path {
                continue;
            }

            let raw = &loaded.text[link.range.clone()];
            let Some(position) = raw.find(&link.target) else {
                bail!("Cannot safely rewrite a link in {}", path.display());
            };
            let (_, fragment) = crate::links::local_target(&link.target, link.wiki)?;
            let old_target = link.target.split('#').next().unwrap_or("");
            let mut target = if Path::new(old_target).is_absolute() {
                new_target.to_string_lossy().into_owned()
            } else if link.wiki {
                new_target
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .into_owned()
            } else {
                pathdiff::diff_paths(&new_target, new_source.parent().unwrap_or(root))
                    .context("Cannot make relative link")?
                    .to_string_lossy()
                    .into_owned()
            };
            if link.wiki
                && is_markdown(&new_target)
                && !old_target.to_lowercase().ends_with(".md")
                && !old_target.to_lowercase().ends_with(".markdown")
                && let Some(stem) = Path::new(&target).file_stem()
            {
                target = Path::new(&target)
                    .with_file_name(stem)
                    .to_string_lossy()
                    .into_owned();
            }
            if !link.wiki {
                target = target
                    .replace('%', "%25")
                    .replace(' ', "%20")
                    .replace('#', "%23");
            }
            if let Some(fragment) = fragment {
                target.push('#');
                target.push_str(&fragment);
            }
            replacements.push((
                link.range.start + position..link.range.start + position + link.target.len(),
                target,
            ));
        }
        if !replacements.is_empty() {
            let mut after = loaded.text.clone();
            replacements.sort_by_key(|(r, _)| r.start);
            for (range, text) in replacements.into_iter().rev() {
                after.replace_range(range, &text);
            }
            if after != loaded.text {
                changes.push(Change {
                    path,
                    before: loaded.text,
                    after,
                    bom: loaded.bom,
                    etag: loaded.version.etag,
                    digest: loaded.version.digest.to_hex().to_string(),
                });
            }
        }
    }
    let unique = format!(
        "move-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let journal = state.join("transactions").join(format!("{unique}.json"));
    Ok(MovePlan {
        source,
        destination,
        changes,
        moved: false,
        journal,
    })
}
pub fn execute_move(plan: &mut MovePlan) -> Result<()> {
    if !plan.source.exists() && plan.destination.exists() {
        plan.moved = true;
    }
    if !plan.moved && plan.destination.exists() {
        bail!("Destination already exists; no files were changed");
    }
    write_plan(plan)?;
    for change in &plan.changes {
        let path = if plan.moved {
            moved_path(&change.path, &plan.source, &plan.destination)
        } else {
            change.path.clone()
        };
        let loaded = load(&path)?;
        if loaded.text == change.after && loaded.bom == change.bom {
            continue;
        }
        if loaded.text != change.before || loaded.bom != change.bom {
            bail!(
                "{} changed during the operation; journal retained at {}",
                path.display(),
                plan.journal.display()
            );
        }
        let expected = Version {
            etag: loaded.version.etag,
            digest: loaded.version.digest,
        };
        save(&path, &change.after, change.bom, &expected).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    if !plan.moved {
        if !plan.source.exists() && plan.destination.exists() {
            plan.moved = true;
        } else {
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                &plan.source,
                rustix::fs::CWD,
                &plan.destination,
                rustix::fs::RenameFlags::NOREPLACE,
            )?;
            plan.moved = true;
        }
        write_plan(plan)?;
    }
    let state = plan
        .journal
        .parent()
        .and_then(Path::parent)
        .context("No history state directory")?;
    crate::history::relocate(state, &plan.source, &plan.destination)?;
    std::fs::remove_file(&plan.journal)?;
    Ok(())
}
pub fn restore_move(plan: &mut MovePlan) -> Result<()> {
    if plan.moved || !plan.source.exists() && plan.destination.exists() {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            &plan.destination,
            rustix::fs::CWD,
            &plan.source,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        plan.moved = false;
        write_plan(plan)?;
    }
    for change in &plan.changes {
        let loaded = load(&change.path)?;
        if loaded.text == change.before {
            continue;
        }
        if loaded.text != change.after || loaded.bom != change.bom {
            bail!(
                "Cannot restore {}; it has external changes",
                change.path.display()
            );
        }
        save(&change.path, &change.before, change.bom, &loaded.version)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let state = plan
        .journal
        .parent()
        .and_then(Path::parent)
        .context("No history state directory")?;
    crate::history::relocate(state, &plan.destination, &plan.source)?;
    std::fs::remove_file(&plan.journal)?;
    Ok(())
}
pub fn interrupted_moves(state: &Path) -> Result<Vec<MovePlan>> {
    let dir = state.join("transactions");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut plans = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "json") {
            plans.push(serde_json::from_slice(&std::fs::read(entry.path())?)?);
        }
    }
    Ok(plans)
}
pub fn duplicate(root: &Path, source: &Path, destination: &Path) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    let parent = destination
        .parent()
        .context("No destination folder")?
        .canonicalize()?;
    if !source.starts_with(root) || source == root || !parent.starts_with(root) {
        bail!("Copy must stay within the vault");
    }
    let destination = parent.join(destination.file_name().context("Copy needs a name")?);
    if source
        .components()
        .chain(destination.components())
        .any(|c| c.as_os_str() == ".obsidian")
    {
        bail!("Obsidian configuration is protected");
    }
    if source.is_dir() {
        fn copy_tree(source: &Path, target: &Path) -> Result<()> {
            for entry in std::fs::read_dir(source)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                let destination = target.join(entry.file_name());
                if kind.is_symlink() {
                    std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, destination)?;
                } else if kind.is_dir() {
                    std::fs::create_dir(&destination)?;
                    copy_tree(&entry.path(), &destination)?;
                } else if kind.is_file() {
                    std::fs::copy(entry.path(), &destination)?;
                    std::fs::File::open(destination)?.sync_all()?;
                }
            }
            std::fs::File::open(target)?.sync_all()?;
            Ok(())
        }
        if destination.starts_with(&source) {
            bail!("Cannot copy a folder into itself");
        }
        let stage = tempfile::TempDir::new_in(destination.parent().unwrap())?;
        copy_tree(&source, stage.path())?;
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            stage.path(),
            rustix::fs::CWD,
            &destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        return Ok(destination.to_owned());
    }
    let mut temp = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
    std::io::copy(&mut std::fs::File::open(source)?, &mut temp)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&destination).map_err(|e| e.error)?;
    Ok(destination.to_owned())
}
pub fn trash(root: &Path, path: &Path) -> Result<()> {
    use gio::prelude::*;
    let canonical = path.canonicalize()?;
    if canonical == root
        || !canonical.starts_with(root)
        || path.components().any(|c| c.as_os_str() == ".obsidian")
    {
        bail!("This path cannot be trashed from Notes");
    }
    gio::File::for_path(path).trash(gio::Cancellable::NONE)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_workspace_operations_and_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        std::fs::create_dir(&root).unwrap();
        let state = temp.path().join("state");
        directory(&root, "Templates").unwrap();
        let template = root.join("Templates/Research.md");
        std::fs::write(&template,"---\ntitle: '{{title}}'\nstatus: draft\n---\n# {{title}}\n{{date}} {{time}}\n{{selection}}\n").unwrap();
        let project =
            template_note(&root, "", "Project", Some(&template), "2026-10-21", "10:30").unwrap();
        assert!(load(&project).unwrap().text.contains("2026-10-21 10:30"));
        let daily =
            daily_note(&root, "Daily Notes", "2026-10-21", Some(&template), "10:30").unwrap();
        let before = std::fs::read(&daily).unwrap();
        assert_eq!(
            daily_note(&root, "Daily Notes", "2026-10-21", None, "11:00").unwrap(),
            daily
        );
        assert_eq!(std::fs::read(&daily).unwrap(), before);
        let external = temp.path().join("picture.png");
        std::fs::write(&external, [1, 2, 3]).unwrap();
        let image = attach(&root, "Attachments", &external).unwrap();
        let image2 = attach(&root, "Attachments", &external).unwrap();
        assert_ne!(image, image2);
        assert_eq!(std::fs::read(&image).unwrap(), [1, 2, 3]);
        let source = root.join("Source.md");
        std::fs::write(&source,"---\ncustom: keep\n---\n[[Project|Alias]] [link](Project.md#Heading) ![image](Attachments/picture.png)\n`[[Project]]`\n").unwrap();
        let db = temp.path().join("index.sqlite");
        crate::rebuild_index(&db, &root).unwrap();
        let destination = root.join("Renamed Project.md");
        let mut plan = plan_move(&root, &db, &state, &project, &destination).unwrap();
        assert_eq!(plan.changes.len(), 1);
        assert!(plan.changes[0].after.contains("[[Renamed Project|Alias]]"));
        assert!(
            plan.changes[0]
                .after
                .contains("Renamed%20Project.md#Heading")
        );
        assert!(plan.changes[0].after.contains("`[[Project]]`"));
        // A partial write can be restored without changing the untouched note body.
        write_plan(&plan).unwrap();
        let change = &plan.changes[0];
        let loaded = load(&change.path).unwrap();
        save(&change.path, &change.after, change.bom, &loaded.version).unwrap();
        assert_eq!(interrupted_moves(&state).unwrap().len(), 1);
        restore_move(&mut plan).unwrap();
        assert!(load(&source).unwrap().text.contains("[[Project|Alias]]"));
        execute_move(&mut plan).unwrap();
        assert!(destination.exists());
        assert!(!project.exists());
        assert!(interrupted_moves(&state).unwrap().is_empty());
        assert!(
            load(&source)
                .unwrap()
                .text
                .starts_with("---\ncustom: keep\n---\n")
        );
        crate::rebuild_index(&db, &root).unwrap();
        let canvas_path = root.join("Map.canvas");
        std::fs::write(&canvas_path,r#"{"nodes":[{"id":"image","type":"file","file":"Attachments/picture.png","x":0,"y":0,"width":200,"height":200}],"custom":"keep"}"#).unwrap();
        let renamed_image = root.join("Attachments/renamed.png");
        let mut image_plan = plan_move(&root, &db, &state, &image, &renamed_image).unwrap();
        assert!(
            image_plan
                .changes
                .iter()
                .any(|c| c.after.contains("Attachments/renamed.png"))
        );
        execute_move(&mut image_plan).unwrap();
        assert_eq!(std::fs::read(&renamed_image).unwrap(), [1, 2, 3]);
        let canvas_text = std::fs::read_to_string(&canvas_path).unwrap();
        assert!(canvas_text.contains("Attachments/renamed.png"));
        assert!(canvas_text.contains("custom"));
        let copy = duplicate(&root, &source, &root.join("Copy.md")).unwrap();
        assert_eq!(
            std::fs::read(copy).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert!(duplicate(&root, &source, &source).is_err());
        duplicate(
            &root,
            &root.join("Attachments"),
            &root.join("Attachments copy"),
        )
        .unwrap();
        assert!(root.join("Attachments copy/renamed.png").exists());
        let recursive_target = root.join("Attachments copy/../Attachments/recursive");
        assert!(duplicate(&root, &root.join("Attachments"), &recursive_target).is_err());
        assert!(
            plan_move(
                &root,
                &db,
                &state,
                &root.join("Attachments"),
                &recursive_target
            )
            .is_err()
        );
        std::fs::create_dir(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/config.json"), "{}").unwrap();
        let config_note = root.join(".obsidian/config.md");
        std::fs::write(&config_note, "keep").unwrap();
        let protected = crate::load(&config_note).unwrap();
        assert!(protected.read_only);
        assert!(crate::save(&config_note, "overwrite", false, &protected.version).is_err());
        assert!(crate::save_copy(&root.join(".obsidian/new.md"), "no", false).is_err());
        assert_eq!(std::fs::read_to_string(&config_note).unwrap(), "keep");
        assert!(duplicate(&root, &source, &root.join(".obsidian/copy.md")).is_err());
        assert!(directory(&root, "../escape").is_err());
        assert!(directory(&root, ".obsidian/attachments").is_err());
        // External edits stop a planned move, and neither resume nor restore overwrites them.
        crate::rebuild_index(&db, &root).unwrap();
        let mut conflict =
            plan_move(&root, &db, &state, &destination, &root.join("Final.md")).unwrap();
        std::fs::write(&source, "external edit").unwrap();
        assert!(execute_move(&mut conflict).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "external edit");
        assert!(restore_move(&mut conflict).is_err());
        assert!(destination.exists());
    }
}
