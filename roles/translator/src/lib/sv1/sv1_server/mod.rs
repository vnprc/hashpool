pub(super) mod channel;
mod difficulty_manager;
pub mod downstream_message_handler;
pub mod sv1_server;

/// Delimiter used to separate original job ID from keepalive mutation counter.
/// Format: `{original_job_id}#{counter}`
const KEEPALIVE_JOB_ID_DELIMITER: char = '#';
