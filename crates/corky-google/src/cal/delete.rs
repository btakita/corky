//! Delete Google Calendar events by search query.

use anyhow::{Result, bail};
use std::collections::HashSet;

use super::auth;
use super::list;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

fn api_delete(token: &str, url: &str) -> Result<()> {
    match ureq::delete(url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(401, _)) => {
            bail!(
                "Calendar API returned 401 Unauthorized.\n\
                 Try re-authenticating with: corky cal auth"
            );
        }
        Err(ureq::Error::Status(404, _)) => {
            // Already deleted (e.g. series deletion removed all instances)
            Ok(())
        }
        Err(ureq::Error::Status(410, _)) => {
            // Gone — already deleted
            Ok(())
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Calendar API error (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Delete events matching a query.
///
/// With `--all`, deletes the entire recurring series (not just visible instances).
/// This works by deleting the series root event using the `recurring_event_id`.
pub fn run(query: &str, all_events: bool, dry_run: bool, account: Option<&str>) -> Result<()> {
    let token = auth::get_access_token(account)?;
    let events = list::fetch_events(&token, Some(query), 25)?;

    if events.is_empty() {
        println!("No events matching '{}'.", query);
        return Ok(());
    }

    // With --all, collect unique series IDs to delete the series root
    if all_events {
        let mut series_ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for event in &events {
            let series_id = event
                .recurring_event_id
                .as_deref()
                .unwrap_or(&event.id)
                .to_string();
            if seen.insert(series_id.clone()) {
                series_ids.push(series_id);
            }
        }

        println!(
            "Found {} event(s) matching '{}' across {} series:",
            events.len(),
            query,
            series_ids.len()
        );
        for event in &events {
            println!("  {} | {}", event.start.display(), event.summary);
        }

        if dry_run {
            println!("\n(dry run — would delete {} series)", series_ids.len());
            return Ok(());
        }

        let mut deleted = 0;
        for series_id in &series_ids {
            let url = format!("{}/calendars/primary/events/{}", CALENDAR_API, series_id,);
            match api_delete(&token, &url) {
                Ok(()) => {
                    deleted += 1;
                    println!("  Deleted series: {}", series_id);
                }
                Err(e) => {
                    eprintln!("  Failed to delete series '{}': {}", series_id, e);
                }
            }
        }
        println!("\nDeleted {} recurring series.", deleted);
    } else {
        println!("Found {} event(s) matching '{}':", events.len(), query);
        for event in &events {
            println!("  {} | {}", event.start.display(), event.summary);
            if let Some(ref rid) = event.recurring_event_id {
                println!("    (recurring series: {})", rid);
            }
        }

        if dry_run {
            println!("\n(dry run — no events deleted)");
            return Ok(());
        }

        let mut deleted = 0;
        for event in &events {
            let url = format!("{}/calendars/primary/events/{}", CALENDAR_API, event.id,);
            match api_delete(&token, &url) {
                Ok(()) => {
                    deleted += 1;
                    println!("  Deleted: {}", event.summary);
                }
                Err(e) => {
                    eprintln!("  Failed to delete '{}': {}", event.summary, e);
                }
            }
        }
        println!("\nDeleted {} event(s).", deleted);
    }

    Ok(())
}
