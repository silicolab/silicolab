//! Error classification + bounded exponential backoff for the agent loop.
//!
//! An agent hammers the endpoint across many turns and *will* hit 429/529/5xx.
//! One-shot callers (`pdb_fetch`, `update_check`) don't need this; the loop does.
//! Terminal classes (400/401/403) are never retried — they would just fail again.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use super::provider::LlmProvider;
use super::types::{AssistantTurn, ChatMessage, LlmConfig, LlmError, StreamEvent, ToolDef};

/// Maximum retry attempts after the first try (so up to `MAX_RETRIES + 1` calls).
pub const MAX_RETRIES: u32 = 5;

/// Base backoff; doubled each attempt and capped by [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_millis(800);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Run one model turn with bounded exponential backoff on transient errors,
/// honoring a server `Retry-After` and aborting promptly when `cancel` is set.
pub fn complete_with_retry(
    provider: &dyn LlmProvider,
    cfg: &LlmConfig,
    tools: &[ToolDef],
    history: &[ChatMessage],
    cancel: &Arc<AtomicBool>,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<AssistantTurn, LlmError> {
    let mut attempt: u32 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }
        match provider.complete(cfg, tools, history, cancel, on_event) {
            Ok(turn) => return Ok(turn),
            Err(error) => {
                if !error.is_retryable() || attempt >= MAX_RETRIES {
                    return Err(error);
                }
                let delay = backoff_delay(attempt, &error);
                if sleep_with_cancel(delay, cancel) {
                    return Err(LlmError::Cancelled);
                }
                attempt += 1;
            }
        }
    }
}

/// The wait before the next attempt: a server-provided `Retry-After` if present
/// (clamped to [`MAX_BACKOFF`]), otherwise exponential backoff from the base.
fn backoff_delay(attempt: u32, error: &LlmError) -> Duration {
    if let LlmError::RateLimited {
        retry_after: Some(after),
    } = error
    {
        return (*after).min(MAX_BACKOFF);
    }
    let scaled = BASE_BACKOFF.saturating_mul(1u32 << attempt.min(5));
    scaled.min(MAX_BACKOFF)
}

/// Sleep up to `delay`, waking every 100 ms to check `cancel`. Returns `true`
/// if cancellation was requested before the wait elapsed.
fn sleep_with_cancel(delay: Duration, cancel: &Arc<AtomicBool>) -> bool {
    let step = Duration::from_millis(100);
    let mut remaining = delay;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let chunk = remaining.min(step);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
    cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_prefers_server_retry_after() {
        let error = LlmError::RateLimited {
            retry_after: Some(Duration::from_secs(3)),
        };
        assert_eq!(backoff_delay(0, &error), Duration::from_secs(3));
    }

    #[test]
    fn retry_after_is_clamped_to_max() {
        let error = LlmError::RateLimited {
            retry_after: Some(Duration::from_secs(600)),
        };
        assert_eq!(backoff_delay(0, &error), MAX_BACKOFF);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_delay(0, &LlmError::Overloaded), BASE_BACKOFF);
        assert_eq!(backoff_delay(1, &LlmError::Overloaded), BASE_BACKOFF * 2);
        // The shift is capped at <<5, so the effective ceiling is base*32 — well
        // under MAX_BACKOFF, which only guards against shift overflow. The delay
        // plateaus there rather than climbing to 30s.
        let plateau = BASE_BACKOFF * 32;
        assert!(plateau <= MAX_BACKOFF);
        assert_eq!(backoff_delay(5, &LlmError::Overloaded), plateau);
        assert_eq!(backoff_delay(10, &LlmError::Overloaded), plateau);
    }

    #[test]
    fn already_cancelled_sleep_returns_immediately() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(sleep_with_cancel(Duration::from_secs(5), &cancel));
    }
}
