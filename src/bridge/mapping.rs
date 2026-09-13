use std::collections::HashSet;

use anyhow::{bail, Result};

use crate::config::{Direction, FilterConfig, Mapping};
use crate::config::filter::TopicFilter;

#[derive(Debug, Clone, PartialEq)]
pub struct NodeMapping {
    pub node_id: String,
    pub topic: String,
}

/// The effective set of mappings after applying the allow/block list,
/// split by direction so the OPC UA and MQTT sides only see the half they
/// need (subscribe-for-read vs subscribe-for-write).
#[derive(Debug, Default)]
pub struct MappingTable {
    /// OPC UA -> MQTT
    pub read: Vec<NodeMapping>,
    /// MQTT -> OPC UA
    pub write: Vec<NodeMapping>,
}

pub fn build(mappings: &[Mapping], filters: &FilterConfig) -> Result<MappingTable> {
    let filter = TopicFilter::build(filters)?;
    let mut table = MappingTable::default();
    let mut seen_read_topics = HashSet::new();
    let mut seen_write_topics = HashSet::new();

    for m in mappings {
        if !filter.is_allowed(&m.topic) {
            continue;
        }

        if m.direction.is_read() {
            if !seen_read_topics.insert(m.topic.clone()) {
                bail!("duplicate read mapping for topic '{}'", m.topic);
            }
            table.read.push(NodeMapping {
                node_id: m.node_id.clone(),
                topic: m.topic.clone(),
            });
        }

        if m.direction.is_write() {
            if !seen_write_topics.insert(m.topic.clone()) {
                bail!("duplicate write mapping for topic '{}'", m.topic);
            }
            table.write.push(NodeMapping {
                node_id: m.node_id.clone(),
                topic: m.topic.clone(),
            });
        }
    }

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(node_id: &str, topic: &str, direction: Direction) -> Mapping {
        Mapping {
            node_id: node_id.to_string(),
            topic: topic.to_string(),
            direction,
        }
    }

    #[test]
    fn splits_by_direction() {
        let mappings = vec![
            mapping("ns=2;s=A", "plant/a", Direction::Read),
            mapping("ns=2;s=B", "plant/b/set", Direction::Write),
            mapping("ns=2;s=C", "plant/c", Direction::Both),
        ];
        let table = build(&mappings, &FilterConfig::default()).unwrap();
        assert_eq!(table.read.len(), 2); // A and C
        assert_eq!(table.write.len(), 2); // B and C
    }

    #[test]
    fn block_list_removes_mapping_from_both_directions() {
        let mappings = vec![mapping("ns=2;s=A", "plant/debug", Direction::Both)];
        let filters = FilterConfig {
            allow: vec![],
            block: vec!["plant/debug".to_string()],
        };
        let table = build(&mappings, &filters).unwrap();
        assert!(table.read.is_empty());
        assert!(table.write.is_empty());
    }

    #[test]
    fn duplicate_read_topic_is_rejected() {
        let mappings = vec![
            mapping("ns=2;s=A", "plant/a", Direction::Read),
            mapping("ns=2;s=B", "plant/a", Direction::Read),
        ];
        assert!(build(&mappings, &FilterConfig::default()).is_err());
    }

    #[test]
    fn duplicate_write_topic_is_rejected() {
        let mappings = vec![
            mapping("ns=2;s=A", "plant/a/set", Direction::Write),
            mapping("ns=2;s=B", "plant/a/set", Direction::Write),
        ];
        assert!(build(&mappings, &FilterConfig::default()).is_err());
    }

    #[test]
    fn same_topic_can_be_both_a_read_and_a_write_target_for_different_nodes() {
        // "read A publishes to plant/a" and "write plant/a writes to B" don't
        // collide with each other — only same-direction duplicates do.
        let mappings = vec![
            mapping("ns=2;s=A", "plant/a", Direction::Read),
            mapping("ns=2;s=B", "plant/a", Direction::Write),
        ];
        let table = build(&mappings, &FilterConfig::default()).unwrap();
        assert_eq!(table.read.len(), 1);
        assert_eq!(table.write.len(), 1);
    }
}
