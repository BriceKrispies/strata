//! Impact Lab runs. Filled in with the simulator; until then every operation says so.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use serde::Deserialize;
use serde_json::Value as Json;
use strata_protocol::api::ErrorCode;

#[derive(Clone, Debug, Deserialize)]
pub struct RunRequest {}

#[derive(Default)]
pub struct Lab {}

impl Lab {
    pub fn new() -> Self {
        Lab {}
    }
    pub fn recent(&self) -> Vec<Json> {
        Vec::new()
    }
    pub fn cancel(&self, _run: &str, _identity: &Identity) -> bool {
        false
    }
    pub fn result(&self, _id: &str, _identity: &Identity) -> Option<Json> {
        None
    }
}

fn pending<T>() -> Result<T, ApiError> {
    Err(ApiError::new(ErrorCode::Unavailable, "the simulator is not built yet"))
}

pub fn overview(_app: &App, _identity: &Identity) -> Result<Json, ApiError> {
    pending()
}
pub fn start(_shared: &Shared, _app: &App, _identity: &Identity, _request: RunRequest) -> Result<Json, ApiError> {
    pending()
}
pub fn replay(_app: &App, _identity: &Identity) -> Result<Json, ApiError> {
    pending()
}
pub fn verify(_shared: &Shared, _app: &App, _identity: &Identity) -> Result<Json, ApiError> {
    pending()
}
