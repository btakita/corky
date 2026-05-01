//! Check calendar availability for a time range.

use anyhow::{Result, bail};
use chrono::{DateTime, FixedOffset};

use super::auth;
use super::list;

/// Check availability in a time range.
///
/// Lists events between `start` and `end` (RFC 3339), then reports
/// busy periods and total free time.
pub fn run(start: &str, end: &str, account: Option<&str>) -> Result<()> {
    let start_dt = DateTime::parse_from_rfc3339(start).map_err(|_| {
        anyhow::anyhow!(
            "Invalid start time '{}'. Use RFC 3339 format (e.g. 2026-03-10T09:00:00-05:00)",
            start
        )
    })?;
    let end_dt = DateTime::parse_from_rfc3339(end).map_err(|_| {
        anyhow::anyhow!(
            "Invalid end time '{}'. Use RFC 3339 format (e.g. 2026-03-10T17:00:00-05:00)",
            end
        )
    })?;

    if end_dt <= start_dt {
        bail!("End time must be after start time.");
    }

    let token = auth::get_access_token(account)?;
    let events = fetch_events_in_range(&token, start, end)?;

    let total_minutes = (end_dt - start_dt).num_minutes();

    if events.is_empty() {
        println!(
            "Free: {} — {} ({} hours)",
            start_dt.format("%a %b %d %I:%M %p"),
            end_dt.format("%I:%M %p %Z"),
            total_minutes / 60,
        );
        println!("\nNo events in this range. Fully available.");
        return Ok(());
    }

    println!(
        "Checking: {} — {}",
        start_dt.format("%a %b %d %I:%M %p"),
        end_dt.format("%I:%M %p %Z"),
    );
    println!();

    let mut busy_minutes: i64 = 0;

    println!("Busy:");
    for event in &events {
        let (ev_start, ev_end) = parse_event_times(event, &start_dt, &end_dt);
        let duration = (ev_end - ev_start).num_minutes();
        busy_minutes += duration;
        println!(
            "  {} — {} | {} ({}m)",
            ev_start.format("%I:%M %p"),
            ev_end.format("%I:%M %p"),
            event.summary,
            duration,
        );
    }

    let free_minutes = total_minutes - busy_minutes;
    println!();
    println!(
        "Summary: {} busy, {} free out of {} total",
        format_duration(busy_minutes),
        format_duration(free_minutes.max(0)),
        format_duration(total_minutes),
    );

    Ok(())
}

/// Fetch events within a specific time range.
fn fetch_events_in_range(
    token: &str,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<list::CalendarEvent>> {
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/primary/events\
         ?maxResults=50\
         &orderBy=startTime\
         &singleEvents=true\
         &timeMin={}\
         &timeMax={}",
        auth::urlencode_pub(time_min),
        auth::urlencode_pub(time_max),
    );

    let resp = match ureq::get(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Calendar API returned 401 Unauthorized.\n\
                 Try re-authenticating with: corky cal auth"
            );
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Calendar API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => return Err(e.into()),
    };

    #[derive(serde::Deserialize)]
    struct EventListResponse {
        #[serde(default)]
        items: Vec<list::CalendarEvent>,
    }

    let list: EventListResponse = resp.into_json()?;
    Ok(list.items)
}

/// Extract start/end times from an event, clamped to the check range.
fn parse_event_times(
    event: &list::CalendarEvent,
    range_start: &DateTime<FixedOffset>,
    range_end: &DateTime<FixedOffset>,
) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
    let ev_start = event
        .start
        .date_time
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .unwrap_or(*range_start);

    let ev_end = event
        .end
        .date_time
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .unwrap_or(*range_end);

    // Clamp to check range
    let clamped_start = ev_start.max(*range_start);
    let clamped_end = ev_end.min(*range_end);

    (clamped_start, clamped_end)
}

fn format_duration(minutes: i64) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    if h > 0 && m > 0 {
        format!("{}h {}m", h, m)
    } else if h > 0 {
        format!("{}h", h)
    } else {
        format!("{}m", m)
    }
}
