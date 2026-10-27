//! Glimpse API v1; only an explicitly enabled read-only Notes export is visible.
use anyhow::Result;
use glimpse_plugin_sdk::{Action, Hit, serve};
use notes_vault::integrations::{self, LauncherAction};
use serde_json::{Value, json};
use std::path::PathBuf;
fn search(query: &str, db: Option<PathBuf>) -> Result<Value> {
    let Some(query) = query
        .strip_prefix("note")
        .filter(|s| s.is_empty() || s.starts_with(char::is_whitespace))
    else {
        return Ok(json!([]));
    };
    let query = query.trim();
    let Some(db) = db else {
        return Ok(json!([Hit{id:"setup".into(),title:"Enable the Notes integration".into(),subtitle:"Enable the local Glimpse export in Notes and update Glimpse for Notes actions.".into(),action:Action::Copy{text:"Notes → File → Suite integration preferences → Export visited vaults to Glimpse".into()}}]));
    };
    let vaults = integrations::launcher_vaults(&db)?;
    if vaults.is_empty() {
        return Ok(json!([]));
    }
    if let Some(title) = query.strip_prefix("new ") {
        let title = title.trim();
        let mut hits = Vec::new();
        for root in vaults {
            let request = LauncherAction {
                api_version: 1,
                op: "create".into(),
                vault: root.clone(),
                path: None,
                title: Some(title.into()),
                offset: None,
                append: false,
            };
            request.validate()?;
            hits.push(Hit {
                id: format!("new:{}", root.display()),
                title: format!("Create {title}"),
                subtitle: root.display().to_string(),
                action: Action::Activate {
                    token: serde_json::to_string(&request)?,
                },
            });
        }
        return Ok(serde_json::to_value(hits)?);
    }
    let (query, append, headings) = if let Some(query) = query.strip_prefix("append ") {
        (query, true, false)
    } else if let Some(query) = query.strip_prefix("heading ") {
        (query, false, true)
    } else {
        (query, false, false)
    };
    let hits = integrations::launcher_search(&db, query, headings)?
        .into_iter()
        .map(|hit| {
            let action = LauncherAction {
                api_version: 1,
                op: "open".into(),
                vault: hit.vault,
                path: Some(hit.path.clone()),
                title: None,
                offset: hit.offset,
                append,
            };
            Ok(Hit {
                id: format!("{}:{}", hit.path.display(), hit.offset.unwrap_or(0)),
                title: if append {
                    format!("Append to {}", hit.title)
                } else {
                    hit.title
                },
                subtitle: hit.snippet.chars().take(180).collect(),
                action: Action::Activate {
                    token: serde_json::to_string(&action)?,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_value(hits)?)
}
fn main() {
    if let Err(error) = serve(&["search", "commands"], |request| {
        match request.op.as_str() {
            "search" => search(
                request.args["query"].as_str().unwrap_or_default(),
                std::env::var_os("NYX_NOTES_INDEX").map(PathBuf::from),
            ),
            "activate" => anyhow::bail!("Update Glimpse to use the Notes host-action integration"),
            _ => Ok(Value::Null),
        }
    }) {
        eprintln!("Notes plugin: {error:#}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_queries_do_not_read_or_act() {
        assert_eq!(search("weather rust", None).unwrap(), json!([]));
        assert_eq!(search("notebook", None).unwrap(), json!([]));
        let response = search("note rust", None).unwrap();
        assert_eq!(response.as_array().unwrap().len(), 1);
        assert_eq!(response[0]["action"]["kind"], "copy");
    }
}
