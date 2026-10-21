//! opencode's global config, read so that the written order of every v1 permission map is kept.
//! opencode uses the last matching pattern, while an ordinary JSON object forgets that order.

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;
use std::collections::BTreeMap;
use std::marker::PhantomData;

/// Ordered permission entries: each kind's decision or its patterns.
pub(super) type Kinds = Vec<(String, Setting)>;

pub struct OcConfig {
    pub value: Value,
    /// The top-level `permission`, or None when the value is neither a decision nor a map of decisions.
    pub(super) permission: Option<Kinds>,
    /// Each v1 agent's `permission`, by the section that defines it (`agent` or `mode`) and its name.
    agent_permissions: BTreeMap<(String, String), Option<Kinds>>,
}

#[derive(Deserialize)]
struct Root {
    #[serde(default)]
    permission: Option<Permission>,
    #[serde(default)]
    agent: Option<Agents>,
    #[serde(default)]
    mode: Option<Agents>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Permission {
    One(String),
    Kinds(Entries<Setting>),
    Other(IgnoredAny),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Agents {
    Named(Entries<AgentRules>),
    Other(IgnoredAny),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AgentRules {
    Agent {
        #[serde(default)]
        permission: Option<Permission>,
    },
    Other(IgnoredAny),
}

/// One permission kind's decision, or its patterns in written order.
#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Setting {
    One(String),
    Patterns(Entries<Value>),
    Other(IgnoredAny),
}

/// A JSON object deserialized as entries in their written order.
pub(super) struct Entries<T>(pub(super) Vec<(String, T)>);
struct EntryVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for EntryVisitor<T> {
    type Value = Entries<T>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Entries<T>, M::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }

        Ok(Entries(entries))
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entries<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(EntryVisitor(PhantomData))
    }
}

fn kinds(permission: Option<Permission>) -> Option<Kinds> {
    match permission? {
        Permission::Kinds(kinds) => Some(kinds.0),
        Permission::One(decision) => Some(vec![("*".into(), Setting::One(decision))]),
        Permission::Other(_) => None,
    }
}

impl OcConfig {
    /// Parses config text whose comments have already been stripped.
    pub fn parse(text: &str) -> Option<Self> {
        let value = serde_json::from_str(text).ok()?;
        let root = serde_json::from_str::<Root>(text).ok();
        let mut agent_permissions = BTreeMap::new();
        let (permission, sections) = match root {
            Some(root) => (kinds(root.permission), [("agent", root.agent), ("mode", root.mode)]),
            None => (None, [("agent", None), ("mode", None)]),
        };

        for (section, agents) in sections {
            let Some(Agents::Named(agents)) = agents else { continue };
            for (name, rules) in agents.0 {
                if let AgentRules::Agent {
                    permission: Some(permission),
                } = rules
                {
                    agent_permissions.insert((section.to_string(), name), kinds(Some(permission)));
                }
            }
        }

        Some(Self {
            value,
            permission,
            agent_permissions,
        })
    }

    /// A v1 agent's permission entries in written order; the outer None when it has none, the inner when unreadable.
    pub(super) fn agent_permission(&self, section: &str, name: &str) -> Option<&Option<Kinds>> {
        self.agent_permissions.get(&(section.to_string(), name.to_string()))
    }

    /// MCP servers by name: v1's directly under `mcp`, v2's under `mcp.servers`, which wins for a name in both.
    /// v1 servers named `servers` or `timeout` are told apart from v2's sections by their `type`, as v2 does.
    pub fn servers(&self) -> Vec<(String, Value)> {
        let is_server = |value: &Value| matches!(value["type"].as_str(), Some("local" | "remote"));
        let mut legacy = Vec::new();
        let mut native = Vec::new();

        for (name, value) in self.value["mcp"].as_object().into_iter().flatten() {
            match name.as_str() {
                "servers" if !is_server(value) => native.extend(value.as_object().into_iter().flatten()),
                "timeout" if !is_server(value) => {}
                _ => legacy.push((name, value)),
            }
        }

        legacy.retain(|(name, _)| !native.iter().any(|(native, _)| native == name));
        legacy
            .into_iter()
            .chain(native)
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}
