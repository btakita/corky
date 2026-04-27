//! LinkedIn API contract tests with HTTP mocking.

use corky::social::linkedin;

// --- get_user_urn ---

#[test]
fn get_user_urn_success() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/v2/userinfo")
        .match_header("Authorization", "Bearer test-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"sub": "abc123", "name": "Test User"}"#)
        .create();

    let result = linkedin::get_user_urn_at(&server.url(), "test-token");
    mock.assert();
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "urn:li:person:abc123");
}

#[test]
fn get_user_urn_missing_sub() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("GET", "/v2/userinfo")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"name": "Test User"}"#)
        .create();

    let result = linkedin::get_user_urn_at(&server.url(), "test-token");
    mock.assert();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Missing 'sub'"));
}

// --- create_post ---

#[test]
fn create_post_text_only() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/rest/posts")
        .match_header("Authorization", "Bearer test-token")
        .match_header("LinkedIn-Version", "202601")
        .with_status(201)
        .with_header("x-restli-id", "urn:li:share:123456")
        .create();

    let result = linkedin::create_post_at(
        &server.url(),
        "test-token",
        "urn:li:person:abc",
        "Hello world",
        "public",
        &[],
    );
    mock.assert();
    let (post_id, post_url) = result.unwrap();
    assert_eq!(post_id, "urn:li:share:123456");
    assert_eq!(
        post_url,
        "https://www.linkedin.com/feed/update/urn:li:share:123456"
    );
}

#[test]
fn create_post_single_image() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/rest/posts")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"content":{"media":{"id":"urn:li:image:img1"}}}"#.to_string(),
        ))
        .with_status(201)
        .with_header("x-restli-id", "urn:li:share:789")
        .create();

    let images = vec!["urn:li:image:img1".to_string()];
    let result = linkedin::create_post_at(
        &server.url(),
        "test-token",
        "urn:li:person:abc",
        "Post with image",
        "public",
        &images,
    );
    mock.assert();
    assert!(result.is_ok());
}

#[test]
fn create_post_multi_image() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/rest/posts")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"content":{"multiImage":{"images":[{"id":"urn:li:image:a"},{"id":"urn:li:image:b"},{"id":"urn:li:image:c"}]}}}"#.to_string(),
        ))
        .with_status(201)
        .with_header("x-restli-id", "urn:li:share:multi")
        .create();

    let images = vec![
        "urn:li:image:a".to_string(),
        "urn:li:image:b".to_string(),
        "urn:li:image:c".to_string(),
    ];
    let result = linkedin::create_post_at(
        &server.url(),
        "test-token",
        "urn:li:person:abc",
        "Multi image post",
        "public",
        &images,
    );
    mock.assert();
    assert!(result.is_ok());
}

#[test]
fn create_post_body_too_long() {
    // No mock needed — validation happens before HTTP
    let long_body = "x".repeat(3001);
    let result = linkedin::create_post_at(
        "http://unused",
        "test-token",
        "urn:li:person:abc",
        &long_body,
        "public",
        &[],
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("character limit"));
}

#[test]
fn create_post_api_error() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/rest/posts")
        .with_status(403)
        .with_body(r#"{"message":"Insufficient permissions","status":403}"#)
        .create();

    let result = linkedin::create_post_at(
        &server.url(),
        "test-token",
        "urn:li:person:abc",
        "Test",
        "public",
        &[],
    );
    mock.assert();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("LinkedIn API error (HTTP 403)"));
    assert!(err.contains("Insufficient permissions"));
}

// --- upload_image ---

#[test]
fn upload_image_success() {
    let mut server = mockito::Server::new();

    // The upload URL will point back to the mock server
    let upload_path = "/upload/image/binary";
    let upload_url = format!("{}{}", server.url(), upload_path);

    // Step 1: Initialize upload mock
    let init_mock = server
        .mock("POST", "/rest/images?action=initializeUpload")
        .match_header("Authorization", "Bearer test-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "value": {
                    "uploadUrl": upload_url,
                    "image": "urn:li:image:uploaded123"
                }
            })
            .to_string(),
        )
        .create();

    // Step 2: Binary upload mock
    let upload_mock = server
        .mock("PUT", upload_path)
        .match_header("Content-Type", "application/octet-stream")
        .with_status(201)
        .create();

    let result = linkedin::upload_image_at(
        &server.url(),
        "test-token",
        "urn:li:person:abc",
        b"fake image data",
    );
    init_mock.assert();
    upload_mock.assert();
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "urn:li:image:uploaded123");
}

#[test]
fn upload_image_init_failure() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/rest/images?action=initializeUpload")
        .with_status(401)
        .with_body(r#"{"message":"Unauthorized"}"#)
        .create();

    let result = linkedin::upload_image_at(
        &server.url(),
        "bad-token",
        "urn:li:person:abc",
        b"image data",
    );
    mock.assert();
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("image init failed")
    );
}

// --- map_visibility ---

#[test]
fn visibility_valid_values() {
    assert_eq!(linkedin::map_visibility("public").unwrap(), "PUBLIC");
    assert_eq!(
        linkedin::map_visibility("connections").unwrap(),
        "CONNECTIONS"
    );
    assert_eq!(linkedin::map_visibility("Public").unwrap(), "PUBLIC");
    assert_eq!(
        linkedin::map_visibility("CONNECTIONS").unwrap(),
        "CONNECTIONS"
    );
}

#[test]
fn visibility_invalid() {
    let result = linkedin::map_visibility("private");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Invalid LinkedIn visibility")
    );
}
