//! How long a web sign-in (`POST /auth/login`) lasts. API tokens are not affected.

/// Limits for web sign-ins, in minutes; 0 turns a limit off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    /// A sign-in ends after this long without a request (`PDM_SESSION_IDLE_MINUTES`).
    pub idle_minutes: i32,
    /// A sign-in ends this long after it was made, however busy (`PDM_SESSION_LIFETIME_MINUTES`).
    pub lifetime_minutes: i32,
}

impl Default for SessionLimits {
    /// Two idle hours; twelve hours in all, so a working day needs one sign-in.
    fn default() -> Self {
        Self {
            idle_minutes: 120,
            lifetime_minutes: 720,
        }
    }
}

impl SessionLimits {
    /// Read `PDM_SESSION_IDLE_MINUTES` and `PDM_SESSION_LIFETIME_MINUTES`.
    pub fn from_env() -> Self {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// A value that is missing, not a whole number or negative keeps the default: a typo must
    /// not turn expiry off.
    fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let read = |key: &str, default: i32| {
            get(key)
                .and_then(|v| v.trim().parse::<i32>().ok())
                .filter(|n| *n >= 0)
                .unwrap_or(default)
        };
        Self {
            idle_minutes: read("PDM_SESSION_IDLE_MINUTES", d.idle_minutes),
            lifetime_minutes: read("PDM_SESSION_LIFETIME_MINUTES", d.lifetime_minutes),
        }
    }

    /// Whether any limit is on.
    pub fn any(&self) -> bool {
        self.idle_minutes > 0 || self.lifetime_minutes > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(vars: &[(&str, &str)]) -> SessionLimits {
        SessionLimits::from_vars(|k| {
            vars.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        })
    }

    #[test]
    fn defaults_when_unset() {
        assert_eq!(limits(&[]), SessionLimits::default());
        assert!(SessionLimits::default().any());
    }

    #[test]
    fn reads_both_values() {
        let l = limits(&[
            ("PDM_SESSION_IDLE_MINUTES", "15"),
            ("PDM_SESSION_LIFETIME_MINUTES", " 480 "),
        ]);
        assert_eq!(l.idle_minutes, 15);
        assert_eq!(l.lifetime_minutes, 480);
    }

    #[test]
    fn zero_turns_a_limit_off() {
        let l = limits(&[
            ("PDM_SESSION_IDLE_MINUTES", "0"),
            ("PDM_SESSION_LIFETIME_MINUTES", "0"),
        ]);
        assert_eq!(l.idle_minutes, 0);
        assert_eq!(l.lifetime_minutes, 0);
        assert!(!l.any());
    }

    #[test]
    fn bad_values_keep_the_default() {
        let l = limits(&[
            ("PDM_SESSION_IDLE_MINUTES", "2h"),
            ("PDM_SESSION_LIFETIME_MINUTES", "-5"),
        ]);
        assert_eq!(l, SessionLimits::default());
    }
}
