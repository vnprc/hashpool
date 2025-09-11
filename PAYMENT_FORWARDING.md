# Automatic Payment Forwarding Development Plan

## Overview

This document outlines the implementation plan for automatic ehash token forwarding in hashpool. The system will monitor the translator's wallet balance and automatically forward accumulated tokens to an external wallet using NUT-18 payment requests.

## Core Requirements

- **Environment Variable Configuration**: Use `CASHU_PAYMENT_REQUEST` to specify the destination
- **Automatic Balance Monitoring**: Periodically check wallet balance against configurable thresholds
- **CDK Integration**: Leverage existing CDK functionality for payment processing
- **Minimal Code Changes**: Keep implementation simple and maintainable

## Architecture

```
Mining Shares → Translator Wallet → Balance Check → CDK Payment API → External Wallet
                     ↓                    ↓                ↓               ↓
               Accumulate ehash      If > threshold   CDK handles:   Receives tokens
                                                     - Parsing        automatically
                                                     - Nostr DMs
                                                     - Transport
```

## Implementation Plan

### Phase 1: Configuration Structure

Add NUT-18 forwarding configuration to the translator:

```rust
// In roles/translator/src/lib/proxy_config.rs
#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    // ... existing fields
    #[serde(default)]
    pub nut18_forwarding: Option<Nut18Config>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Nut18Config {
    pub enabled: bool,
    pub payment_request: String,
    pub sweep_threshold: u64,     // Amount to accumulate before forwarding
    pub sweep_interval_secs: u64, // Time-based forwarding interval
}
```

### Phase 2: Environment Variable Integration

Modify the translator constructor to read the environment variable and merge with config file settings:

```rust
// In roles/translator/src/lib/mod.rs
impl TranslatorSv2 {
    pub fn new(mut config: ProxyConfig) -> Self {
        // Override payment_request with environment variable if present
        if let Ok(payment_request) = env::var("CASHU_PAYMENT_REQUEST") {
            info!("Found CASHU_PAYMENT_REQUEST environment variable, enabling NUT-18 forwarding");
            
            // Use existing config values or defaults
            let nut18_config = config.nut18_forwarding.take().unwrap_or_else(|| {
                proxy_config::Nut18Config {
                    enabled: false,
                    payment_request: String::new(),
                    sweep_threshold: 1000,    // Default threshold
                    sweep_interval_secs: 60,  // Default interval
                }
            });
            
            config.nut18_forwarding = Some(proxy_config::Nut18Config {
                enabled: true,
                payment_request,
                sweep_threshold: nut18_config.sweep_threshold,
                sweep_interval_secs: nut18_config.sweep_interval_secs,
            });
        }
        // ... rest of constructor
    }
}
```

### Phase 3: CDK-Based Token Forwarding

Add token forwarding methods using existing CDK functionality:

```rust
// Add to TranslatorSv2 impl
async fn check_and_forward_tokens(&self) -> Result<()> {
    if let Some(nut18_config) = &self.config.nut18_forwarding {
        if !nut18_config.enabled {
            return Ok(());
        }
        
        // Parse payment request using CDK
        let payment_request = PaymentRequest::from_str(&nut18_config.payment_request)?;
        
        // Check balance
        if let Some(wallet) = &self.wallet {
            let balance = wallet.total_balance().await?;
            if balance >= nut18_config.sweep_threshold.into() {
                info!("Balance {} >= threshold {}, forwarding tokens", 
                      balance, nut18_config.sweep_threshold);
                self.fulfill_payment_via_cdk(wallet, &payment_request, balance.into()).await?;
            }
        }
    }
    Ok(())
}

async fn fulfill_payment_via_cdk(
    &self, 
    wallet: &Arc<Wallet>, 
    request: &PaymentRequest, 
    amount: u64
) -> Result<()> {
    // Use CDK's prepare_send (same as cdk-cli)
    let prepared_send = wallet.prepare_send(
        amount.into(), 
        SendOptions { include_fee: true, ..Default::default() }
    ).await?;
    
    // Create payment payload (same format as cdk-cli)
    let payload = PaymentRequestPayload {
        id: request.payment_id.clone(),
        memo: None,
        mint: wallet.mint_url.clone(),
        unit: wallet.unit.clone(), 
        proofs: prepared_send.proofs(),
    };
    
    // CDK handles transport automatically (Nostr NIP-17 or HTTP POST)
    // This uses the same transport resolution as cdk-cli
    wallet.send_payment_request_payload(&payload, request).await?;
    
    info!("Successfully forwarded {} ehash tokens", amount);
    Ok(())
}
```

### Phase 4: Automatic Sweeping Integration

Add a background task to periodically check and forward tokens:

