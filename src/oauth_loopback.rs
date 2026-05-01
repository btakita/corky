use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::io::ErrorKind;
use std::net::TcpListener;
use tiny_http::{Request, Response, Server};

const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PATH: &str = "/callback";
const DEFAULT_CALLBACK_PORT: u16 = 8484;
pub(crate) const CALLBACK_PORT_ENV: &str = "CORKY_OAUTH_CALLBACK_PORT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortMode {
    FixedOnly,
    EphemeralFallback,
}

pub(crate) struct LoopbackServer {
    server: Server,
    redirect_uri: String,
    port: u16,
}

pub(crate) struct OAuthCallback {
    request: Request,
    pub(crate) code: String,
    pub(crate) state: String,
}

impl OAuthCallback {
    pub(crate) fn respond_text(self, body: &str) {
        let _ = self.request.respond(Response::from_string(body));
    }
}

impl LoopbackServer {
    pub(crate) fn bind(provider_name: &str, port_mode: PortMode) -> Result<Self> {
        let requested_port = requested_port_from_env()?;
        let server = bind_with_settings(
            provider_name,
            requested_port,
            DEFAULT_CALLBACK_PORT,
            port_mode,
        )?;
        if requested_port.is_none() && server.port != DEFAULT_CALLBACK_PORT {
            eprintln!(
                "OAuth callback port {} is busy; using {} instead. Set {} to pin a port for this session.",
                DEFAULT_CALLBACK_PORT, server.port, CALLBACK_PORT_ENV
            );
        }
        Ok(server)
    }

    pub(crate) fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub(crate) fn recv_callback(self, timeout_secs: u64) -> Result<OAuthCallback> {
        let request = self
            .server
            .recv_timeout(std::time::Duration::from_secs(timeout_secs))
            .map_err(|e| anyhow!("Callback server error: {}", e))?
            .ok_or_else(|| anyhow!("Timed out waiting for OAuth callback ({}s)", timeout_secs))?;
        let url = request.url().to_string();
        let query = url.split('?').nth(1).unwrap_or("");
        let (code, state) = crate::social::auth::parse_callback(query)?;
        Ok(OAuthCallback {
            request,
            code,
            state,
        })
    }
}

fn requested_port_from_env() -> Result<Option<u16>> {
    match env::var(CALLBACK_PORT_ENV) {
        Ok(raw) => Ok(Some(parse_callback_port(&raw)?)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            bail!("{} must be valid UTF-8 if set", CALLBACK_PORT_ENV)
        }
    }
}

fn parse_callback_port(raw: &str) -> Result<u16> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("{} must not be empty", CALLBACK_PORT_ENV);
    }
    let port = trimmed.parse::<u16>().with_context(|| {
        format!(
            "{} must be an integer between 1 and 65535, got '{}'",
            CALLBACK_PORT_ENV, raw
        )
    })?;
    if port == 0 {
        bail!("{} must be between 1 and 65535", CALLBACK_PORT_ENV);
    }
    Ok(port)
}

fn bind_with_settings(
    provider_name: &str,
    requested_port: Option<u16>,
    default_port: u16,
    port_mode: PortMode,
) -> Result<LoopbackServer> {
    let desired_port = requested_port.unwrap_or(default_port);
    match bind_server(desired_port) {
        Ok(server) => Ok(server),
        Err(err)
            if requested_port.is_none()
                && matches!(port_mode, PortMode::EphemeralFallback)
                && err.kind() == ErrorKind::AddrInUse =>
        {
            bind_server(0).with_context(|| {
                format!(
                    "Failed to bind a fallback OAuth callback listener for {} after {}:{} was already in use",
                    provider_name, CALLBACK_HOST, desired_port
                )
            })
        }
        Err(err) => Err(format_bind_error(
            provider_name,
            desired_port,
            requested_port.is_some(),
            err,
        )),
    }
}

fn bind_server(port: u16) -> std::io::Result<LoopbackServer> {
    let listener = TcpListener::bind((CALLBACK_HOST, port))?;
    let addr = listener.local_addr()?;
    let server = Server::from_listener(listener, None)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(LoopbackServer {
        server,
        redirect_uri: format!("http://{}:{}{}", CALLBACK_HOST, addr.port(), CALLBACK_PATH),
        port: addr.port(),
    })
}

fn format_bind_error(
    provider_name: &str,
    port: u16,
    from_env: bool,
    err: std::io::Error,
) -> anyhow::Error {
    if from_env {
        anyhow!(
            "Failed to start the {} OAuth callback listener on {}:{}: {}.\nFree that port or rerun with a different {} value.",
            provider_name,
            CALLBACK_HOST,
            port,
            err,
            CALLBACK_PORT_ENV
        )
    } else {
        anyhow!(
            "Failed to start the {} OAuth callback listener on {}:{}: {}.\nFree that port or rerun with {}=<port> for this session.",
            provider_name,
            CALLBACK_HOST,
            port,
            err,
            CALLBACK_PORT_ENV
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parse_callback_port_accepts_valid_integer() {
        assert_eq!(parse_callback_port("8484").unwrap(), 8484);
    }

    #[test]
    fn parse_callback_port_rejects_zero() {
        let err = parse_callback_port("0").unwrap_err().to_string();
        assert!(err.contains("1 and 65535"));
    }

    #[test]
    fn bind_falls_back_to_ephemeral_port_when_default_is_busy() {
        let busy_port = reserve_free_port();
        let hold = TcpListener::bind((CALLBACK_HOST, busy_port)).unwrap();
        let server =
            bind_with_settings("Google", None, busy_port, PortMode::EphemeralFallback).unwrap();
        assert_ne!(server.port, busy_port);
        assert!(
            server
                .redirect_uri()
                .starts_with(&format!("http://{}:", CALLBACK_HOST))
        );
        drop(hold);
    }

    #[test]
    fn bind_fixed_mode_returns_actionable_busy_port_error() {
        let busy_port = reserve_free_port();
        let hold = TcpListener::bind((CALLBACK_HOST, busy_port)).unwrap();
        let result = bind_with_settings("LinkedIn", None, busy_port, PortMode::FixedOnly);
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("LinkedIn"));
        assert!(err.contains(CALLBACK_PORT_ENV));
        drop(hold);
    }

    fn reserve_free_port() -> u16 {
        TcpListener::bind((CALLBACK_HOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
}
