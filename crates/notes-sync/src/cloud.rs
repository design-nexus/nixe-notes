//! Account-backed, append-only Markdown revisions. No tokens enter Notes.
use super::*;
use nyx_cloud::{Client, SetupSession};
use serde_json::json;
#[derive(Clone, Debug, Deserialize)]
struct Revision {
    id: String,
    path: String,
    parents: Vec<String>,
    deleted: bool,
    #[serde(default)]
    size: u64,
}
pub struct CloudRemote {
    session: SetupSession,
    heads: BTreeMap<String, Vec<Revision>>,
}
/// Call on a worker. Account enablement is owned by Settings, not local Notes preferences.
pub fn online_profiles() -> Result<Vec<Config>> {
    Ok(Client::connect()?
        .list()?
        .into_iter()
        .filter(|r| !r.notes.vault.is_empty())
        .map(|r| Config {
            root: r.notes.vault.into(),
            account_id: Some(r.id),
            enabled: r.notes.enabled,
            ..Default::default()
        })
        .collect())
}
impl CloudRemote {
    pub fn connect(id: &str) -> Result<Self> {
        Ok(Self {
            session: Client::connect()?.notes_session(id)?,
            heads: BTreeMap::new(),
        })
    }
    fn refresh(&mut self) -> Result<()> {
        let entries: Vec<Revision> =
            serde_json::from_value(self.session.request(json!({"action":"list"}))?)?;
        let mut paths: BTreeMap<String, Vec<Revision>> = BTreeMap::new();
        for entry in entries {
            validate_path(&entry.path)?;
            paths.entry(entry.path.clone()).or_default().push(entry);
        }
        self.heads.clear();
        for (path, revisions) in paths {
            let ancestors: BTreeSet<_> = revisions
                .iter()
                .flat_map(|r| r.parents.iter().cloned())
                .collect();
            let mut heads: Vec<_> = revisions
                .into_iter()
                .filter(|r| !ancestors.contains(&r.id))
                .collect();
            heads.sort_by(|a, b| a.id.cmp(&b.id));
            self.heads.insert(path, heads);
        }
        Ok(())
    }
    fn etag(heads: &[Revision]) -> Result<String> {
        Ok(serde_json::to_string(
            &heads.iter().map(|r| &r.id).collect::<Vec<_>>(),
        )?)
    }
    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        etag: Option<&str>,
        deleted: bool,
    ) -> Result<String> {
        validate_path(path)?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "Account sync files are limited to 32 MiB"
        );
        self.refresh()?;
        let heads = self.heads.get(path).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(expected) = etag {
            ensure!(
                Self::etag(heads)? == expected,
                "Cloud note changed; synchronize again"
            );
        } else {
            ensure!(heads.iter().all(|h| h.deleted), "Cloud note already exists");
        }
        let parents: Vec<_> = heads.iter().map(|r| r.id.clone()).collect();
        let result = self.session.request(
            json!({"action":"put","path":path,"parents":parents,"bytes":bytes,"deleted":deleted}),
        )?;
        let id = result["id"]
            .as_str()
            .context("Cloud revision not committed")?
            .to_owned();
        let revision = Revision {
            id: id.clone(),
            path: path.into(),
            parents,
            deleted,
            size: bytes.len() as u64,
        };
        let etag = Self::etag(std::slice::from_ref(&revision))?;
        self.heads.insert(path.into(), vec![revision]);
        Ok(etag)
    }
}
impl Transport for CloudRemote {
    fn has_divergent_versions(&self, path: &str) -> bool {
        self.heads.get(path).is_some_and(|h| h.len() > 1)
    }
    fn list(&mut self, cancel: &AtomicBool) -> Result<BTreeMap<String, RemoteFile>> {
        cancelled(cancel)?;
        self.refresh()?;
        cancelled(cancel)?;
        self.heads
            .iter()
            .filter_map(|(path, heads)| {
                let active = heads.iter().rev().find(|r| !r.deleted)?;
                Some(Self::etag(heads).map(|etag| {
                    (
                        path.clone(),
                        RemoteFile {
                            etag,
                            size: active.size,
                        },
                    )
                }))
            })
            .collect()
    }
    fn get(&mut self, path: &str, etag: &str) -> Result<Vec<u8>> {
        validate_path(path)?;
        self.refresh()?;
        let heads = self
            .heads
            .get(path)
            .context("Cloud note no longer exists")?;
        ensure!(
            Self::etag(heads)? == etag,
            "Cloud note changed; synchronize again"
        );
        let revision = heads
            .iter()
            .rev()
            .find(|r| !r.deleted)
            .context("Cloud note was deleted")?;
        let result = self
            .session
            .request(json!({"action":"get","id":revision.id,"path":path}))?;
        let bytes: Vec<u8> = serde_json::from_value(result["bytes"].clone())?;
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "Cloud note exceeds safety limit"
        );
        Ok(bytes)
    }
    fn put(&mut self, path: &str, bytes: &[u8], etag: Option<&str>) -> Result<String> {
        self.write(path, bytes, etag, false)
    }
    fn delete(&mut self, path: &str, etag: &str) -> Result<()> {
        self.write(path, &[], Some(etag), true)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the isolated Cloud accounts fixture"]
    fn cloud_account_transport() {
        let id = std::env::var("NYX_CLOUD_TEST_ID").unwrap();
        let root = PathBuf::from(std::env::var("NYX_NOTES_TEST_ROOT").unwrap());
        let state = tempfile::tempdir().unwrap();
        let config = Config {
            root: root.clone(),
            account_id: Some(id.clone()),
            enabled: true,
            ..Default::default()
        };
        config.validate().unwrap();
        let engine = Engine::open(state.path(), config).unwrap();
        let mut remote = CloudRemote::connect(&id).unwrap();
        let cancel = AtomicBool::new(false);
        let blocked = BTreeSet::new();
        let report = engine.run(&mut remote, &blocked, &cancel).unwrap();
        assert_eq!(report.downloaded, 1);
        assert_eq!(
            std::fs::read(root.join("hello.md")).unwrap(),
            b"# Updated\n"
        );
        std::fs::write(root.join("cloud.md"), b"# Local\n").unwrap();
        assert_eq!(
            engine.run(&mut remote, &blocked, &cancel).unwrap().uploaded,
            1
        );
        let mut other = CloudRemote::connect(&id).unwrap();
        let etag = other.list(&cancel).unwrap()["cloud.md"].etag.clone();
        other
            .put("cloud.md", b"# Remote edit\n", Some(&etag))
            .unwrap();
        std::fs::write(root.join("cloud.md"), b"# Concurrent local edit\n").unwrap();
        assert_eq!(
            engine
                .run(&mut remote, &blocked, &cancel)
                .unwrap()
                .conflicts,
            1
        );
        assert_eq!(
            std::fs::read(root.join("cloud.md")).unwrap(),
            b"# Concurrent local edit\n"
        );
        let conflict = engine.conflict_for("cloud.md").unwrap();
        engine
            .resolve(&mut remote, &conflict, Resolution::Local)
            .unwrap();
        let current = other.list(&cancel).unwrap()["cloud.md"].etag.clone();
        assert_eq!(
            other.get("cloud.md", &current).unwrap(),
            b"# Concurrent local edit\n"
        );
    }
}
