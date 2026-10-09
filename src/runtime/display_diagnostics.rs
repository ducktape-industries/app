//! Warns once per guest when this host's sanitizer cut its frame: what was
//! cut and where the first cut fell. A view's own tests fail on any cut,
//! so one seen here is a view that was not tested against this SDK.
use super::*;

#[derive(Default)]
pub(super) struct DisplayDiagnostics {
    warned: bool,
}
impl DisplayDiagnostics {
    /// Whether `cuts` is the first cut seen for this guest.
    pub(super) fn first_cut(&mut self, cuts: &wire::SanitizeReport) -> bool {
        let first = !cuts.is_empty() && !self.warned;
        self.warned |= first;
        first
    }
}
