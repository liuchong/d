use crate::fiber::{FiberId, FiberState};

pub(crate) const MAX_ERROR_SUMMARY_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub fibers: Vec<FiberSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiberSnapshot {
    pub id: FiberId,
    pub name: String,
    pub state: FiberState,
    pub dependencies: Vec<&'static str>,
    pub provided: Vec<&'static str>,
    pub effect_count: usize,
    pub error_summary: Option<String>,
}

pub(crate) fn truncate_summary(msg: &str) -> String {
    if msg.len() <= MAX_ERROR_SUMMARY_BYTES {
        return msg.to_string();
    }
    let mut end = MAX_ERROR_SUMMARY_BYTES.saturating_sub(3);
    while end > 0 && !msg.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = msg[..end].to_string();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_summary_respects_utf8_boundaries() {
        let s = "中".repeat(200);
        let out = truncate_summary(&s);
        assert!(out.len() <= MAX_ERROR_SUMMARY_BYTES);
        assert!(out.ends_with("..."));
    }
}
