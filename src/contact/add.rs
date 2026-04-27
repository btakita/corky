//! Add a new contact: scaffold mail/contacts/{name}/ with AGENTS.md.

use anyhow::Result;

use crate::config::contact::{Contact, load_contacts, save_contact};
use crate::resolve;

/// Default AGENTS.md template for a new contact.
pub fn default_agents_md(name: &str) -> String {
    format!(
        r#"# Contact: {name}

Context for drafting emails to or about {name}.

## Relationship

<!-- How you know this person, what they work on, shared history -->

## Formality

professional

<!-- Options: casual, professional, formal.
     Applies to group emails — use maximum formality across all recipients. -->

## Tone

<!-- Communication style for {name}.
     Defaults to voice.md; note overrides here.
     e.g. "More formal than usual" or "Very casual, first-name basis" -->

## Topics

<!-- Recurring subjects, current projects, things to reference or avoid -->

## Research

<!-- Where to look for more details about this contact. -->

## Notes

<!-- Freeform: quirks, preferences, pending items, important dates -->
"#,
        name = name,
    )
}

/// Enriched AGENTS.md pre-filled from conversation data.
pub fn enriched_agents_md(
    name: &str,
    topics: &[String],
    other_participants: &[String],
    email_domain: Option<&str>,
) -> String {
    let topics_section = if topics.is_empty() {
        "<!-- Recurring subjects, current projects, things to reference or avoid -->".to_string()
    } else {
        let lines: Vec<String> = topics
            .iter()
            .map(|t| format!("- {} (from conversation)", t))
            .collect();
        lines.join("\n")
    };

    let shared_hint = if other_participants.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n     Shared threads with: {}
     Note any tone adjustments when these people are also on the thread.",
            other_participants.join(", ")
        )
    };

    let research_section = if let Some(domain) = email_domain {
        format!(
            "<!-- Where to look for more details about this contact.\n     Email domain derived from contact's email address. -->\n- Email domain: {}",
            domain
        )
    } else {
        "<!-- Where to look for more details about this contact. -->".to_string()
    };

    format!(
        r#"# Contact: {name}

Context for drafting emails to or about {name}.

## Relationship

<!-- How you know this person, what they work on, shared history -->

## Formality

professional

<!-- Options: casual, professional, formal.
     Applies to group emails — use maximum formality across all recipients. -->

## Tone

<!-- Communication style for {name}.
     Defaults to voice.md; note overrides here.{shared_hint} -->

## Topics

{topics_section}

## Research

{research_section}

## Notes

<!-- Freeform: quirks, preferences, pending items, important dates -->
"#,
        name = name,
        shared_hint = shared_hint,
        topics_section = topics_section,
        research_section = research_section,
    )
}

/// Create a contact with the default AGENTS.md template.
pub fn run(name: &str, emails: &[String]) -> Result<()> {
    run_with_agents_md(name, emails, &default_agents_md(name))
}

/// Create a contact with custom AGENTS.md content (used by from_conversation).
pub fn run_with_agents_md(name: &str, emails: &[String], agents_md_content: &str) -> Result<()> {
    // Check not already configured
    let contacts = load_contacts(None)?;
    if contacts.contains_key(name) {
        anyhow::bail!("Contact '{}' already exists in .corky.toml", name);
    }

    let contact_dir = resolve::contacts_dir().join(name);
    if contact_dir.exists() {
        anyhow::bail!("Directory {} already exists", contact_dir.display());
    }

    // 1. Create contact directory with AGENTS.md + CLAUDE.md symlink
    std::fs::create_dir_all(&contact_dir)?;
    let agents_md = contact_dir.join("AGENTS.md");
    std::fs::write(&agents_md, agents_md_content)?;

    #[cfg(unix)]
    std::os::unix::fs::symlink("AGENTS.md", contact_dir.join("CLAUDE.md"))?;

    println!("Created {}/AGENTS.md", contact_dir.display());

    // 2. Update .corky.toml
    let contact = Contact {
        emails: emails.to_vec(),
        ..Default::default()
    };
    save_contact(name, &contact, None)?;
    println!("Updated .corky.toml");

    // 3. Next steps
    println!();
    println!("Done! Next steps:");
    println!(
        "  - Edit {}/AGENTS.md with relationship context",
        contact_dir.display()
    );

    Ok(())
}
