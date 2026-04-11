//! `corky tasks list` — list Google Tasks.

use anyhow::{bail, Result};

use crate::filter::gmail_auth;

const TASKS_API: &str = "https://tasks.googleapis.com/tasks/v1";

/// List tasks from a tasklist (default: `@default`).
pub fn run(tasklist: Option<&str>, account: Option<&str>) -> Result<()> {
    let list_id = tasklist.unwrap_or("@default");
    let token = gmail_auth::get_access_token_for_user(
        Some("default"),
        gmail_auth::TASKS_SCOPE,
        account,
    )?;

    let url = format!(
        "{}/lists/{}/tasks?showCompleted=false",
        TASKS_API, list_id
    );

    let resp = ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .call();

    match resp {
        Ok(r) => {
            let data: serde_json::Value = r.into_json()?;
            let items = data["items"].as_array();
            match items {
                None => {
                    println!("No pending tasks.");
                }
                Some(tasks) if tasks.is_empty() => {
                    println!("No pending tasks.");
                }
                Some(tasks) => {
                    for task in tasks {
                        let id = task["id"].as_str().unwrap_or("?");
                        let title = task["title"].as_str().unwrap_or("(no title)");
                        let due = task["due"].as_str().unwrap_or("");
                        if due.is_empty() {
                            println!("[{}] {}", id, title);
                        } else {
                            println!("[{}] {} (due: {})", id, title, &due[..10]);
                        }
                    }
                }
            }
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
