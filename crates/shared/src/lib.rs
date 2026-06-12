//! Types shared between the tsubomi server and CLI.
//!
//! Defining request/response shapes once here keeps the HTTP contract in sync:
//! the server serializes these, the CLI deserializes the same structs.

use serde::{Deserialize, Serialize};

/// Response body for `GET /api/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
}

/// Response body for `GET /api/hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Greeting {
    pub message: String,
}
