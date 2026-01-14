//! Property tests for HTTP client module
//! Tests for Property 13, 14, and 15

use proptest::prelude::*;
use standx_mm::http_client::RetryConfig;

// Helper function to create a RetryConfig with custom delays
fn create_retry_config(base_delay_ms: u64, max_delay_ms: u64) -> RetryConfig {
    RetryConfig {
        max_retries: 100,
        base_delay_ms,
        max_delay_ms,
    }
}

// ============================================================================
// Property 13: Exponential Backoff Calculation
// **Validates: Requirements 10.1, 10.2**
//
// For any attempt number n in [0, max_retries], the delay should be
// min(base_delay * 2^n, max_delay).
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Feature: standx-maker-bot, Property 13: Exponential Backoff Calculation
    /// For any attempt number and config, delay = min(base_delay * 2^attempt, max_delay)
    #[test]
    fn prop_exponential_backoff_calculation(
        attempt in 0u32..20,
        base_delay_ms in 100u64..5000,
        max_delay_ms in 5000u64..60000,
    ) {
        // Ensure max_delay >= base_delay for valid config
        let max_delay_ms = max_delay_ms.max(base_delay_ms);

        let config = create_retry_config(base_delay_ms, max_delay_ms);

        let delay = config.calculate_delay(attempt);

        // Calculate expected delay: base_delay * 2^attempt, capped at max_delay
        let expected = if attempt >= 64 {
            max_delay_ms
        } else {
            let raw = base_delay_ms.saturating_mul(1u64 << attempt);
            raw.min(max_delay_ms)
        };

        prop_assert_eq!(
            delay, expected,
            "Delay for attempt {} with base={}, max={} should be {}",
            attempt, base_delay_ms, max_delay_ms, expected
        );
    }

    /// Feature: standx-maker-bot, Property 13: Exponential Backoff Monotonicity
    /// For any two consecutive attempts, delay should be non-decreasing until max
    #[test]
    fn prop_exponential_backoff_monotonic(
        attempt in 0u32..19,
        base_delay_ms in 100u64..5000,
        max_delay_ms in 5000u64..60000,
    ) {
        let max_delay_ms = max_delay_ms.max(base_delay_ms);

        let config = create_retry_config(base_delay_ms, max_delay_ms);

        let delay_n = config.calculate_delay(attempt);
        let delay_n_plus_1 = config.calculate_delay(attempt + 1);

        // Delay should be non-decreasing
        prop_assert!(
            delay_n_plus_1 >= delay_n,
            "Delay should be non-decreasing: attempt {} = {}, attempt {} = {}",
            attempt, delay_n, attempt + 1, delay_n_plus_1
        );
    }

    /// Feature: standx-maker-bot, Property 13: Delay Never Exceeds Max
    /// For any attempt, delay should never exceed max_delay_ms
    #[test]
    fn prop_delay_never_exceeds_max(
        attempt in 0u32..100,
        base_delay_ms in 1u64..10000,
        max_delay_ms in 1u64..60000,
    ) {
        let config = create_retry_config(base_delay_ms, max_delay_ms);

        let delay = config.calculate_delay(attempt);

        // Delay should never exceed max_delay_ms
        prop_assert!(
            delay <= max_delay_ms,
            "Delay {} should not exceed max_delay {}",
            delay, max_delay_ms
        );

        // Delay should also be at least base_delay for attempt 0
        if attempt == 0 {
            prop_assert!(
                delay >= base_delay_ms.min(max_delay_ms),
                "Delay {} should be at least min(base_delay, max_delay) = {}",
                delay, base_delay_ms.min(max_delay_ms)
            );
        }
    }
}

