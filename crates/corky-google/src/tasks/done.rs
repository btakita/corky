//! `corky tasks done` — mark a Google Task as completed.

use anyhow::{Result, bail};

use crate::filter::gmail_auth;

const TASKS_API: &str = "https://tasks.googleapis.com/tasks/v1";

/// Mark a task as completed via PATCH.
pub fn run(task_id: &str, tasklist: Option<&str>, account: Option<&str>) -> Result<()> {
    let list_id = tasklist.unwrap_or("@default");
    let token =
        gmail_auth::get_access_token_for_user(Some("default"), gmail_auth::TASKS_SCOPE, account)?;

    let url = format!("{}/lists/{}/tasks/{}", TASKS_API, list_id, task_id);
    let body = serde_json::json!({ "status": "completed" });

    let resp = ureq::request("PATCH", &url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(&body);

    match resp {
        Ok(_) => {
            println!("Task {} marked as completed.", task_id);
            Ok(())
        }
        Err(ureq::Error::Status(401, _)) => {
            bail!("Tasks API: unauthorized (401). Re-run `corky filter auth`.")
        }
        Err(ureq::Error::Status(404, _)) => {
            bail!("Task not found: {}", task_id)
        }
        Err(ureq::Error::Status(status, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            bail!("Tasks API error (HTTP {}): {}", status, body);
        }
        Err(e) => Err(e.into()),
    }
}
