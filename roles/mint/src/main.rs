use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::str::FromStr;

use cdk::mint::Mint;
use cdk::nuts::{CurrencyUnit, MintInfo, Nuts, PaymentMethod, MintMethodSettings};
use cdk::nuts::nutXX::MintQuoteMiningShareRequest;
use cdk::types::QuoteTTL;
use cdk::Amount;
use cdk_axum::cache::HttpCache;
use cdk_mintd::config::{self, LnBackend};
use cdk::cdk_payment::MintPayment;
use cdk::cdk_payment;
use cdk_signatory::db_signatory::DbSignatory;
use cdk::types::PaymentProcessorKey;
use cdk_sqlite::MintSqliteDatabase;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use bip39::Mnemonic;
use anyhow::{Result, bail};
use shared_config::{PoolGlobalConfig, Sv2MessagingConfig};
use tokio::net::TcpStream;
use network_helpers_sv2::plain_connection_tokio::PlainConnection;
use roles_logic_sv2::parsers::{PoolMessages, MintQuote};
use mint_quote_sv2::MintQuoteResponse;
use codec_sv2::StandardSv2Frame;
use const_sv2::{MESSAGE_TYPE_MINT_QUOTE_REQUEST, MESSAGE_TYPE_MINT_QUOTE_RESPONSE, MESSAGE_TYPE_MINT_QUOTE_ERROR};
use binary_sv2::{self, Str0255, Sv2Option};

use toml;
use std::{env, fs};

