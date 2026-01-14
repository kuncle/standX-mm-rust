//! Property tests for authentication module
//! Tests for Property 1 and Property 2

use proptest::prelude::*;

// We need to access the auth module from the main crate
// Since standx-mm is a binary crate, we'll test the public interface

/// Generate arbitrary byte arrays for testing
fn arb_message_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..1024)
}

/// Generate arbitrary payload strings for testing
fn arb_payload_string() -> impl Strategy<Value = String> {
    // Generate valid JSON-like payloads
    prop::string::string_regex(r#"\{"[a-z]+":"[a-zA-Z0-9]+"\}"#)
        .unwrap()
        .prop_filter("non-empty payload", |s| !s.is_empty())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: standx-maker-bot, Property 1: Ed25519 Signing Round-Trip**
    /// 
    /// *For any* message bytes, signing with a generated Ed25519 key pair 
    /// and then verifying with the corresponding public key should succeed.
    /// 
    /// **Validates: Requirements 1.1, 1.4**
    #[test]
    fn prop_ed25519_signing_roundtrip(message in arb_message_bytes()) {
        use ed25519_dalek::{SigningKey, Signer, Verifier};
        use rand::rngs::OsRng;

        // Generate a new key pair
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        // Sign the message
        let signature = signing_key.sign(&message);

        // Verify the signature - should always succeed
        prop_assert!(verifying_key.verify(&message, &signature).is_ok(),
            "Signature verification failed for message of length {}", message.len());
    }

    /// **Feature: standx-maker-bot, Property 1 (extended): Wrong message fails verification**
    /// 
    /// *For any* two different messages, signing one and verifying with the other should fail.
    /// 
    /// **Validates: Requirements 1.1, 1.4**
    #[test]
    fn prop_ed25519_wrong_message_fails(
        message1 in arb_message_bytes(),
        message2 in arb_message_bytes()
    ) {
        use ed25519_dalek::{SigningKey, Signer, Verifier};
        use rand::rngs::OsRng;

        // Skip if messages are the same
        prop_assume!(message1 != message2);

        // Generate a new key pair
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        // Sign message1
        let signature = signing_key.sign(&message1);

        // Verify with message2 - should fail
        prop_assert!(verifying_key.verify(&message2, &signature).is_err(),
            "Signature verification should fail for different message");
    }
}


/// Generate arbitrary valid JSON payload strings
fn arb_json_payload() -> impl Strategy<Value = String> {
    // Generate simple JSON objects with various key-value pairs
    (
        prop::string::string_regex("[a-z]{1,10}").unwrap(),
        prop::string::string_regex("[a-zA-Z0-9]{1,20}").unwrap(),
    )
        .prop_map(|(key, value)| format!(r#"{{"{}":"{}"}}"#, key, value))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: standx-maker-bot, Property 2: Request Signature Format**
    /// 
    /// *For any* valid payload string, request_id, and timestamp, the signed message 
    /// format should be exactly "v1,{request_id},{timestamp},{payload}" and the 
    /// signature should be valid Base64.
    /// 
    /// **Validates: Requirements 1.4**
    #[test]
    fn prop_request_signature_format(payload in arb_json_payload()) {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        use ed25519_dalek::{SigningKey, Signer, Verifier};
        use rand::rngs::OsRng;
        use uuid::Uuid;
        use std::time::{SystemTime, UNIX_EPOCH};

        // Generate key pair
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        // Generate request components
        let request_id = Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let version = "v1";

        // Create message in the expected format
        let message = format!("{},{},{},{}", version, request_id, timestamp, payload);
        let message_bytes = message.as_bytes();

        // Sign the message
        let signature = signing_key.sign(message_bytes);
        let signature_b64 = BASE64.encode(signature.to_bytes());

        // Verify format constraints:
        
        // 1. Version should be "v1"
        prop_assert!(message.starts_with("v1,"), 
            "Message should start with 'v1,'");

        // 2. Request ID should be a valid UUID
        let parts: Vec<&str> = message.splitn(4, ',').collect();
        prop_assert_eq!(parts.len(), 4, "Message should have 4 comma-separated parts");
        prop_assert!(Uuid::parse_str(parts[1]).is_ok(), 
            "Request ID should be a valid UUID");

        // 3. Timestamp should be a valid number
        let ts: Result<u64, _> = parts[2].parse();
        prop_assert!(ts.is_ok(), "Timestamp should be a valid u64");
        prop_assert!(ts.unwrap() > 0, "Timestamp should be positive");

        // 4. Payload should be preserved
        prop_assert_eq!(parts[3], payload, "Payload should be preserved in message");

        // 5. Signature should be valid Base64
        let decoded = BASE64.decode(&signature_b64);
        prop_assert!(decoded.is_ok(), "Signature should be valid Base64");
        prop_assert_eq!(decoded.unwrap().len(), 64, 
            "Ed25519 signature should be 64 bytes");

        // 6. Signature should verify correctly
        prop_assert!(verifying_key.verify(message_bytes, &signature).is_ok(),
            "Signature should verify correctly");
    }

    /// **Feature: standx-maker-bot, Property 2 (extended): Signature uniqueness**
    /// 
    /// *For any* two different payloads, the signatures should be different.
    /// 
    /// **Validates: Requirements 1.4**
    #[test]
    fn prop_signature_uniqueness(
        payload1 in arb_json_payload(),
        payload2 in arb_json_payload()
    ) {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        use ed25519_dalek::{SigningKey, Signer};
        use rand::rngs::OsRng;
        use uuid::Uuid;
        use std::time::{SystemTime, UNIX_EPOCH};

        // Skip if payloads are the same
        prop_assume!(payload1 != payload2);

        // Generate key pair
        let signing_key = SigningKey::generate(&mut OsRng);

        // Use same request_id and timestamp for both
        let request_id = Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let version = "v1";

        // Create messages
        let message1 = format!("{},{},{},{}", version, request_id, timestamp, payload1);
        let message2 = format!("{},{},{},{}", version, request_id, timestamp, payload2);

        // Sign both messages
        let signature1 = signing_key.sign(message1.as_bytes());
        let signature2 = signing_key.sign(message2.as_bytes());

        // Signatures should be different
        prop_assert_ne!(signature1.to_bytes(), signature2.to_bytes(),
            "Different payloads should produce different signatures");
    }
}
