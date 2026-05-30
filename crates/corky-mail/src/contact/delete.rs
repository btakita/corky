use anyhow::{Result, bail};

use super::push::get_access_token;

/// Delete a contact from Google Contacts by resource name (e.g. "people/c5045079896432711148").
pub fn delete_google_contact(resource_name: &str) -> Result<()> {
    let token = get_access_token()?;

    let url = format!(
        "https://people.googleapis.com/v1/{}:deleteContact",
        resource_name
    );

    match ureq::delete(&url)
        .set("Authorization", &format!("Bearer {}", token))
        .call()
    {
        Ok(_) => {
            println!("Deleted: {}", resource_name);
            Ok(())
        }
        Err(ureq::Error::Status(status, resp)) => {
            let err_body = resp.into_string().unwrap_or_default();
            bail!("Delete contact failed (HTTP {}): {}", status, err_body);
        }
        Err(e) => Err(e.into()),
    }
}

/// Run the delete subcommand: delete one or more contacts from Google by resource name.
pub fn run(resource_names: &[String]) -> Result<()> {
    if resource_names.is_empty() {
        bail!("No resource names provided. Usage: corky contact delete <RESOURCE_NAME>...");
    }

    for name in resource_names {
        // Accept both "people/c123" and "c123" formats
        let resource = if name.starts_with("people/") {
            name.clone()
        } else {
            format!("people/{}", name)
        };
        delete_google_contact(&resource)?;
    }

    Ok(())
}
