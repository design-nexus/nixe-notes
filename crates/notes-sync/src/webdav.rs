use super::*;

use reqwest::{
    Method, StatusCode,
    blocking::{Client, Response},
    header::{ETAG, IF_MATCH, IF_NONE_MATCH},
};
use url::Url;
pub(crate) fn endpoint(raw: &str, allow_test_http: bool) -> Result<Url> {
    let mut url = Url::parse(raw)?;
    ensure!(
        url.scheme() == "https"
            || allow_test_http
                && url.scheme() == "http"
                && url
                    .host_str()
                    .is_some_and(|h| h == "127.0.0.1" || h == "[::1]"),
        "WebDAV requires HTTPS"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Keep credentials, queries, and fragments out of the server URL"
    );
    ensure!(url.host_str().is_some(), "Enter a server hostname");
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}
pub struct WebDav {
    client: Client,
    root: Url,
    username: String,
    password: Secret,
}
impl WebDav {
    pub fn new(config: &Config, password: Secret) -> Result<Self> {
        Self::build(config, password, false)
    }
    fn build(config: &Config, password: Secret, test_http: bool) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            root: endpoint(&config.endpoint, test_http)?,
            username: config.username.clone(),
            password,
        })
    }
    fn url(&self, path: &str) -> Result<Url> {
        validate_path(path)?;
        let mut url = self.root.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid collection URL"))?;
            segments.pop_if_empty();
            for p in path.split('/') {
                segments.push(p);
            }
        }
        Ok(url)
    }
    fn request(&self, method: Method, url: Url) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, url)
            .basic_auth(&self.username, Some(self.password.expose()))
    }
    fn checked(response: Response) -> Result<Response> {
        let status = response.status();
        ensure!(
            status.is_success(),
            "WebDAV returned {status}; conditional conflicts require another sync, authentication failures require account settings"
        );
        Ok(response)
    }
    fn bounded(response: Response, max: u64) -> Result<Vec<u8>> {
        ensure!(
            response.content_length().is_none_or(|s| s <= max),
            "WebDAV response exceeds transfer limit"
        );
        let mut bytes = Vec::new();
        response.take(max + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= max,
            "WebDAV response exceeds transfer limit"
        );
        Ok(bytes)
    }
    fn etag(response: &Response) -> Result<String> {
        let tag = response
            .headers()
            .get(ETAG)
            .context("WebDAV server did not supply an ETag")?
            .to_str()?;
        strong(tag)?;
        Ok(tag.into())
    }
    /// Explicitly create only the configured final collection. Never create server/account ancestors.
    pub fn create_collection(&self) -> Result<()> {
        let status = self
            .request(Method::from_bytes(b"MKCOL")?, self.root.clone())
            .send()?
            .status();
        ensure!(
            status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED,
            "Cannot create collection: {status}"
        );
        Ok(())
    }
    fn parents(&self, path: &str) -> Result<()> {
        let components = path.split('/').collect::<Vec<_>>();
        for i in 1..components.len() {
            let mut url = self.url(&components[..i].join("/"))?;
            url.set_path(&format!("{}/", url.path()));
            let status = self
                .request(Method::from_bytes(b"MKCOL")?, url)
                .send()?
                .status();
            ensure!(
                status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED,
                "Cannot create remote folder: {status}"
            );
        }
        Ok(())
    }
}
fn strong(etag: &str) -> Result<()> {
    ensure!(
        etag.starts_with('"')
            && etag.ends_with('"')
            && !etag.contains(['\r', '\n'])
            && etag.len() <= 1024,
        "Server must provide strong quoted ETags"
    );
    Ok(())
}
impl Transport for WebDav {
    fn list(&mut self, cancel: &AtomicBool) -> Result<BTreeMap<String, RemoteFile>> {
        let mut pending = vec![(self.root.clone(), 0usize)];
        let mut visited = BTreeSet::new();
        let mut out = BTreeMap::new();
        while let Some((url, depth)) = pending.pop() {
            cancelled(cancel)?;
            ensure!(depth <= 64, "Remote nesting limit exceeded");
            if !visited.insert(url.as_str().to_owned()) {
                continue;
            }
            let response=self.request(Method::from_bytes(b"PROPFIND")?,url.clone()).header("Depth","1").header("Content-Type","application/xml; charset=utf-8").body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getetag/><d:getcontentlength/></d:prop></d:propfind>"#).send()?;
            ensure!(
                response.status() == StatusCode::MULTI_STATUS,
                "Remote listing incomplete: {}",
                response.status()
            );
            let bytes = Self::bounded(response, 16 * 1024 * 1024)?;
            for (path, collection, etag, size) in parse_listing(&self.root, &url, &bytes)? {
                if path.is_empty() {
                    continue;
                }
                if validate_path(path.trim_end_matches('/')).is_err() {
                    continue;
                }
                if collection {
                    let mut next = self.url(path.trim_end_matches('/'))?;
                    next.set_path(&format!("{}/", next.path()));
                    pending.push((next, depth + 1));
                } else {
                    strong(&etag)?;
                    ensure!(out.len() < 100_000, "Remote file limit exceeded");
                    ensure!(
                        out.insert(path, RemoteFile { etag, size }).is_none(),
                        "Duplicate remote path"
                    );
                }
            }
        }
        Ok(out)
    }
    fn get(&mut self, path: &str, etag: &str) -> Result<Vec<u8>> {
        strong(etag)?;
        let response = Self::checked(
            self.request(Method::GET, self.url(path)?)
                .header(IF_MATCH, etag)
                .send()?,
        )?;
        ensure!(
            Self::etag(&response)? == etag,
            "Remote changed while downloading"
        );
        Self::bounded(response, MAX_FILE)
    }
    fn put(&mut self, path: &str, bytes: &[u8], etag: Option<&str>) -> Result<String> {
        ensure!(
            bytes.len() as u64 <= MAX_FILE,
            "Attachment exceeds 128 MiB transfer limit"
        );
        self.parents(path)?;
        let request = self
            .request(Method::PUT, self.url(path)?)
            .header("Content-Type", "application/octet-stream")
            .body(bytes.to_vec());
        let request = if let Some(tag) = etag {
            strong(tag)?;
            request.header(IF_MATCH, tag)
        } else {
            request.header(IF_NONE_MATCH, "*")
        };
        let response = Self::checked(request.send()?)?;
        if let Ok(tag) = Self::etag(&response) {
            return Ok(tag);
        }
        let response = Self::checked(self.request(Method::HEAD, self.url(path)?).send()?)?;
        let tag = Self::etag(&response)?;
        ensure!(
            self.get(path, &tag)? == bytes,
            "Remote changed after upload; repeat synchronization"
        );
        Ok(tag)
    }
    fn delete(&mut self, path: &str, etag: &str) -> Result<()> {
        strong(etag)?;
        Self::checked(
            self.request(Method::DELETE, self.url(path)?)
                .header(IF_MATCH, etag)
                .send()?,
        )?;
        Ok(())
    }
}
type Listing = Vec<(String, bool, String, u64)>;
fn parse_listing(root: &Url, requested: &Url, bytes: &[u8]) -> Result<Listing> {
    let text = std::str::from_utf8(bytes)?;
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 500_000,
            ..Default::default()
        },
    )?;
    let mut out = Vec::new();
    ensure!(
        doc.root_element().has_tag_name(("DAV:", "multistatus")),
        "Invalid WebDAV listing"
    );
    for node in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name(("DAV:", "response")))
    {
        let href = node
            .children()
            .find(|n| n.has_tag_name(("DAV:", "href")))
            .and_then(|n| n.text())
            .context("WebDAV entry lacks a path")?;
        let url = requested.join(href)?;
        ensure!(
            url.origin() == root.origin()
                && url.path().starts_with(root.path())
                && url.query().is_none()
                && url.fragment().is_none(),
            "WebDAV returned a path outside the vault"
        );
        let rel = percent_encoding::percent_decode_str(&url.path()[root.path().len()..])
            .decode_utf8()?
            .into_owned();
        // Reject encoded separators/traversal and unexpected grandchildren in a Depth:1 response.
        let parent = percent_encoding::percent_decode_str(&requested.path()[root.path().len()..])
            .decode_utf8()?
            .into_owned();
        ensure!(
            rel == parent
                || rel.strip_prefix(&parent).is_some_and(|s| s
                    .trim_end_matches('/')
                    .split('/')
                    .count()
                    == 1),
            "Invalid Depth:1 response"
        );
        let propstat = node
            .children()
            .find(|n| {
                n.has_tag_name(("DAV:", "propstat"))
                    && n.children().any(|s| {
                        s.has_tag_name(("DAV:", "status"))
                            && s.text().is_some_and(|s| s.contains(" 200 "))
                    })
            })
            .context("WebDAV entry properties unavailable")?;
        let prop = propstat
            .children()
            .find(|n| n.has_tag_name(("DAV:", "prop")))
            .context("WebDAV property missing")?;
        let collection = prop
            .descendants()
            .any(|n| n.has_tag_name(("DAV:", "collection")));
        let value = |name: &str| {
            prop.children()
                .find(|n| n.has_tag_name(("DAV:", name)))
                .and_then(|n| n.text())
        };
        let etag = value("getetag").unwrap_or_default().to_owned();
        let size = value("getcontentlength").unwrap_or("0").parse()?;
        out.push((rel, collection, etag, size));
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listing_and_transport_boundaries() {
        let root = Url::parse("https://example.org/dav/Notes/").unwrap();
        let xml=br#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/Notes/Caf%C3%A9.md</d:href><d:propstat><d:prop><d:getetag>"v1"</d:getetag><d:getcontentlength>3</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
        let parsed = parse_listing(&root, &root, xml).unwrap();
        assert_eq!(parsed[0].0, "Café.md");
        assert!(endpoint("http://example.org/", false).is_err());
        assert!(endpoint("https://user:secret@example.org/", false).is_err());
        let escape = String::from_utf8(xml.to_vec())
            .unwrap()
            .replace("/dav/Notes/Caf%C3%A9.md", "https://evil.test/a.md");
        assert!(parse_listing(&root, &root, escape.as_bytes()).is_err());
        assert!(strong("W/\"1\"").is_err());
    }
}

