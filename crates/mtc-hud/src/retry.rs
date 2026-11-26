//! How long to wait before trying a failed download again (map pictures, heightmaps, ammunition
//! data). From mainland China the map service (Cloudflare) timed out on 23–33 % of China Unicom
//! test nodes and the wiki data on 6 % of all, so failed downloads are retried instead of waiting
//! for the next start: soon a few times, then every ten minutes while it keeps failing, in the
//! background.
use std::time::Duration;

const SOON_S: [u64; 3] = [5, 15, 60];
const THEN_S: u64 = 600;

/// The wait before retry number `attempt` (0: the first retry).
pub fn delay(attempt: usize) -> Duration {
    Duration::from_secs(SOON_S.get(attempt).copied().unwrap_or(THEN_S))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soon_then_every_ten_minutes() {
        let waits: Vec<u64> = (0..5).map(|a| delay(a).as_secs()).collect();
        assert_eq!(waits, [5, 15, 60, 600, 600]);
    }
}
