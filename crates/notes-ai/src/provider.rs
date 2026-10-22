use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixStream,
    },
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Capabilities {
    pub api_version: u32,
    pub generation: bool,
    pub local: bool,
    pub embedding: bool,
    #[serde(default)]
    pub embedding_model: String,
    #[serde(default)]
    pub embedding_profile: String,
    #[serde(default)]
    pub embedding_endpoint: String,
}
#[derive(Clone, Debug)]
pub struct Embeddings {
    pub profile: String,
    pub vectors: Vec<Vec<f32>>,
}
pub trait Provider: Send + Sync {
    fn capabilities(&self) -> Result<Capabilities>;
    fn generate(&self, system: &str, text: &str) -> Result<String>;
    fn embed(&self, input: &[String]) -> Result<Embeddings>;
}
#[derive(Clone, Debug)]
pub struct Assistant {
    pub socket: PathBuf,
}
impl Default for Assistant {
    fn default() -> Self {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
                    })
                    .join("nixe-assistant/runtime")
            });
        Self {
            socket: base.join("nixe-assistant/assistant.sock"),
        }
    }
}
impl Assistant {
    pub fn request(&self, action: Value, timeout: Duration) -> Result<Value> {
        let metadata = std::fs::symlink_metadata(&self.socket)
            .context("Assistant is unavailable; Notes works normally without it")?;
        if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o077 != 0 {
            bail!("Assistant must use its private local socket");
        }
        let mut socket = UnixStream::connect(&self.socket).context("Connect to Assistant")?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!(
            "notes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let bytes = serde_json::to_vec(&json!({"type":"request","request_id":id,"action":action}))?;
        if bytes.len() > 256 * 1024 {
            bail!("Assistant request exceeds limit");
        }
        socket.write_all(&bytes)?;
        socket.write_all(b"\n")?;
        let start = Instant::now();
        let mut reader = BufReader::new(socket);
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                bail!(
                    "Assistant request timed out; an older running Assistant may need its normal restart to use the installed protocol update"
                );
            }
            reader.get_ref().set_read_timeout(Some(remaining))?;
            let mut line = Vec::new();
            let n = Read::by_ref(&mut reader)
                .take(8 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .map_err(|error|if matches!(error.kind(),std::io::ErrorKind::TimedOut|std::io::ErrorKind::WouldBlock){anyhow::anyhow!("Assistant model API timed out. An older running process will use the installed update after its next normal restart.")}else{anyhow::anyhow!("Read Assistant reply: {error}")})?;
            if n == 0 {
                bail!("Assistant disconnected");
            }
            if n > 8 * 1024 * 1024 {
                bail!("Assistant response exceeds limit");
            }
            let event: Value = serde_json::from_slice(&line)?;
            if event["type"] == "result" && event["request_id"] == id {
                if event["ok"] != true {
                    bail!(
                        "{}",
                        event["error"]
                            .as_str()
                            .unwrap_or("Assistant request failed")
                    );
                }
                return Ok(event["data"].clone());
            }
        }
    }
    pub fn configure_embeddings(
        &self,
        enabled: bool,
        endpoint: &str,
        model: &str,
    ) -> Result<Capabilities> {
        let data=self.request(json!({"type":"model_configure_embeddings","enabled":enabled,"endpoint":endpoint,"model":model}),Duration::from_secs(5))?;
        Ok(serde_json::from_value(data)?)
    }
}
impl Provider for Assistant {
    fn capabilities(&self) -> Result<Capabilities> {
        let c: Capabilities = serde_json::from_value(
            self.request(json!({"type":"model_capabilities"}), Duration::from_secs(2))?,
        )?;
        if c.api_version != 1 || !c.local {
            bail!("Unsupported or non-local Assistant model API");
        }
        Ok(c)
    }
    fn generate(&self, system: &str, text: &str) -> Result<String> {
        let caps = self.capabilities()?;
        if !caps.generation {
            bail!("Assistant has no local generation provider");
        }
        let data=self.request(json!({"type":"model_generate","system":system,"text":text,"max_tokens":2048,"temperature":0.25}),Duration::from_secs(125))?;
        let text = data["text"]
            .as_str()
            .context("Assistant returned no text")?;
        if text.len() > 1024 * 1024 {
            bail!("Generated text exceeds limit");
        }
        Ok(text.to_owned())
    }
    fn embed(&self, input: &[String]) -> Result<Embeddings> {
        let caps = self.capabilities()?;
        if !caps.embedding {
            bail!("Configure an optional local embedding model in Assistant first");
        }
        let data = self.request(
            json!({"type":"model_embed","input":input}),
            Duration::from_secs(95),
        )?;
        let vectors: Vec<Vec<f32>> = serde_json::from_value(data["vectors"].clone())?;
        validate_vectors(&vectors, input.len())?;
        Ok(Embeddings {
            profile: data["profile"]
                .as_str()
                .context("Embedding profile missing")?
                .to_owned(),
            vectors,
        })
    }
}
pub fn validate_vectors(vectors: &[Vec<f32>], count: usize) -> Result<usize> {
    let dimensions = vectors.first().map_or(0, Vec::len);
    if dimensions == 0
        || dimensions > 16384
        || vectors.len() != count
        || vectors.iter().any(|v| {
            v.len() != dimensions
                || v.iter().any(|n| !n.is_finite())
                || v.iter().map(|n| (*n as f64).powi(2)).sum::<f64>() <= 0.0
        })
    {
        bail!("Invalid embedding count, dimensions, or values");
    }
    Ok(dimensions)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correlated_local_transport_and_vector_validation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("assistant.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            writeln!(
                socket,
                "{}",
                json!({"type":"result","request_id":"other-client","ok":true,"data":{}})
            )
            .unwrap();
            writeln!(socket,"{}",json!({"type":"result","request_id":request["request_id"],"ok":true,"data":{"api_version":1,"generation":true,"local":true,"embedding":false}})).unwrap();
        });
        assert!(
            Assistant { socket: path }
                .capabilities()
                .unwrap()
                .generation
        );
        thread.join().unwrap();
        assert!(validate_vectors(&[vec![f32::NAN]], 1).is_err());
        assert!(validate_vectors(&[vec![0.0]], 1).is_err());
        assert_eq!(validate_vectors(&[vec![1.0, 0.0]], 1).unwrap(), 2);
    }
}
