//! `corky tasks add` — add a task to Google Tasks.

use anyhow::{bail, Result};

use crate::filter::gmail_auth;

const TASKS_API: &str = "https://tasks.googleapis.com/tasks/v1";

/// Add a new task to a tasklist.
///
/// `due` should be an RFC 3339 date string (e.g. `2026-04-15`) — the time portion
/// is ignored by the Tasks API; only the date is stored.
pub fn run(title: &str, due: Option<&str>, tasklist: Option<&str>, account: Option<&str>) -> Result<()> {
    let list_id = tasklist.unwrap_or("@default");
    let token = gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::TASKS_SCOPE,
        account,
    )?;

    let mut task = serde_json::json!({ "title": title });
    if let Some(d) = due {
        // Tasks API expects RFC 3339 with time component; add T00:00:00.000Z if bare date
        let due_rfc = if d.contains('T') {
            d.to_string()
        } else {
            format!("{}T00:00:00.000Z", d)
        };
        task["due"] = serde_json::Value::String(due_rfc);
    }

    let url = format!("{}/lists/{}/tasks", TASKS_API, list_id);

    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&task);

    match resp {
        Ok(r) => {
            let result: serde_json::Value = r.into_json()?;
            let id = result["id"].as_str().unwrap_or("(unknown)");
            let created_title = result["title"].as_str().unwrap_or(title);
            println!("Task created: [{}] {}", id, created_title);
            Ok(())
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!("Tasks API: unauthorized (401). Re-run `corky filter auth`.")
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Tasks API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}
