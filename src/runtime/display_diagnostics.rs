//! Warns once per guest when display text was truncated, kept apart by who
//! cut it: this host's sanitizer (`local`, the "host" origin) or the view
//! itself, which reports its own cuts in the frame (`upstream`,
//! "producer-reported"). Each origin is logged the first time it is seen.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FrameReports {
    pub(super) local: wire::SanitizeReport,
    pub(super) upstream: wire::SanitizeReport,
}
impl FrameReports {
    pub(super) fn inherit(&mut self, held: Self) {
        self.local.merge(held.local);
        self.upstream.merge(held.upstream);
    }
}
#[derive(Default)]
pub(super) struct DisplayDiagnostics {
    seen: FrameReports,
}
impl DisplayDiagnostics {
    pub(super) fn observe(&mut self, reports: FrameReports) -> [Option<&'static str>; 2] {
        let mut origins = [None, None];
        if reports.local.display_text_truncated && !self.seen.local.display_text_truncated {
            origins[0] = Some("host");
        }
        if reports.upstream.display_text_truncated && !self.seen.upstream.display_text_truncated {
            origins[1] = Some("producer-reported");
        }
        self.seen.inherit(reports);
        origins
    }
}
