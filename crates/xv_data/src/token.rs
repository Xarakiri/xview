/// Identifies one load request; compare against the guard to drop stale results.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadToken(u64);

#[derive(Debug, Default)]
pub struct LoadGuard {
    current: u64,
}

impl LoadGuard {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::should_implement_trait)]
    /// Issues a new token, invalidating all earlier ones.
    pub fn next(&mut self) -> LoadToken {
        self.current += 1;
        LoadToken(self.current)
    }

    pub fn is_current(&self, token: &LoadToken) -> bool {
        token.0 == self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_token_is_rejected() {
        let mut g = LoadGuard::new();
        let t1 = g.next();
        let t2 = g.next();
        assert!(!g.is_current(&t1));
        assert!(g.is_current(&t2));
    }
}
