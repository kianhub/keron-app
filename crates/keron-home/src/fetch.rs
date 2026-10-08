//! Fetching a widget's payload from the door or a script. `zeron:` sources
//! are answered by the app from its snapshot ([`crate::zeron::payload`]).

use std::path::PathBuf;

use keron_door::{DoorClient, DoorError};

use crate::kinds::parse_payload;
use crate::script::{self, ScriptError, ScriptOptions};
use crate::{Manifest, Payload, PayloadError, SourceSpec};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// Not signed in to the door (or the sign-in was revoked): Home shows
    /// its sign-in card.
    #[error("sign in to your memory to see this")]
    SignedOut,
    #[error(transparent)]
    Door(DoorError),
    #[error(transparent)]
    Script(ScriptError),
    #[error(transparent)]
    Payload(PayloadError),
    /// A `zeron:` source handed to the fetcher, or no door configured.
    #[error("{0}")]
    Unsupported(String),
}

impl From<DoorError> for FetchError {
    /// `SignedOut` and `Expired` become [`FetchError::SignedOut`].
    fn from(error: DoorError) -> Self {
        match error {
            DoorError::SignedOut | DoorError::Expired => FetchError::SignedOut,
            other => FetchError::Door(other),
        }
    }
}

/// Fetches door and script sources. Cheap to clone.
#[derive(Clone)]
pub struct Fetcher {
    door: Option<DoorClient>,
    widgets_dir: PathBuf,
    script: ScriptOptions,
}

impl Fetcher {
    pub fn new(door: Option<DoorClient>, widgets_dir: PathBuf, script: ScriptOptions) -> Self {
        Self {
            door,
            widgets_dir,
            script,
        }
    }

    pub fn door(&self) -> Option<&DoorClient> {
        self.door.as_ref()
    }

    /// Fetch and parse the manifest's source as its kind. Must run inside tokio.
    pub async fn fetch(&self, manifest: &Manifest) -> Result<Payload, FetchError> {
        let value = match &manifest.source {
            SourceSpec::KeronSources(_) | SourceSpec::Memory(_) => {
                let Some(door) = &self.door else {
                    return Err(FetchError::Unsupported(
                        "this app has no memory door set up".to_string(),
                    ));
                };
                let path = manifest.source.door_path().unwrap_or_default();
                door.get_json(&path).await?
            }
            SourceSpec::Script(script) => script::run(&self.widgets_dir, script, &self.script)
                .await
                .map_err(FetchError::Script)?,
            SourceSpec::Zeron(name) => {
                return Err(FetchError::Unsupported(format!(
                    "zeron:{name} is answered by the app, not fetched"
                )));
            }
        };
        parse_payload(manifest.kind, &value).map_err(FetchError::Payload)
    }
}
