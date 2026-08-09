use std::sync::Arc;

use async_trait::async_trait;
// NUT-XX: PaymentMethod is only referenced from the disabled fetch call below;
// kept imported (rather than removed) so re-enabling it is a one-line uncomment.
#[allow(unused_imports)]
use cdk::{nuts::PaymentMethod, wallet::Wallet};
use mint_quote_sv2::{from_bytes, MintQuoteFailure, MintQuoteNotification};
use tracing::{info, warn};

use super::custom_handler::CustomMiningMessageHandler;

pub struct CdkQuoteNotificationHandler {
    // NUT-XX: unused while the quote-id fetch below is disabled; retained so the
    // handler still owns the wallet it's constructed with. See handle_custom_message.
    #[allow(dead_code)]
    wallet: Arc<Wallet>,
}

impl std::fmt::Debug for CdkQuoteNotificationHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdkQuoteNotificationHandler")
            .field("wallet", &"<Arc<Wallet>>")
            .finish()
    }
}

impl CdkQuoteNotificationHandler {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }
}

#[async_trait]
impl CustomMiningMessageHandler for CdkQuoteNotificationHandler {
    async fn handle_custom_message(
        &self,
        msg_type: u8,
        payload: &[u8],
    ) -> Result<(), anyhow::Error> {
        match msg_type {
            mint_quote_sv2::MESSAGE_TYPE_MINT_QUOTE_NOTIFICATION => {
                let mut payload_buf = payload.to_vec();
                let notification: MintQuoteNotification =
                    from_bytes(&mut payload_buf)
                        .map_err(|e| anyhow::anyhow!("Failed to decode MintQuoteNotification: {e:?}"))?;
                let quote_id = notification.quote_id.as_utf8_or_hex();
                let amount = notification.amount;
                info!(
                    "Received MintQuoteNotification: quote_id={}, amount={}",
                    quote_id, amount
                );
                // NUT-XX: discovery now via Wallet::mint_quotes_by_pubkey in the sweeper;
                // SV2 quote-id fetch retained but disabled until the NUT/CDK lands upstream.
                // self.wallet
                //     .fetch_mint_quote(
                //         &quote_id,
                //         Some(PaymentMethod::Custom("ehash".to_string())),
                //     )
                //     .await
                //     .map_err(|e| anyhow::anyhow!("Failed to fetch mint quote: {e}"))?;
            }
            mint_quote_sv2::MESSAGE_TYPE_MINT_QUOTE_FAILURE => {
                let mut payload_buf = payload.to_vec();
                let failure: MintQuoteFailure = from_bytes(&mut payload_buf)
                    .map_err(|e| anyhow::anyhow!("Failed to decode MintQuoteFailure: {e:?}"))?;
                let quote_id = failure.quote_id.as_utf8_or_hex();
                let error_code = failure.error_code;
                let error_message = failure.error_message.as_utf8_or_hex();
                warn!(
                    "Received MintQuoteFailure: quote_id={}, code={}, message={}",
                    quote_id, error_code, error_message
                );
            }
            _ => {}
        }
        Ok(())
    }
}