#[cfg(test)]
mod http_fixture {
    use super::*;
    use std::net::TcpListener;
    #[test]
    fn real_http_uses_conditional_requests_and_preserves_binary_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut content: Option<Vec<u8>> = None;
            for connection in listener.incoming().take(7) {
                let mut stream = connection.unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut data = Vec::new();
                let header_end = loop {
                    let mut b = [0u8; 1];
                    stream.read_exact(&mut b).unwrap();
                    data.push(b[0]);
                    if data.ends_with(b"\r\n\r\n") {
                        break data.len();
                    }
                };
                let headers = String::from_utf8(data[..header_end].to_vec())
                    .unwrap()
                    .to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .and_then(|l| l.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                assert!(headers.contains("authorization: basic "));
                let method = headers.split_whitespace().next().unwrap();
                let (status, body) = match method {
                    "put" => {
                        let allowed = if content.is_none() {
                            headers.contains("if-none-match: *")
                        } else {
                            headers.contains("if-match: \"v1\"")
                        };
                        if allowed {
                            content = Some(body);
                            ("201 Created", Vec::new())
                        } else {
                            ("412 Precondition Failed", Vec::new())
                        }
                    }
                    "get" => {
                        if headers.contains("if-match: \"v1\"") {
                            ("200 OK", content.clone().unwrap())
                        } else {
                            ("412 Precondition Failed", Vec::new())
                        }
                    }
                    "delete" => {
                        if headers.contains("if-match: \"v1\"") {
                            content = None;
                            ("204 No Content", Vec::new())
                        } else {
                            ("412 Precondition Failed", Vec::new())
                        }
                    }
                    "propfind" => {
                        assert!(headers.contains("depth: 1"));
                        ("207 Multi-Status",format!(r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/vault/Caf%C3%A9.bin</d:href><d:propstat><d:prop><d:getetag>"v1"</d:getetag><d:getcontentlength>{}</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,content.as_ref().unwrap().len()).into_bytes())
                    }
                    _ => panic!("Unexpected request {method}"),
                };
                write!(stream,"HTTP/1.1 {status}\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let config = Config {
            endpoint: format!("http://{address}/vault/"),
            username: "fixture".into(),
            ..Default::default()
        };
        let mut remote = WebDav::build(&config, Secret::new("fixture").unwrap(), true).unwrap();
        let bytes = [0, 255, 42, 13, 10];
        let etag = remote.put("Café.bin", &bytes, None).unwrap();
        assert_eq!(
            remote.list(&AtomicBool::new(false)).unwrap()["Café.bin"].size,
            5
        );
        assert_eq!(remote.get("Café.bin", &etag).unwrap(), bytes);
        assert!(
            remote
                .put("Café.bin", b"overwrite", Some("\"stale\""))
                .is_err()
        );
        assert!(remote.delete("Café.bin", "\"stale\"").is_err());
        assert_eq!(remote.get("Café.bin", &etag).unwrap(), bytes);
        remote.delete("Café.bin", &etag).unwrap();
        server.join().unwrap();
    }
}
