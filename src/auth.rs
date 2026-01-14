//! Authentication module for StandX API
//!
//! Handles Ed25519 key generation, request signing, and wallet authentication.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use ethers::core::k256::ecdsa::SigningKey as EthSigningKey;
use ethers::signers::{LocalWallet, Signer as EthSigner};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Base URL for StandX API
const BASE_URL: &str = "https://api.standx.com";

/// StandX authentication handler
pub struct StandXAuth {
    signing_key: SigningKey,
    verify_key: VerifyingKey,
    request_id: String,
    token: Option<String>,
    token_expires_at: u64,
}

/// Signature headers for authenticated requests
#[derive(Debug, Clone)]
pub struct SignatureHeaders {
    pub version: String,
    pub request_id: String,
    pub timestamp: String,
    pub signature: String,
}

impl StandXAuth {
    /// Create a new authentication instance with generated Ed25519 key pair
    pub fn new() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verify_key = signing_key.verifying_key();
        let request_id = base58_encode(verify_key.as_bytes());

        Self {
            signing_key,
            verify_key,
            request_id,
            token: None,
            token_expires_at: 0,
        }
    }

    /// Get the request ID (Base58-encoded public key)
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Get the current JWT token
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Get the verifying key for signature verification
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verify_key
    }

    /// Sign arbitrary bytes with Ed25519
    pub fn sign_bytes(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    /// Verify a signature with the public key
    pub fn verify_signature(&self, message: &[u8], signature: &Signature) -> bool {
        self.verify_key.verify(message, signature).is_ok()
    }


    /// Sign a request payload for authenticated endpoints
    ///
    /// Returns signature headers with format: "v1,{request_id},{timestamp},{payload}"
    pub fn sign_request(&self, payload: &str) -> SignatureHeaders {
        let request_id = Uuid::new_v4().to_string();
        let timestamp = current_timestamp_ms();
        let version = "v1".to_string();

        // Create message to sign: "v1,{request_id},{timestamp},{payload}"
        let message = format!("{},{},{},{}", version, request_id, timestamp, payload);
        let message_bytes = message.as_bytes();

        // Sign with Ed25519
        let signature = self.signing_key.sign(message_bytes);
        let signature_b64 = BASE64.encode(signature.to_bytes());

        SignatureHeaders {
            version,
            request_id,
            timestamp: timestamp.to_string(),
            signature: signature_b64,
        }
    }

    /// Get all headers needed for authenticated requests
    pub fn get_auth_headers(&self, payload: &str) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if let Some(token) = &self.token {
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
        }

        if !payload.is_empty() {
            let sig_headers = self.sign_request(payload);
            headers.insert("x-request-sign-version".to_string(), sig_headers.version);
            headers.insert("x-request-id".to_string(), sig_headers.request_id);
            headers.insert("x-request-timestamp".to_string(), sig_headers.timestamp);
            headers.insert("x-request-signature".to_string(), sig_headers.signature);
        }

        headers
    }

    /// Check if authenticated with a valid token
    pub fn is_authenticated(&self) -> bool {
        if let Some(_token) = &self.token {
            let now = current_timestamp_ms() / 1000;
            now < self.token_expires_at
        } else {
            false
        }
    }


    /// Authenticate with StandX using wallet signature
    ///
    /// # Arguments
    /// * `chain` - Blockchain chain (e.g., "bsc")
    /// * `private_key` - Wallet private key (hex string, with or without 0x prefix)
    ///
    /// # Returns
    /// JWT access token on success
    pub async fn authenticate(&mut self, chain: &str, private_key: &str) -> Result<String> {
        let client = reqwest::Client::new();

        // Step 1: Get wallet address from private key
        let wallet_address = get_wallet_address(chain, private_key)?;
        tracing::info!("Wallet address: {}", wallet_address);

        // Step 2: Request signature data from server
        let signed_data = self.prepare_sign_in(&client, chain, &wallet_address).await?;
        tracing::debug!("Signed data (JWT): {}", signed_data);

        // Step 3: Parse message from signed data (JWT)
        let payload = parse_jwt(&signed_data)?;
        tracing::debug!("JWT payload: {:?}", payload);
        
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing message in JWT payload"))?;
        
        // Log message with escaped newlines to see full structure
        tracing::info!("Message to sign (escaped): {:?}", message);
        tracing::info!("Message length: {}", message.len());

        // Step 4: Sign message with wallet (use async version)
        let signature = sign_message_with_wallet_async(chain, private_key, message).await?;
        tracing::debug!("Signature: {}", signature);

        // Step 5: Login to get access token
        let login_response = self.login(&client, chain, &signature, &signed_data).await?;

        self.token = Some(login_response.token.clone());
        // Token expires in 7 days by default
        self.token_expires_at = (current_timestamp_ms() / 1000) + 7 * 24 * 60 * 60;

        Ok(login_response.token)
    }

    /// Request signature data from server (prepare sign-in)
    async fn prepare_sign_in(
        &self,
        client: &reqwest::Client,
        chain: &str,
        address: &str,
    ) -> Result<String> {
        let url = format!("{}/v1/offchain/prepare-signin?chain={}", BASE_URL, chain);

        let request_body = PrepareSignInRequest {
            address: address.to_string(),
            request_id: self.request_id.clone(),
        };

        let response = client
            .post(&url)
            .json(&request_body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to prepare sign-in: {} - {}", status, error_text));
        }

        let data: PrepareSignInResponse = response.json().await?;

        if !data.success {
            return Err(anyhow!("Failed to prepare sign-in: server returned success=false"));
        }

        Ok(data.signed_data)
    }

    /// Login with signature to get access token
    async fn login(
        &self,
        client: &reqwest::Client,
        chain: &str,
        signature: &str,
        signed_data: &str,
    ) -> Result<LoginResponse> {
        let url = format!("{}/v1/offchain/login?chain={}", BASE_URL, chain);

        let request_body = LoginRequest {
            signature: signature.to_string(),
            signed_data: signed_data.to_string(),
            expires_seconds: 604800, // 7 days
        };

        let response = client
            .post(&url)
            .json(&request_body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to login: {} - {}", status, error_text));
        }

        let data: LoginResponse = response.json().await?;
        Ok(data)
    }
}

