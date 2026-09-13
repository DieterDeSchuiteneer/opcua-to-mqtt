use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

/// Allow/block list matched against MQTT topics. Block always wins over
/// allow; an empty (or absent) allow list means "allow everything not
/// explicitly blocked".
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FilterConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub block: Vec<String>,
}

pub struct TopicFilter {
    allow: Option<GlobSet>,
    block: GlobSet,
}

impl TopicFilter {
    pub fn build(cfg: &FilterConfig) -> anyhow::Result<Self> {
        let allow = if cfg.allow.is_empty() {
            None
        } else {
            Some(build_globset(&cfg.allow)?)
        };
        let block = build_globset(&cfg.block)?;
        Ok(Self { allow, block })
    }

    /// Whether a topic is permitted to cross the bridge.
    pub fn is_allowed(&self, topic: &str) -> bool {
        if self.block.is_match(topic) {
            return false;
        }
        match &self.allow {
            None => true,
            Some(allow) => allow.is_match(topic),
        }
    }
}

fn build_globset(patterns: &[String]) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(allow: &[&str], block: &[&str]) -> TopicFilter {
        TopicFilter::build(&FilterConfig {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            block: block.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap()
    }

    #[test]
    fn empty_lists_allow_everything() {
        let f = filter(&[], &[]);
        assert!(f.is_allowed("plant/line1/tag1"));
        assert!(f.is_allowed("anything/at/all"));
    }

    #[test]
    fn allow_list_restricts_to_matches() {
        let f = filter(&["plant/line1/*"], &[]);
        assert!(f.is_allowed("plant/line1/tag1"));
        assert!(!f.is_allowed("plant/line2/tag1"));
    }

    #[test]
    fn block_removes_from_allow_all() {
        let f = filter(&[], &["plant/line1/tag_debug"]);
        assert!(f.is_allowed("plant/line1/tag1"));
        assert!(!f.is_allowed("plant/line1/tag_debug"));
    }

    #[test]
    fn block_wins_over_allow() {
        let f = filter(&["plant/line1/*"], &["plant/line1/tag_debug"]);
        assert!(f.is_allowed("plant/line1/tag1"));
        assert!(!f.is_allowed("plant/line1/tag_debug"));
    }

    #[test]
    fn glob_double_star_matches_across_segments() {
        let f = filter(&["plant/**"], &[]);
        assert!(f.is_allowed("plant/line1/cell2/tag1"));
    }

    #[test]
    fn single_star_does_not_cross_segments() {
        let f = filter(&["plant/*"], &[]);
        assert!(!f.is_allowed("plant/line1/tag1"));
        assert!(f.is_allowed("plant/line1"));
    }
}