// ============================================================================
// Property 14: Retryable Status Codes
// **Validates: Requirements 10.3**
//
// For any HTTP status code, is_retryable should return true if and only if
// the code is in {429, 500, 502, 503, 504}.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Feature: standx-maker-bot, Property 14: Retryable Status Codes
    /// is_retryable returns true iff status is in {429, 500, 502, 503, 504}
    #[test]
    fn prop_retryable_status_codes(status in 100u16..600) {
        let config = RetryConfig::default();
        let is_retryable = config.is_retryable_status(status);

        let expected_retryable = matches!(status, 429 | 500 | 502 | 503 | 504);

        prop_assert_eq!(
            is_retryable, expected_retryable,
            "Status {} should be retryable={}, got {}",
            status, expected_retryable, is_retryable
        );
    }

    /// Feature: standx-maker-bot, Property 14: 4xx Errors Not Retryable (except 429)
    /// All 4xx errors except 429 should not be retryable
    #[test]
    fn prop_4xx_not_retryable_except_429(status in 400u16..500) {
        let config = RetryConfig::default();
        let is_retryable = config.is_retryable_status(status);

        if status == 429 {
            prop_assert!(is_retryable, "429 should be retryable");
        } else {
            prop_assert!(!is_retryable, "4xx status {} should not be retryable", status);
        }
    }

    /// Feature: standx-maker-bot, Property 14: Success Codes Not Retryable
    /// All 2xx success codes should not be retryable
    #[test]
    fn prop_2xx_not_retryable(status in 200u16..300) {
        let config = RetryConfig::default();
        let is_retryable = config.is_retryable_status(status);

        prop_assert!(!is_retryable, "2xx status {} should not be retryable", status);
    }
}

// ============================================================================
// Unit tests for specific edge cases
// ============================================================================

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_specific_retryable_codes() {
        let config = RetryConfig::default();

        // All retryable codes
        assert!(config.is_retryable_status(429), "429 should be retryable");
        assert!(config.is_retryable_status(500), "500 should be retryable");
        assert!(config.is_retryable_status(502), "502 should be retryable");
        assert!(config.is_retryable_status(503), "503 should be retryable");
        assert!(config.is_retryable_status(504), "504 should be retryable");

        // Non-retryable codes
        assert!(!config.is_retryable_status(200), "200 should not be retryable");
        assert!(!config.is_retryable_status(400), "400 should not be retryable");
        assert!(!config.is_retryable_status(401), "401 should not be retryable");
        assert!(!config.is_retryable_status(403), "403 should not be retryable");
        assert!(!config.is_retryable_status(404), "404 should not be retryable");
        assert!(!config.is_retryable_status(501), "501 should not be retryable");
    }

    #[test]
    fn test_exponential_backoff_sequence() {
        let config = create_retry_config(1000, 30000);

        // Expected sequence: 1000, 2000, 4000, 8000, 16000, 30000 (capped)
        assert_eq!(config.calculate_delay(0), 1000);
        assert_eq!(config.calculate_delay(1), 2000);
        assert_eq!(config.calculate_delay(2), 4000);
        assert_eq!(config.calculate_delay(3), 8000);
        assert_eq!(config.calculate_delay(4), 16000);
        assert_eq!(config.calculate_delay(5), 30000); // Capped at max
        assert_eq!(config.calculate_delay(6), 30000); // Still capped
    }

    #[test]
    fn test_delay_with_small_max() {
        let config = create_retry_config(1000, 1500);

        assert_eq!(config.calculate_delay(0), 1000);
        assert_eq!(config.calculate_delay(1), 1500); // Capped immediately
        assert_eq!(config.calculate_delay(2), 1500);
    }
}


// ============================================================================
// Property 15: Rate Limiter Timing
// **Validates: Requirements 10.5**
//
// For any sequence of N requests through the rate limiter with 
// requests_per_second = R, the total time should be at least (N-1) / R seconds.
// ============================================================================

#[cfg(test)]
mod rate_limiter_tests {
    use standx_mm::http_client::RateLimiter;
    use std::time::Instant;

