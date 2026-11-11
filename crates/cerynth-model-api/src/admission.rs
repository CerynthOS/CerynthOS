//! Priority/FIFO admission with ancestor concurrency limits and cancellation-safe leases.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AdmissionQueue {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<State>,
    changed: Notify,
    limit: usize,
}
#[derive(Default)]
struct State {
    active: usize,
    counters: HashMap<String, usize>,
    waiting: Vec<Ticket>,
    next: u64,
    priority_bypasses: usize,
}
struct Ticket {
    id: u64,
    priority: i32,
    limits: Vec<(String, usize)>,
}

pub struct AdmissionPermit {
    inner: Arc<Inner>,
    limits: Vec<String>,
    pub queue_ms: f64,
}
struct Waiting {
    inner: Arc<Inner>,
    id: u64,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .waiting
            .retain(|t| t.id != self.id);
        self.inner.changed.notify_waiters();
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        for key in &self.limits {
            if let Some(count) = state.counters.get_mut(key) {
                *count -= 1;
                if *count == 0 {
                    state.counters.remove(key);
                }
            }
        }
        drop(state);
        self.inner.changed.notify_waiters();
    }
}

impl AdmissionQueue {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                changed: Notify::new(),
                limit,
            }),
        }
    }

    /// Limits must come from trusted session grants; include parent/project IDs.
    ///
    /// # Errors
    /// Returns an error if the queue is full, disabled, or the hierarchy is invalid.
    pub async fn acquire(
        &self,
        priority: i32,
        limits: Vec<(String, usize)>,
    ) -> Result<AdmissionPermit, String> {
        if self.inner.limit == 0
            || limits.is_empty()
            || limits.iter().any(|(_, limit)| *limit == 0)
            || limits.len() > 32
        {
            return Err("inference admission is disabled or limits are invalid".into());
        }
        let mut keys: Vec<_> = limits.iter().map(|(key, _)| key).collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.len() != limits.len() {
            return Err("duplicate hierarchy identity".into());
        }
        let start = Instant::now();
        let id = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.waiting.len() >= 128 {
                return Err("inference queue is full".into());
            }
            state.next += 1;
            let id = state.next;
            state.waiting.push(Ticket {
                id,
                priority,
                limits: limits.clone(),
            });
            id
        };
        let _waiting = Waiting {
            inner: self.inner.clone(),
            id,
        };
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let eligible: Vec<_> = state
                    .waiting
                    .iter()
                    .filter(|ticket| {
                        ticket.limits.iter().all(|(key, limit)| {
                            state.counters.get(key).copied().unwrap_or(0) < *limit
                        })
                    })
                    .collect();
                let oldest = eligible.iter().map(|ticket| ticket.id).min();
                let next = if state.priority_bypasses >= 8 {
                    oldest
                } else {
                    eligible
                        .iter()
                        .min_by_key(|ticket| (std::cmp::Reverse(ticket.priority), ticket.id))
                        .map(|ticket| ticket.id)
                };
                if state.active < self.inner.limit && next == Some(id) {
                    state.priority_bypasses = if oldest == Some(id) {
                        0
                    } else {
                        state.priority_bypasses + 1
                    };
                    state.waiting.retain(|ticket| ticket.id != id);
                    state.active += 1;
                    for (key, _) in &limits {
                        *state.counters.entry(key.clone()).or_default() += 1;
                    }
                    drop(state);
                    self.inner.changed.notify_waiters();
                    return Ok(AdmissionPermit {
                        inner: self.inner.clone(),
                        limits: limits.into_iter().map(|(k, _)| k).collect(),
                        queue_ms: start.elapsed().as_secs_f64() * 1000.0,
                    });
                }
            }
            changed.await;
        }
    }

    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.active, state.waiting.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hierarchical_limits_priority_and_cancelled_waiters() {
        let queue = AdmissionQueue::new(1);
        let first = queue.acquire(0, vec![("parent".into(), 1)]).await.unwrap();
        let low_queue = queue.clone();
        let low = tokio::spawn(async move {
            low_queue
                .acquire(1, vec![("parent".into(), 1)])
                .await
                .unwrap()
        });
        let high_queue = queue.clone();
        let high = tokio::spawn(async move {
            high_queue
                .acquire(10, vec![("parent".into(), 1)])
                .await
                .unwrap()
        });
        while queue.counts().1 < 2 {
            tokio::task::yield_now().await;
        }
        drop(first);
        let high_permit = tokio::time::timeout(std::time::Duration::from_secs(1), high)
            .await
            .unwrap()
            .unwrap();
        assert!(!low.is_finished());
        low.abort();
        let _ = low.await;
        assert_eq!(queue.counts(), (1, 0));
        drop(high_permit);
        assert_eq!(queue.counts(), (0, 0));
    }

    #[tokio::test]
    async fn old_eligible_work_is_not_starved_by_a_stream_of_higher_priorities() {
        let queue = AdmissionQueue::new(1);
        let held = queue.acquire(0, vec![("root".into(), 1)]).await.unwrap();
        let low_queue = queue.clone();
        let (send, mut receive) = tokio::sync::mpsc::channel(16);
        let low_send = send.clone();
        let low = tokio::spawn(async move {
            let _permit = low_queue
                .acquire(-100, vec![("root".into(), 1)])
                .await
                .unwrap();
            low_send.send("low").await.unwrap();
        });
        while queue.counts().1 != 1 {
            tokio::task::yield_now().await;
        }
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let next = queue.clone();
            let send = send.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = next.acquire(100, vec![("root".into(), 1)]).await.unwrap();
                send.send("high").await.unwrap();
            }));
        }
        while queue.counts().1 != 13 {
            tokio::task::yield_now().await;
        }
        drop(held);
        let mut position = 0;
        loop {
            let value = tokio::time::timeout(std::time::Duration::from_secs(2), receive.recv())
                .await
                .unwrap()
                .unwrap();
            if value == "low" {
                break;
            }
            position += 1;
        }
        assert!(position <= 8, "low priority work was starved");
        low.await.unwrap();
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(queue.counts(), (0, 0));
    }
}
