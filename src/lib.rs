pub mod audit_docs;
pub mod cli;
pub mod help;
pub mod init;
pub mod upgrade;
pub mod watch;

pub use corky_core::{
    accounts, app_config, config, desktop_notify, file_store, oauth_loopback, resolve, util,
};
pub use corky_google::{cal, doc, gsc, tasks};
pub use corky_mail::{
    contact, doctor, draft, filter, label, mailbox, schedule, search, skill, sync, topics,
};
pub use corky_social as social;

#[cfg(feature = "transcribe")]
pub use corky_transcribe as transcribe;

#[cfg(not(feature = "transcribe"))]
pub mod transcribe {
    use anyhow::{Result, bail};
    use std::path::Path;

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        _file: &Path,
        _model: Option<&str>,
        _language: Option<&str>,
        _output: Option<&str>,
        _speakers: &[String],
        _diarize: bool,
        _no_adaptive_chunk: bool,
        _no_resolve_unknown: bool,
        _no_confidence_retranscribe: bool,
    ) -> Result<()> {
        bail!(
            "Transcription support is not compiled in. \
             Rebuild with: cargo install corky --features transcribe-cuda"
        )
    }
}