/// Connect to pool via SV2 TCP connection and listen for quote requests
async fn connect_to_pool_sv2(
    mint: Arc<Mint>,
    sv2_config: Sv2MessagingConfig,
) {
    info!("Connecting to pool SV2 endpoint: {}", sv2_config.mint_listen_address);
    
    loop {
        match TcpStream::connect(&sv2_config.mint_listen_address).await {
            Ok(stream) => {
                info!("✅ Successfully connected to pool SV2 endpoint");
                
                // Create SV2 connection with plain connection helper
                let (receiver, sender) = PlainConnection::new(stream).await;
                
                if let Err(e) = handle_sv2_connection(mint.clone(), receiver, sender).await {
                    tracing::error!("SV2 connection error: {}", e);
                }
            },
            Err(e) => {
                tracing::warn!("❌ Failed to connect to pool SV2 endpoint: {:?}", e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

/// Handle SV2 connection frames and process mint quote requests
async fn handle_sv2_connection(
    mint: Arc<Mint>,
    receiver: async_channel::Receiver<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
    sender: async_channel::Sender<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
) -> Result<()> {
    info!("Starting SV2 message handling loop");
    
    while let Ok(either_frame) = receiver.recv().await {
        if let Err(e) = process_sv2_frame(&mint, either_frame, &sender).await {
            tracing::error!("Error processing SV2 frame: {}", e);
            // Continue processing other frames
        }
    }
    
    Ok(())
}

/// Process a single SV2 frame
async fn process_sv2_frame(
    mint: &Arc<Mint>,
    either_frame: codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>,
    sender: &async_channel::Sender<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
) -> Result<()> {
    tracing::debug!("Received SV2 either frame");
    
    match either_frame {
        codec_sv2::StandardEitherFrame::Sv2(incoming) => {
            process_sv2_message(mint, incoming, sender).await
        }
        codec_sv2::StandardEitherFrame::HandShake(_) => {
            tracing::debug!("Received handshake frame - ignoring");
            Ok(())
        }
    }
}

/// Process an SV2 message frame
async fn process_sv2_message(
    mint: &Arc<Mint>,
    mut incoming: codec_sv2::StandardSv2Frame<roles_logic_sv2::parsers::PoolMessages<'static>>,
    sender: &async_channel::Sender<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
) -> Result<()> {
    tracing::debug!("Received SV2 frame");
    
    let message_type = incoming
        .get_header()
        .ok_or_else(|| anyhow::anyhow!("No header set"))?
        .msg_type();
    let payload = incoming.payload();
    
    tracing::debug!("Received message type: 0x{:02x}, payload length: {} bytes", message_type, payload.len());
    
    if is_mint_quote_message(message_type) {
        process_mint_quote_message(mint.clone(), message_type, payload, sender).await
    } else {
        tracing::warn!("Received non-mint-quote message type: 0x{:02x}", message_type);
        Ok(())
    }
}

/// Check if message type is a mint quote message
fn is_mint_quote_message(message_type: u8) -> bool {
    matches!(message_type, MESSAGE_TYPE_MINT_QUOTE_REQUEST | MESSAGE_TYPE_MINT_QUOTE_RESPONSE | MESSAGE_TYPE_MINT_QUOTE_ERROR)
}

/// Process mint quote messages
async fn process_mint_quote_message(
    mint: Arc<Mint>,
    message_type: u8,
    payload: &[u8],
    _sender: &async_channel::Sender<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
) -> Result<()> {
    info!("Received mint quote message - processing with mint");
    
    match message_type {
        MESSAGE_TYPE_MINT_QUOTE_REQUEST => {
            // Parse the payload into a MintQuoteRequest 
            let mut payload_copy = payload.to_vec();
            let parsed_request: mint_pool_messaging::MintQuoteRequest = binary_sv2::from_bytes(&mut payload_copy)
                .map_err(|e| anyhow::anyhow!("Failed to parse MintQuoteRequest: {:?}", e))?;
            
            // Create a static lifetime version for the conversion function
            let request_static = create_static_mint_quote_request(parsed_request)?;
            
            // Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest
            let cdk_request = convert_sv2_to_cdk_quote_request(request_static)?;
            
            // Process with CDK mint
            match mint.create_mint_mining_share_quote(cdk_request).await {
                Ok(quote_response) => {
                    info!("Successfully created mint quote: {:?}", quote_response);
                    // TODO: Send response back to pool
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("Failed to create mint quote: {}", e);
                    // TODO: Send error response back to pool
                    Err(anyhow::anyhow!("Mint quote creation failed: {}", e))
                }
            }
        },
        _ => {
            tracing::warn!("Received unsupported mint quote message type: 0x{:02x}", message_type);
            Ok(())
        }
    }
}

/// Send quote response back to pool
async fn send_quote_response(
    response: MintQuoteResponse<'static>,
    sender: &async_channel::Sender<codec_sv2::StandardEitherFrame<roles_logic_sv2::parsers::PoolMessages<'static>>>,
) -> Result<()> {
    let pool_response = PoolMessages::MintQuote(
        MintQuote::MintQuoteResponse(response)
    );
    
    let sv2_frame: StandardSv2Frame<PoolMessages> = pool_response.try_into()
        .map_err(|e| anyhow::anyhow!("Failed to create SV2 frame: {:?}", e))?;
    let either_frame = sv2_frame.into();
    
    sender.send(either_frame).await
        .map_err(|e| anyhow::anyhow!("Failed to send response: {}", e))?;
        
    Ok(())
}

/// Create a static lifetime version of MintQuoteRequest from a borrowed one
fn create_static_mint_quote_request(
    parsed_request: mint_pool_messaging::MintQuoteRequest
) -> Result<mint_pool_messaging::MintQuoteRequest<'static>> {
    use binary_sv2::{U256, CompressedPubKey};
    
    // Convert the borrowed data to owned data with static lifetime
    let unit_str = String::from_utf8_lossy(parsed_request.unit.inner_as_ref()).to_string();
    let unit_static = Str0255::try_from(unit_str)
        .map_err(|e| anyhow::anyhow!("Invalid unit string: {:?}", e))?;
    
    let description_static = if let Some(desc) = parsed_request.description.into_inner() {
        let desc_str = String::from_utf8_lossy(desc.inner_as_ref()).to_string();
        let desc_static = Str0255::try_from(desc_str)
            .map_err(|e| anyhow::anyhow!("Invalid description string: {:?}", e))?;
        Sv2Option::new(Some(desc_static))
    } else {
        Sv2Option::new(None)
    };
    
    // Create owned versions of the other fields
    let header_hash_bytes = parsed_request.header_hash.inner_as_ref().to_vec();
    let header_hash_static = U256::try_from(header_hash_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {:?}", e))?;
    
    let locking_key_bytes = parsed_request.locking_key.inner_as_ref().to_vec();  
    let locking_key_static = CompressedPubKey::try_from(locking_key_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking key: {:?}", e))?;
    
    let keyset_id_bytes = parsed_request.keyset_id.inner_as_ref().to_vec();
    let keyset_id_static = U256::try_from(keyset_id_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid keyset ID: {:?}", e))?;
    
    Ok(mint_pool_messaging::MintQuoteRequest {
        amount: parsed_request.amount,
        unit: unit_static,
        header_hash: header_hash_static,
        description: description_static,
        locking_key: locking_key_static,
        keyset_id: keyset_id_static,
    })
}

/// Convert SV2 MintQuoteRequest to CDK MintQuoteMiningShareRequest  
fn convert_sv2_to_cdk_quote_request(
    sv2_request: mint_pool_messaging::MintQuoteRequest<'static>,
) -> Result<cdk::nuts::nutXX::MintQuoteMiningShareRequest> {
    use cdk::secp256k1::hashes::Hash as CdkHashTrait;
    
    // Convert amount (already u64)
    let amount = cdk::Amount::from(sv2_request.amount);
    
    // Convert unit (should be "HASH")  
    let unit = cdk::nuts::CurrencyUnit::Hash;
    
    // Convert header hash from SV2 U256 to CDK Hash
    let header_hash_bytes = sv2_request.header_hash.inner_as_ref();
    let header_hash = CdkHashTrait::from_slice(header_hash_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid header hash: {}", e))?;
    
    // Convert description (optional)  
    let description = sv2_request.description.into_inner().map(|s| {
        String::from_utf8_lossy(s.inner_as_ref()).to_string()
    });
    
    // Convert locking key (compressed public key)
    let pubkey_bytes = sv2_request.locking_key.inner_as_ref();
    let pubkey = cdk::nuts::PublicKey::from_slice(pubkey_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid locking pubkey: {}", e))?;
    
    // Convert keyset ID from SV2 U256 to CDK format
    let keyset_id_bytes = sv2_request.keyset_id.inner_as_ref();
    let keyset_id = mining_sv2::cashu::keyset_from_sv2_bytes(keyset_id_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to convert keyset ID: {}", e))?;
    
    Ok(cdk::nuts::nutXX::MintQuoteMiningShareRequest {
        amount,
        unit,
        header_hash,
        description,
        pubkey,
        keyset_id,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("debug,sqlx=warn,hyper=warn,h2=warn"))
        .init();

    let mut args = std::env::args().skip(1); // Skip binary name

    let mint_config_path = match (args.next().as_deref(), args.next()) {
        (Some("-c"), Some(path)) => path,
        _ => {
            eprintln!("Usage: -c <mint_config_path> -g <global_config_path>");
            std::process::exit(1);
        }
    };

    let global_config_path = match (args.next().as_deref(), args.next()) {
        (Some("-g"), Some(path)) => path,
        _ => {
            eprintln!("Usage: -c <mint_config_path> -g <global_config_path>");
            std::process::exit(1);
        }
    };

    let mint_settings = config::Settings::new(Some(mint_config_path)).from_env()?;
    let global_config: PoolGlobalConfig = toml::from_str(&fs::read_to_string(global_config_path)?)?;

    if mint_settings.ln.ln_backend == LnBackend::None {
        bail!("Ln backend must be set");
    }

    let work_dir: PathBuf = home::home_dir()
        .unwrap()
        .join(".cdk-mintd");

    std::fs::create_dir_all(&work_dir)?;

    // TODO add to config
    const NUM_KEYS: u8 = 64;

    let mnemonic = Mnemonic::from_str(&mint_settings.info.mnemonic.unwrap())
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic in mint config: {}", e))?;
    let seed_bytes : &[u8] = &mnemonic.to_seed("");

    let hash_currency_unit = CurrencyUnit::Hash;

    let mut currency_units = HashMap::new();
    currency_units.insert(hash_currency_unit.clone(), (0, NUM_KEYS));

    let cache: HttpCache = mint_settings.info.http_cache.into();

    // TODO update settings to accept mint db path, just use env var for now
    let mut mint_db_path = resolve_and_prepare_db_path(".devenv/state/mint/mint.sqlite");

    // override config file with env var for improved devex configurability
    if let Ok(db_path_override) = std::env::var("CDK_MINT_DB_PATH") {
        tracing::info!("Overriding mint.dbPath with env var CDK_MINT_DB_PATH={}", db_path_override);
        mint_db_path = resolve_and_prepare_db_path(&db_path_override);
    }

    let db = Arc::new(MintSqliteDatabase::new(mint_db_path).await?);

    let signatory = Arc::new(
        DbSignatory::new(
            db.clone(),
            seed_bytes,
            currency_units,
            HashMap::new(),
        ).await.unwrap()
    );

    let ln: HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>> = HashMap::new();

    // Configure NUT-04 settings for MiningShare payment method with HASH unit
    let mining_share_method = MintMethodSettings {
        method: PaymentMethod::MiningShare,
        unit: hash_currency_unit.clone(),
        min_amount: Some(Amount::from(1)),
        // TODO update units to 2^bits not just raw bits
        max_amount: Some(Amount::from(256)),
        options: None,
    };
    
    let mut nuts = Nuts::new();
    nuts.nut04.methods.push(mining_share_method);
    nuts.nut04.disabled = false;
    
    let mint_info = MintInfo {
        name: Some(mint_settings.mint_info.name.clone()),
        description: Some(mint_settings.mint_info.description.clone()),
        pubkey: None,
        version: None,
        description_long: None,
        contact: None,
        nuts,
        icon_url: None,
        urls: None,
        motd: None,
        time: None,
        tos_url: None,
    };

    let mint = Arc::new(Mint::new(
        mint_info,
        signatory,
        db,
        ln,
    )
    .await.unwrap());
    
    mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;

    let router = cdk_axum::create_mint_router_with_custom_cache(mint.clone(), cache, false).await?;

    // Start background tasks for invoice monitoring
    mint.start().await?;

    let redis_url = global_config.redis.url.clone();
    let active_keyset_prefix = global_config.redis.active_keyset_prefix.clone();
    
    use redis::AsyncCommands;
    use serde_json;

    let keysets = mint.keysets();
    let keyset_id = keysets.keysets.first().unwrap().id;
    let keyset = mint.keyset(&keyset_id).unwrap();

    // Serialize full keyset
    let keyset_json = serde_json::to_string(&keyset).expect("Failed to serialize keyset");

    let redis_client = redis::Client::open(redis_url.clone())?;
    let mut redis_conn = redis_client.get_async_connection().await?;

    let redis_key = &active_keyset_prefix;

    // Cache and broadcast
    redis_conn.set(redis_key, &keyset_json).await?;

    tracing::info!(
        "Published keyset {} to Redis key '{}",
        keyset_id,
        redis_key,
    );

    // Start SV2 connection to pool if enabled
    if let Some(ref sv2_config) = global_config.sv2_messaging {
        if sv2_config.enabled {
            tokio::spawn(connect_to_pool_sv2(
                mint.clone(),
                sv2_config.clone(),
            ));
        }
    }

    info!("Mint listening on {}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port);
    let addr = format!("{}:{}", mint_settings.info.listen_host, mint_settings.info.listen_port);
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, router).await?;

    Ok(())
}

fn resolve_and_prepare_db_path(config_path: &str) -> PathBuf {
    let path = Path::new(config_path);
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .expect("Failed to get current working directory")
            .join(path)
    };

    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).expect("Failed to create parent directory for DB path");
        }
    }

    full_path
}