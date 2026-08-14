//! The one panic chokepoint every driver window runs through: it catches
//! the unwind, names the site the panic came from, and hands back a
//! `NO-PANIC` `Violation` the session can record like any other.
//!
//! The hook installed here CHAINS to whatever hook was already in place
//! (libtest's, or the default one) instead of replacing it — the stderr
//! `panicked at` line an operator reads is evidence in its own right and
//! must survive being caught.

use std::any::Any;
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use crate::invariant::Violation;

/// Where a caught panic came from, as the panic hook saw it — the caught
/// payload alone names only the assertion text, never its producer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanicSite {
    pub location: String,
    pub backtrace: String,
}

const UNKNOWN_LOCATION: &str = "<unknown location>";

thread_local! {
    static LAST_SITE: RefCell<Option<PanicSite>> = const { RefCell::new(None) };
}

static CHAIN_HOOK: Once = Once::new();

fn install_chained_hook() {
    CHAIN_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let site = PanicSite {
                location: info
                    .location()
                    .map_or_else(|| UNKNOWN_LOCATION.to_string(), ToString::to_string),
                backtrace: Backtrace::force_capture().to_string(),
            };
            LAST_SITE.with(|slot| slot.replace(Some(site)));
            previous(info);
        }));
    });
}

/// Runs `f` under `catch_unwind`, turning any panic into the `NO-PANIC`
/// violation the session records — with the location and backtrace the
/// chained hook captured on the way out.
pub fn catching_panic<T>(f: impl FnOnce() -> T) -> Result<T, Violation> {
    install_chained_hook();
    LAST_SITE.with(|slot| slot.replace(None));
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) => Err(Violation::panicked(
            downcast_panic(payload),
            LAST_SITE.with(std::cell::RefCell::take),
        )),
    }
}

/// The same downcast ladder proptest itself uses to render a caught panic's
/// payload.
fn downcast_panic(payload: Box<dyn Any + Send>) -> String {
    let payload = match payload.downcast::<&str>() {
        Ok(s) => return (*s).to_string(),
        Err(payload) => payload,
    };
    payload
        .downcast::<String>()
        .map_or_else(|_| "<unknown panic value>".to_string(), |s| *s)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn formatted_panic_message_survives_violation_rendering() {
        let violation = catching_panic(|| panic!("caught formatted panic {}", 42))
            .expect_err("the closure must panic");
        assert_eq!(violation.message, "caught formatted panic 42");
        assert_eq!(violation.id, "NO-PANIC");
    }

    #[test]
    fn literal_panic_message_survives_violation_rendering() {
        let violation =
            catching_panic(|| panic!("caught literal panic")).expect_err("the closure must panic");
        assert_eq!(violation.message, "caught literal panic");
    }

    #[test]
    fn caught_panic_names_the_file_and_line_it_came_from() {
        let violation =
            catching_panic(|| panic!("located panic")).expect_err("the closure must panic");
        let site = violation.site.expect("a caught panic carries its site");
        assert!(
            site.location.contains("src/guard.rs"),
            "location must name this file, got {:?}",
            site.location
        );
        assert!(
            !site.backtrace.is_empty(),
            "a caught panic carries a backtrace"
        );
    }

    #[test]
    fn a_clean_run_returns_its_value_and_leaves_no_site_behind() {
        assert_eq!(catching_panic(|| 7), Ok(7));
        let violation =
            catching_panic(|| panic!("second panic")).expect_err("the closure must panic");
        assert_eq!(violation.message, "second panic");
    }
}
