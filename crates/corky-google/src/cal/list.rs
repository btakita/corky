//! List upcoming Google Calendar events.

use anyhow::{Result, bail};
use chrono::Utc;

use super::auth;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

#[derive(Debug, serde::Deserialize)]
struct EventListResponse {
    #[serde(default)]
    items: Vec<CalendarEvent>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub start: EventDateTime,
    #[serde(default)]
    pub end: EventDateTime,
    #[serde(default)]
    pub recurrence: Vec<String>,
    #[serde(default)]
    pub recurring_event_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub html_link: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDateTime {
    #[serde(default)]
    pub date_time: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub time_zone: Option<String>,
}

impl EventDateTime {
    pub fn display(&self) -> String {
        if let Some(ref dt) = self.date_time {
            // Parse ISO 8601 and format nicely
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(dt) {
                return parsed.format("%a %b %d, %Y %I:%M %p %Z").to_string();
            }
            return dt.clone();
        }
        if let Some(ref d) = self.date {
            return d.clone();
        }
        "(no time)".to_string()
    }
}

fn api_get(token: &str, url: &str) -> Result<ureq::Response> {
    match ureq::get(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(401, _)) => {
            let _ = auth::clear_access_token(token);
            bail!(
                "Calendar API returned 401 Unauthorized. Cleared the cached token.\n\
                 Try re-authenticating with: corky cal auth"
            );
        }
        Err(ureq::Error::Status(403, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Calendar API returned 403 Forbidden: {}", err_body);
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Calendar API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Fetch upcoming events, optionally filtering by query string.
pub fn fetch_events(token: &str, query: Option<&str>, limit: usize) -> Result<Vec<CalendarEvent>> {
    let now = Utc::now().to_rfc3339();
    let mut url = format!(
        "{}/calendars/primary/events?maxResults={}&orderBy=startTime&singleEvents=true&timeMin={}",
        CALENDAR_API,
        limit,
        crate::cal::auth::urlencode_pub(&now),
    );
    if let Some(q) = query {
        url.push_str(&format!("&q={}", crate::cal::auth::urlencode_pub(q)));
    }
    let resp = api_get(token, &url)?;
    let list: EventListResponse = resp.into_json()?;
    Ok(list.items)
}

/// List upcoming events to stdout.
pub fn run(limit: usize, query: Option<&str>, account: Option<&str>) -> Result<()> {
    let token = auth::get_access_token(account)?;
    let events = fetch_events(&token, query, limit)?;
    if events.is_empty() {
        if let Some(q) = query {
            println!("No upcoming events matching '{}'.", q);
        } else {
            println!("No upcoming events.");
        }
        return Ok(());
    }
    for event in &events {
        println!(
            "  {} | {} | id:{}",
            event.start.display(),
            event.summary,
            event.id,
        );
    }
    println!("\n{} event(s).", events.len());
    Ok(())
}
