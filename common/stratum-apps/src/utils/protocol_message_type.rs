use stratum_core::extensions_sv2::{
    EXTENSION_TYPE_EXTENSIONS_NEGOTIATION, MESSAGE_TYPE_REQUEST_EXTENSIONS,
    MESSAGE_TYPE_REQUEST_EXTENSIONS_ERROR, MESSAGE_TYPE_REQUEST_EXTENSIONS_SUCCESS,
};

use crate::stratum_core::{
    common_messages_sv2::{
        MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, MESSAGE_TYPE_RECONNECT,
        MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
    },
    job_declaration_sv2::{
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS,
        MESSAGE_TYPE_DECLARE_MINING_JOB, MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR,
        MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS, MESSAGE_TYPE_PUSH_SOLUTION,
    },
    mining_sv2::{
        MESSAGE_TYPE_CLOSE_CHANNEL, MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH,
        MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB, MESSAGE_TYPE_NEW_MINING_JOB,
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS, MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR,
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB,
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        MESSAGE_TYPE_SET_EXTRANONCE_PREFIX, MESSAGE_TYPE_SET_GROUP_CHANNEL,
        MESSAGE_TYPE_SET_TARGET, MESSAGE_TYPE_SUBMIT_SHARES_ERROR,
        MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS, MESSAGE_TYPE_UPDATE_CHANNEL,
        MESSAGE_TYPE_UPDATE_CHANNEL_ERROR,
    },
    template_distribution_sv2::{
        MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS, MESSAGE_TYPE_NEW_TEMPLATE,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA, MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR,
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS, MESSAGE_TYPE_SET_NEW_PREV_HASH,
        MESSAGE_TYPE_SUBMIT_SOLUTION,
    },
};

pub fn is_common_message(extension_type: u16, message_type: u8) -> bool {
    extension_type == 0
        && matches!(
            message_type,
            MESSAGE_TYPE_SETUP_CONNECTION
                | MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS
                | MESSAGE_TYPE_SETUP_CONNECTION_ERROR
                | MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED
                | MESSAGE_TYPE_RECONNECT
        )
}

pub fn is_mining_message(extension_type: u16, message_type: u8) -> bool {
    extension_type == 0
        && matches!(
            message_type,
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL
                | MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
                | MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR
                | MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL
                | MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
                | MESSAGE_TYPE_NEW_MINING_JOB
                | MESSAGE_TYPE_UPDATE_CHANNEL
                | MESSAGE_TYPE_UPDATE_CHANNEL_ERROR
                | MESSAGE_TYPE_CLOSE_CHANNEL
                | MESSAGE_TYPE_SET_EXTRANONCE_PREFIX
                | MESSAGE_TYPE_SUBMIT_SHARES_STANDARD
                | MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED
                | MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS
                | MESSAGE_TYPE_SUBMIT_SHARES_ERROR
                // | MESSAGE_TYPE_RESERVED
                | 0x1e
                | MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB
            | MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH
            | MESSAGE_TYPE_SET_TARGET
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR
            | MESSAGE_TYPE_SET_GROUP_CHANNEL
        )
}

pub fn is_job_declaration_message(extension_type: u16, message_type: u8) -> bool {
    extension_type == 0
        && matches!(
            message_type,
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN
                | MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS
                | MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS
                | MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS
                | MESSAGE_TYPE_DECLARE_MINING_JOB
                | MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS
                | MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR
                | MESSAGE_TYPE_PUSH_SOLUTION
        )
}

pub fn is_template_distribution_message(extension_type: u16, message_type: u8) -> bool {
    extension_type == 0
        && matches!(
            message_type,
            MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS
                | MESSAGE_TYPE_NEW_TEMPLATE
                | MESSAGE_TYPE_SET_NEW_PREV_HASH
                | MESSAGE_TYPE_REQUEST_TRANSACTION_DATA
                | MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS
                | MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR
                | MESSAGE_TYPE_SUBMIT_SOLUTION
        )
}

pub fn is_extensions_message(extension_type: u16, message_type: u8) -> bool {
    extension_type == EXTENSION_TYPE_EXTENSIONS_NEGOTIATION
        && matches!(
            message_type,
            MESSAGE_TYPE_REQUEST_EXTENSIONS
                | MESSAGE_TYPE_REQUEST_EXTENSIONS_ERROR
                | MESSAGE_TYPE_REQUEST_EXTENSIONS_SUCCESS
        )
}

#[derive(Debug, PartialEq, Eq)]
pub enum MessageType {
    Common,
    Mining,
    JobDeclaration,
    TemplateDistribution,
    Extensions,
    Unknown,
}

pub fn protocol_message_type(extension_type: u16, message_type: u8) -> MessageType {
    // Remove the channel_msg bit (bit 15) from extension_type to ensure correct matching
    const CHANNEL_MSG_MASK: u16 = 0b1000_0000_0000_0000;
    let extension_type = extension_type & !CHANNEL_MSG_MASK;
    if is_common_message(extension_type, message_type) {
        MessageType::Common
    } else if is_mining_message(extension_type, message_type) {
        MessageType::Mining
    } else if is_job_declaration_message(extension_type, message_type) {
        MessageType::JobDeclaration
    } else if is_template_distribution_message(extension_type, message_type) {
        MessageType::TemplateDistribution
    } else if is_extensions_message(extension_type, message_type) {
        MessageType::Extensions
    } else {
        MessageType::Unknown
    }
}
