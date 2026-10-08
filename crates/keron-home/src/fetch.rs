//! Fetching a widget's payload from the door or a script. `zeron:` sources
//! are answered by the app from its snapshot ([`crate::zeron::payload`]).

use std::path::PathBuf;

use keron_door::{DoorClient, DoorError};

use crate::script::{ScriptError, ScriptOptions};
use crate::{Manifest, Payload, PayloadError};

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
        let _ = error;
        todo!("keron-home: FetchError from DoorError")
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
        let _ = (manifest, &self.widgets_dir, &self.script);
        todo!("keron-home: Fetcher::fetch")
    }
}