    /// Feature: standx-maker-bot, Property 15: Rate Limiter Timing
    /// For N requests at R requests/second, total time >= (N-1)/R seconds
    #[tokio::test]
    async fn test_rate_limiter_timing_property() {
        // Use a high rate to keep test fast but still measurable
        let requests_per_second = 100.0; // 10ms interval
        let mut limiter = RateLimiter::new(requests_per_second);
        
        let num_requests = 5;
        let start = Instant::now();
        
        for _ in 0..num_requests {
            limiter.wait().await;
        }
        
        let elapsed = start.elapsed();
        
        // Expected minimum time: (N-1) / R seconds
        // For 5 requests at 100/s: (5-1) / 100 = 0.04 seconds = 40ms
        let expected_min_ms = ((num_requests - 1) as f64 / requests_per_second * 1000.0) as u64;
        
        // Allow some tolerance for timing variations (subtract 5ms)
        let tolerance_ms = 5;
        let actual_ms = elapsed.as_millis() as u64;
        
        assert!(
            actual_ms >= expected_min_ms.saturating_sub(tolerance_ms),
            "Total time {}ms should be at least {}ms (with {}ms tolerance) for {} requests at {}/s",
            actual_ms, expected_min_ms, tolerance_ms, num_requests, requests_per_second
        );
    }

    /// Feature: standx-maker-bot, Property 15: Rate Limiter First Request No Wait
    /// The first request should not wait
    #[tokio::test]
    async fn test_rate_limiter_first_request_no_wait() {
        let mut limiter = RateLimiter::new(10.0);
        
        let start = Instant::now();
        let waited = limiter.wait().await;
        let elapsed = start.elapsed();
        
        // First request should not wait
        assert_eq!(waited, 0, "First request should not wait");
        assert!(
            elapsed.as_millis() < 10,
            "First request should complete quickly, took {}ms",
            elapsed.as_millis()
        );
    }

    /// Feature: standx-maker-bot, Property 15: Rate Limiter Enforces Interval
    /// Consecutive requests should be spaced by at least 1/R seconds
    #[tokio::test]
    async fn test_rate_limiter_enforces_interval() {
        let requests_per_second = 50.0; // 20ms interval
        let mut limiter = RateLimiter::new(requests_per_second);
        
        // First request
        limiter.wait().await;
        
        // Second request immediately - should wait
        let start = Instant::now();
        limiter.wait().await;
        let elapsed = start.elapsed();
        
        // Should have waited approximately 20ms (allow 5ms tolerance)
        let expected_interval_ms = (1000.0 / requests_per_second) as u64;
        let actual_ms = elapsed.as_millis() as u64;
        
        // Allow some tolerance for timing variations
        assert!(
            actual_ms >= expected_interval_ms.saturating_sub(5),
            "Second request should wait at least {}ms, waited {}ms",
            expected_interval_ms - 5, actual_ms
        );
    }

    /// Feature: standx-maker-bot, Property 15: Rate Limiter Respects Natural Spacing
    /// If requests are naturally spaced, no additional wait is needed
    #[tokio::test]
    async fn test_rate_limiter_respects_natural_spacing() {
        let requests_per_second = 50.0; // 20ms interval
        let mut limiter = RateLimiter::new(requests_per_second);
        
        // First request
        limiter.wait().await;
        
        // Wait longer than the interval
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
        
        // Second request should not need to wait
        let start = Instant::now();
        let waited = limiter.wait().await;
        let elapsed = start.elapsed();
        
        assert_eq!(waited, 0, "Should not wait when naturally spaced");
        assert!(
            elapsed.as_millis() < 5,
            "Should complete quickly when naturally spaced, took {}ms",
            elapsed.as_millis()
        );
    }

    /// Feature: standx-maker-bot, Property 15: Rate Limiter Configuration
    /// Rate limiter should correctly report its configuration
    #[test]
    fn test_rate_limiter_configuration() {
        let limiter = RateLimiter::new(25.0);
        assert_eq!(limiter.requests_per_second(), 25.0);
        
        let default_limiter = RateLimiter::default();
        assert_eq!(default_limiter.requests_per_second(), 10.0);
    }
}