impl Default for StandXAuth {
    fn default() -> Self {
        Self::new()
    }
}


// ============================================================================
// Helper Functions
// ============================================================================

/// Get current timestamp in milliseconds
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

/// Base58 encode bytes (Bitcoin alphabet)
pub fn base58_encode(data: &[u8]) -> String {
    bs58::encode(data).into_string()
}

/// Base58 decode string to bytes
pub fn base58_decode(s: &str) -> Result<Vec<u8>> {
    bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("Base58 decode error: {}", e))
}

/// Get wallet address from private key
fn get_wallet_address(chain: &str, private_key: &str) -> Result<String> {
    match chain {
        "bsc" => {
            let wallet = create_eth_wallet(private_key)?;
            // Use checksum address format (EIP-55)
            Ok(ethers::utils::to_checksum(&wallet.address(), None))
        }
        _ => Err(anyhow!("Chain {} not implemented", chain)),
    }
}

/// Sign a message with wallet (EIP-191 personal sign) - async version
async fn sign_message_with_wallet_async(chain: &str, private_key: &str, message: &str) -> Result<String> {
    match chain {
        "bsc" => {
            let wallet = create_eth_wallet(private_key)?;
            
            // Debug: log the message being signed
            tracing::debug!("Signing message: {}", message);
            
            let signature = wallet.sign_message(message).await?;
            let sig_hex = format!("0x{}", hex::encode(signature.to_vec()));
            
            tracing::debug!("Signature: {}", sig_hex);
            
            Ok(sig_hex)
        }
        _ => Err(anyhow!("Chain {} not implemented", chain)),
    }
}

/// Create an Ethereum wallet from private key
fn create_eth_wallet(private_key: &str) -> Result<LocalWallet> {
    // Remove 0x prefix if present
    let key_hex = private_key.strip_prefix("0x").unwrap_or(private_key);
    
    let key_bytes = hex::decode(key_hex)
        .map_err(|e| anyhow!("Invalid private key hex: {}", e))?;
    
    if key_bytes.len() != 32 {
        return Err(anyhow!("Private key must be 32 bytes"));
    }
    
    let signing_key = EthSigningKey::from_slice(&key_bytes)
        .map_err(|e| anyhow!("Invalid private key: {}", e))?;
    
    Ok(LocalWallet::from(signing_key))
}

/// Parse JWT payload without verification
fn parse_jwt(token: &str) -> Result<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("Invalid JWT format"));
    }

    let payload_b64 = parts[1];
    
    // URL-safe base64 decode with padding
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| {
            // Try with padding
            let padded = match payload_b64.len() % 4 {
                2 => format!("{}==", payload_b64),
                3 => format!("{}=", payload_b64),
                _ => payload_b64.to_string(),
            };
            base64::engine::general_purpose::URL_SAFE
                .decode(&padded)
        })
        .map_err(|e| anyhow!("Failed to decode JWT payload: {}", e))?;

    serde_json::from_slice(&payload_bytes)
        .map_err(|e| anyhow!("Failed to parse JWT payload: {}", e))
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Serialize)]
struct PrepareSignInRequest {
    address: String,
    #[serde(rename = "requestId")]
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct PrepareSignInResponse {
    success: bool,
    #[serde(rename = "signedData")]
    signed_data: String,
}

#[derive(Debug, Serialize)]
struct LoginRequest {
    signature: String,
    #[serde(rename = "signedData")]
    signed_data: String,
    #[serde(rename = "expiresSeconds")]
    expires_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    token: String,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_generates_valid_keypair() {
        let auth = StandXAuth::new();
        
        // Request ID should be non-empty Base58 string
        assert!(!auth.request_id().is_empty());
        
        // Should be able to decode the request_id back to bytes
        let decoded = base58_decode(auth.request_id()).unwrap();
        assert_eq!(decoded.len(), 32); // Ed25519 public key is 32 bytes
    }

