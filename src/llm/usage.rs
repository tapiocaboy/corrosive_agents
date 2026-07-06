//! Token-usage accounting hooks.
//!
//! Every non-streaming completion an [`Agent`](crate::agent::Agent) makes is
//! reported to an optional [`UsageObserver`]
//! ([`AgentBuilder::usage_observer`](crate::agent::AgentBuilder::usage_observer))
//! and always accumulated into the agent's built-in totals
//! ([`Agent::usage`](crate::agent::Agent::usage)).

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::llm::Usage;

/// One completion's worth of token accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Session that triggered the completion.
    pub session_id: String,
    /// Model that served it.
    pub model: String,
    /// Tokens consumed.
    pub usage: Usage,
}

/// Receives a [`UsageEvent`] after every completion — implement to feed
/// metrics, billing, or rate-limit dashboards.
pub trait UsageObserver: Send + Sync {
    /// Called after each successful completion that reported usage.
    fn on_usage(&self, event: &UsageEvent);
}

/// Call an `Fn(&UsageEvent)` closure as an observer.
impl<F: Fn(&UsageEvent) + Send + Sync> UsageObserver for F {
    fn on_usage(&self, event: &UsageEvent) {
        self(event)
    }
}

/// Lock-free running totals across all sessions (the agent keeps one).
#[derive(Debug, Default)]
pub struct UsageTotals {
    requests: AtomicU64,
    prompt_tokens: AtomicU64,
    completion_tokens: AtomicU64,
    total_tokens: AtomicU64,
}

/// A point-in-time copy of [`UsageTotals`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    /// Number of completions that reported usage.
    pub requests: u64,
    /// Cumulative prompt tokens.
    pub prompt_tokens: u64,
    /// Cumulative completion tokens.
    pub completion_tokens: u64,
    /// Cumulative total tokens.
    pub total_tokens: u64,
}

impl UsageTotals {
    /// Add one event to the totals.
    pub fn record(&self, event: &UsageEvent) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.prompt_tokens
            .fetch_add(u64::from(event.usage.prompt_tokens), Ordering::Relaxed);
        self.completion_tokens
            .fetch_add(u64::from(event.usage.completion_tokens), Ordering::Relaxed);
        self.total_tokens
            .fetch_add(u64::from(event.usage.total_tokens), Ordering::Relaxed);
    }

    /// A consistent-enough copy of the counters.
    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            prompt_tokens: self.prompt_tokens.load(Ordering::Relaxed),
            completion_tokens: self.completion_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
        }
    }
}

impl UsageObserver for UsageTotals {
    fn on_usage(&self, event: &UsageEvent) {
        self.record(event);
    }
}
