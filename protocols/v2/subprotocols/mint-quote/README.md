# Mint Quote Protocol Messages

This crate implements SV2 message types for mint-quote communication between mining pools and mint services.

## Message Types

- **MintQuoteRequest**: Pool requests a mint quote from the mint service
- **MintQuoteResponse**: Mint service responds with quote details
- **MintQuoteError**: Error response from mint service

## Usage

This protocol enables mining pools to request mint quotes for mining shares using the Sv2 messaging system, providing an alternative to Redis-based communication.

The messages use standard SV2 binary encoding and framing for efficient communication between roles.