    #[test]
    fn test_sign_and_verify() {
        let auth = StandXAuth::new();
        let message = b"test message";
        
        let signature = auth.sign_bytes(message);
        assert!(auth.verify_signature(message, &signature));
        
        // Wrong message should fail verification
        let wrong_message = b"wrong message";
        assert!(!auth.verify_signature(wrong_message, &signature));
    }

    #[test]
    fn test_sign_request_format() {
        let auth = StandXAuth::new();
        let payload = r#"{"symbol":"BTC-USD"}"#;
        
        let headers = auth.sign_request(payload);
        
        assert_eq!(headers.version, "v1");
        assert!(!headers.request_id.is_empty());
        assert!(!headers.timestamp.is_empty());
        assert!(!headers.signature.is_empty());
        
        // Timestamp should be a valid number
        let ts: u64 = headers.timestamp.parse().unwrap();
        assert!(ts > 0);
        
        // Signature should be valid Base64
        let sig_bytes = BASE64.decode(&headers.signature).unwrap();
        assert_eq!(sig_bytes.len(), 64); // Ed25519 signature is 64 bytes
    }

    #[test]
    fn test_get_auth_headers() {
        let auth = StandXAuth::new();
        let payload = r#"{"test":"data"}"#;
        
        let headers = auth.get_auth_headers(payload);
        
        assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
        assert!(headers.contains_key("x-request-sign-version"));
        assert!(headers.contains_key("x-request-id"));
        assert!(headers.contains_key("x-request-timestamp"));
        assert!(headers.contains_key("x-request-signature"));
    }

    #[test]
    fn test_base58_encode_decode_roundtrip() {
        let original = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let encoded = base58_encode(&original);
        let decoded = base58_decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_is_authenticated_without_token() {
        let auth = StandXAuth::new();
        assert!(!auth.is_authenticated());
    }

    #[test]
    fn test_get_wallet_address_bsc() {
        // Known test private key and expected address
        // This is a well-known test key - DO NOT use in production
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let address = get_wallet_address("bsc", private_key).unwrap();
        
        // Should return a valid Ethereum address format
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42); // 0x + 40 hex chars
        
        // Known address for this test key
        assert_eq!(address.to_lowercase(), "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266");
    }

    #[test]
    fn test_get_wallet_address_invalid_chain() {
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let result = get_wallet_address("invalid_chain", private_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_eth_wallet_with_0x_prefix() {
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = create_eth_wallet(private_key).unwrap();
        assert_eq!(
            format!("{:?}", wallet.address()).to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_create_eth_wallet_without_0x_prefix() {
        let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = create_eth_wallet(private_key).unwrap();
        assert_eq!(
            format!("{:?}", wallet.address()).to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_create_eth_wallet_invalid_key() {
        let result = create_eth_wallet("invalid_hex");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jwt_valid() {
        // A simple test JWT (header.payload.signature)
        // Payload: {"message": "test message", "exp": 1234567890}
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJtZXNzYWdlIjoidGVzdCBtZXNzYWdlIiwiZXhwIjoxMjM0NTY3ODkwfQ.signature";
        let payload = parse_jwt(jwt).unwrap();
        
        assert_eq!(payload.get("message").unwrap().as_str().unwrap(), "test message");
        assert_eq!(payload.get("exp").unwrap().as_i64().unwrap(), 1234567890);
    }

    #[test]
    fn test_parse_jwt_invalid_format() {
        let result = parse_jwt("not.a.valid.jwt.format");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_message_with_wallet_bsc() {
        // Known test private key - DO NOT use in production
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let message = "Sign this message to authenticate";
        
        let signature = sign_message_with_wallet_async("bsc", private_key, message).await.unwrap();
        
        // Should return a valid hex signature with 0x prefix
        assert!(signature.starts_with("0x"));
        // EIP-191 signature is 65 bytes (130 hex chars + 0x prefix)
        assert_eq!(signature.len(), 132);
    }

    #[tokio::test]
    async fn test_sign_message_with_wallet_invalid_chain() {
        let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let message = "test message";
        
        let result = sign_message_with_wallet_async("invalid_chain", private_key, message).await;
        assert!(result.is_err());
    }
}
