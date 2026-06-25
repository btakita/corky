//! Create Google Calendar events.

use anyhow::{Result, bail};

use super::auth;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateEventRequest {
    summary: String,
    start: EventDateTimeRequest,
    end: EventDateTimeRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct EventDateTimeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    date_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
}

#[derive(serde::Deserialize)]
struct CreateEventResponse {
    id: String,
    #[serde(default)]
    html_link: String,
}

/// Create a calendar event.
///
/// `start` and `end` accept RFC 3339 datetime (e.g. `2026-03-10T14:00:00-05:00`)
/// or YYYY-MM-DD for all-day events.
pub fn run(
    summary: &str,
    start: &str,
    end: &str,
    description: Option<&str>,
    location: Option<&str>,
    account: Option<&str>,
) -> Result<()> {
    let token = auth::get_access_token(account)?;

    let start_dt = parse_datetime(start)?;
    let end_dt = parse_datetime(end)?;

    let request = CreateEventRequest {
        summary: summary.to_string(),
        start: start_dt,
        end: end_dt,
        description: description.map(|s| s.to_string()),
        location: location.map(|s| s.to_string()),
    };

    let url = format!("{}/calendars/primary/events", CALENDAR_API);
    let json_value = serde_json::to_value(&request)?;

    let resp = match ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_json(json_value)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            let _ = auth::clear_access_token(&token);
            bail!(
                "Calendar API returned 401 Unauthorized. Cleared the cached token.\n\
                 Try re-authenticating with: corky cal auth"
            );
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Calendar API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    let created: CreateEventResponse = resp.into_json()?;
    println!("Created: {}", summary);
    println!("  ID: {}", created.id);
    if !created.html_link.is_empty() {
        println!("  Link: {}", created.html_link);
    }

    Ok(())
}

/// Parse a datetime string as either RFC 3339 or YYYY-MM-DD (all-day).
fn parse_datetime(s: &str) -> Result<EventDateTimeRequest> {
    // Try RFC 3339 first
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return Ok(EventDateTimeRequest {
            date_time: Some(s.to_string()),
            date: None,
        });
    }
    // Try YYYY-MM-DD
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return Ok(EventDateTimeRequest {
            date_time: None,
            date: Some(s.to_string()),
        });
    }
    bail!(
        "Invalid datetime '{}'. Use RFC 3339 (e.g. 2026-03-10T14:00:00-05:00) \
         or YYYY-MM-DD for all-day events.",
        s
    )
}
