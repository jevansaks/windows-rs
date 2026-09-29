use std::fmt;
use std::sync::OnceLock;
use std::time::Instant;

const ENV: &str = "WINDOWS_CLANG_TIMINGS";

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| enabled_value(std::env::var_os(ENV).as_deref()))
}

fn enabled_value(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

pub(crate) struct Timer(Option<Instant>);

impl Timer {
    pub(crate) fn start() -> Self {
        Self(enabled().then(Instant::now))
    }

    pub(crate) fn report(self, phase: &str, fields: fmt::Arguments<'_>) {
        if let Some(start) = self.0 {
            eprintln!(
                "WINDOWS_CLANG_TIMING phase={phase} elapsed_ms={:.3} {fields}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn timings_require_exact_opt_in() {
        assert!(enabled_value(Some(OsStr::new("1"))));
        assert!(!enabled_value(None));
        assert!(!enabled_value(Some(OsStr::new(""))));
        assert!(!enabled_value(Some(OsStr::new("0"))));
        assert!(!enabled_value(Some(OsStr::new("true"))));
    }
}
