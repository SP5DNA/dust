/// Fine-grained poll for playback/confirm waits. Keeps Morse→input latency low
/// without spinning.
pub const POLL_MS: u32 = 16;

pub async fn sleep_ms(ms: u32) {
    let ms = ms.max(1);
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
    }
}

pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Monotonic milliseconds, for anything timed against itself: the paddle
/// keyer, and the session's response times, which a wall clock stepped back
/// would turn negative.
///
/// Natively this is Tokio's clock, which is the system's monotonic clock in
/// the app and the paused, hand-advanced one in tests, so a test's session
/// is timed by the same clock its timers run on. A browser has its own
/// monotonic clock in `performance.now()`.
pub fn mono_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        CLOCK_ORIGIN.with(|origin| {
            let start = origin.get().unwrap_or_else(|| {
                let now = tokio::time::Instant::now();
                origin.set(Some(now));
                now
            });
            tokio::time::Instant::now()
                .saturating_duration_since(start)
                .as_millis() as u64
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Every page has a window; the wall clock is only for a context
        // without one, which this app never runs in.
        web_sys::window()
            .and_then(|window| window.performance())
            .map_or_else(now_ms, |performance| performance.now().max(0.0) as u64)
    }
}

#[cfg(not(target_arch = "wasm32"))]
std::thread_local! {
    static CLOCK_ORIGIN: std::cell::Cell<Option<tokio::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Start the monotonic clock over. Each test runtime must call this so a
/// previous test's origin is not reused.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn reset_mono_clock() {
    CLOCK_ORIGIN.with(|origin| origin.set(None));
}

pub fn local_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// A seed nobody else has: fresh entropy from the platform.
///
/// Every launch of the app, and every session in one, draws its stations and
/// its groups from one of these. A clock reading would hand the same seed to
/// two launches in the same millisecond, and a counter the same seed to the
/// first session of every launch.
pub fn seed_rng() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        // The browser build's std has no entropy source; the page has one.
        let word = || (js_sys::Math::random() * 4_294_967_296.0) as u64;
        ((word() << 32) | word()) ^ now_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, Hasher};
        // Each `RandomState` is keyed afresh from the operating system's
        // random source, so hashing anything with it gives a new seed.
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(now_ms());
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    /// Outside any runtime too: the clock runs forward, never back.
    #[test]
    fn the_monotonic_clock_never_runs_back() {
        let mut last = super::mono_ms();
        for _ in 0..1_000 {
            let now = super::mono_ms();
            assert!(now >= last);
            last = now;
        }
    }

    #[test]
    fn no_two_seeds_are_the_same() {
        let seeds: std::collections::BTreeSet<u64> = (0..64).map(|_| super::seed_rng()).collect();
        assert_eq!(seeds.len(), 64, "a seed repeated within one launch");
    }
}