```rust
// In the start() method after wallet initialization
if let Some(nut18_config) = &self.config.nut18_forwarding {
    if nut18_config.enabled {
        info!("Starting automatic token forwarding task");
        let wallet_clone = wallet.clone();
        let config_clone = nut18_config.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                std::time::Duration::from_secs(config_clone.sweep_interval_secs)
            );
            
            loop {
                interval.tick().await;
                
                match TranslatorSv2::check_and_forward_tokens_static(&wallet_clone, &config_clone).await {
                    Ok(()) => {},
                    Err(e) => {
                        warn!("Token forwarding failed: {}", e);
                    }
                }
            }
        });
    }
}
```

And add the static helper method:

```rust
async fn check_and_forward_tokens_static(
    wallet: &Arc<Wallet>, 
    nut18_config: &proxy_config::Nut18Config
) -> Result<()> {
    // Parse payment request using CDK
    let payment_request = PaymentRequest::from_str(&nut18_config.payment_request)?;
    
    // Check balance
    let balance = wallet.total_balance().await?;
    
    if balance >= nut18_config.sweep_threshold.into() {
        info!("Balance {} >= threshold {}, forwarding tokens", 
              balance, nut18_config.sweep_threshold);
        
        // Use prepare_send directly
        let prepared_send = wallet.prepare_send(
            balance, 
            SendOptions { include_fee: true, ..Default::default() }
        ).await?;
        
        // Create payment payload (same format as cdk-cli)
        let payload = PaymentRequestPayload {
            id: payment_request.payment_id.clone(),
            memo: None,
            mint: wallet.mint_url.clone(),
            unit: wallet.unit.clone(), 
            proofs: prepared_send.proofs(),
        };
        
        // CDK handles transport automatically (Nostr NIP-17 or HTTP POST)
        wallet.send_payment_request_payload(&payload, &payment_request).await?;
        
        info!("Successfully forwarded {} ehash tokens", balance);
    }
    
    Ok(())
}
```

### Phase 5: Dependencies

Add required dependencies to `roles/translator/Cargo.toml`:

```toml
[dependencies]
# ... existing dependencies
serde_cbor = "0.11"    # For CBOR decoding
base64 = "0.21"        # For base64 decoding  
nostr = "0.35"         # Rust Nostr client library
nostr-sdk = "0.35"     # High-level SDK with NIP-17 support
```

## Testing Strategy

### Manual Test Setup

```bash
# 1. Generate payment request in receiver wallet
mkdir -p /tmp/receiver-wallet
cd /tmp/receiver-wallet
timeout 5s cdk-cli create-request hash "Auto forwarding test" > request.txt
PAYMENT_REQUEST=$(grep -o 'creqA[A-Za-z0-9_-]*' request.txt)

# 2. Start hashpool with forwarding enabled
cd /home/evan/work/hashpool
CASHU_PAYMENT_REQUEST="$PAYMENT_REQUEST" devenv up

# 3. Monitor logs for forwarding activity
tail -f logs/proxy.log | grep -i "forwarding\|balance.*threshold"

# 4. Check receiver balance after mining
cd /tmp/receiver-wallet
cdk-cli balance
```

### Expected Behavior

- Payment request parsed successfully on startup
- Periodic balance checks logged every 60 seconds
- When balance exceeds threshold, forwarding attempt logged
- Receiver wallet should show incoming tokens

## Key Features

- **Threshold-Based**: Only forwards when balance meets configured threshold
- **Timer-Based**: Regular checks ensure tokens don't accumulate indefinitely  
- **Environment Driven**: Simple `CASHU_PAYMENT_REQUEST` configuration
- **CDK Integration**: Uses proven CDK payment preparation methods
- **Minimal Impact**: No changes to existing mining or minting flows

## Implementation Notes

- CDK already handles all transport layers (Nostr NIP-17 and HTTP POST) automatically
- Use `wallet.send_payment_request_payload()` method which mirrors cdk-cli functionality
- Maintain existing quote processing and minting functionality unchanged
- Add proper error handling for network failures and invalid payment requests
- Consider rate limiting for high-frequency operations

## Configuration Options

### Config File Example
Add to your proxy config file:

```toml
[nut18_forwarding]
enabled = false  # Will be set to true automatically when CASHU_PAYMENT_REQUEST is set
payment_request = ""  # Will be overridden by environment variable
sweep_threshold = 500  # Forward when wallet has 500+ ehash tokens
sweep_interval_secs = 30  # Check balance every 30 seconds
```

### Environment Variables
- `CASHU_PAYMENT_REQUEST`: The NUT-18 payment request string (required)
  - When set, automatically enables forwarding and uses config file thresholds/intervals
  - Example: `CASHU_PAYMENT_REQUEST="creqA..." devenv up`

### Default Values (if not in config file)
- Sweep threshold: 1000 ehash tokens
- Sweep interval: 60 seconds
- Include fees in send operations: true

This implementation provides a clean, maintainable solution for automatic token forwarding using CDK's existing capabilities.