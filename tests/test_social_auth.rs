//! OAuth auth flow edge cases (A1–A5).

mod common;

use corky::social::auth;
use corky::social::platform::Platform;

// A1: Correct auth URL generation
// Note: requires credentials set up, so we test URL building components.
// auth::build_auth_url needs credentials — we test parse_callback which is pure.

// A2: Valid callback parse
#[test]
fn a2_valid_callback_parse() {
    let query = "code=AUTH_CODE_123&state=STATE_ABC";
    let (code, state) = auth::parse_callback(query).unwrap();
    assert_eq!(code, "AUTH_CODE_123");
    assert_eq!(state, "STATE_ABC");
}

#[test]
fn callback_parse_decodes_form_encoded_values() {
    let query = "code=4%2F0AZEOvh%2Babc%3D&state=STATE%2BABC%2F123";
    let (code, state) = auth::parse_callback(query).unwrap();
    assert_eq!(code, "4/0AZEOvh+abc=");
    assert_eq!(state, "STATE+ABC/123");
}

// A3: Callback missing code
#[test]
fn a3_callback_missing_code() {
    let query = "state=STATE_ABC";
    let result = auth::parse_callback(query);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("code"),
        "Expected missing code error, got: {}",
        err
    );
}

// A4: State mismatch (tested at the caller level, but we test callback parsing)
#[test]
fn a4_state_present_in_parse() {
    let query = "code=AUTH_CODE&state=WRONG_STATE";
    let (_, state) = auth::parse_callback(query).unwrap();
    // The state mismatch check happens in auth::run, but we verify parsing extracts it
    assert_eq!(state, "WRONG_STATE");
}

// A5: Callback with error param (user denied)
#[test]
fn a5_callback_with_error() {
    let query = "error=user_cancelled_authorize&error_description=The+user+denied+the+request";
    let result = auth::parse_callback(query);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("user_cancelled_authorize") || err.contains("denied"),
        "Expected user denied error, got: {}",
        err
    );
}

#[test]
fn callback_error_description_is_url_decoded() {
    let query = "error=invalid_request&error_description=Bad%2Bstate+value";
    let result = auth::parse_callback(query);
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().to_string(),
        "OAuth error: invalid_request: Bad+state value"
    );
}

// Additional: callback with both code and error (error wins)
#[test]
fn callback_error_takes_precedence() {
    let query = "code=SOME_CODE&error=access_denied&state=STATE";
    let result = auth::parse_callback(query);
    assert!(result.is_err());
}

// Additional: empty query string
#[test]
fn callback_empty_query() {
    let result = auth::parse_callback("");
    assert!(result.is_err());
}

// Additional: platform FromStr
#[test]
fn platform_from_str() {
    assert_eq!("linkedin".parse::<Platform>().unwrap(), Platform::LinkedIn);
    assert_eq!("LinkedIn".parse::<Platform>().unwrap(), Platform::LinkedIn);
    assert_eq!("LINKEDIN".parse::<Platform>().unwrap(), Platform::LinkedIn);
    assert!("myspace".parse::<Platform>().is_err());
}

// Additional: platform Display
#[test]
fn platform_display() {
    assert_eq!(Platform::LinkedIn.to_string(), "linkedin");
    assert_eq!(Platform::Bluesky.to_string(), "bluesky");
    assert_eq!(Platform::Mastodon.to_string(), "mastodon");
    assert_eq!(Platform::Twitter.to_string(), "twitter");
}
