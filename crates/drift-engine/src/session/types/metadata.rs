use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

// One field list keeps storage, merging and the API schema in agreement.
macro_rules! metadata_fields {
    ($($field:ident: $ty:ty => $key:literal),* $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq, Deserialize, ToSchema)]
        #[serde(from = "Value")]
        pub struct ToolMetadata {
            $(
                #[serde(rename = $key, skip_serializing_if = "Option::is_none")]
                #[schema(rename = $key)]
                pub $field: Option<$ty>,
            )*
            #[serde(flatten)]
            pub extra: serde_json::Map<String, Value>,
            #[serde(skip)]
            pub legacy: Option<Value>,
        }

        impl From<Value> for ToolMetadata {
            fn from(value: Value) -> Self {
                let Value::Object(mut fields) = value else {
                    return Self { legacy: Some(value), ..Self::default() };
                };

                Self {
                    $($field: metadata_field(&mut fields, $key),)*
                    extra: fields,
                    legacy: None,
                }
            }
        }

        impl Serialize for ToolMetadata {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;

                if let Some(value) = &self.legacy {
                    return value.serialize(serializer);
                }

                let mut map = serializer.serialize_map(None)?;
                $(serialize_metadata_field(&mut map, $key, &self.$field)?;)*
                let typed = [$(($key, self.$field.is_some()),)*];
                for (key, value) in &self.extra {
                    if !typed.iter().any(|(name, present)| name == key && *present) {
                        map.serialize_entry(key, value)?;
                    }
                }

                map.end()
            }
        }

        impl ToolMetadata {
            pub fn merged(mut self, extra: Option<Self>) -> Option<Self> {
                if self.is_null() {
                    return extra;
                }
                if self.legacy.is_some() {
                    return Some(self);
                }

                if let Some(mut extra) = extra.filter(|extra| extra.legacy.is_none()) {
                    $(
                        replace_metadata_field(
                            &mut self.$field,
                            &mut self.extra,
                            &mut extra.$field,
                            &extra.extra,
                            $key,
                        );
                    )*
                    self.extra.extend(extra.extra);
                }

                Some(self)
            }
        }
    };
}

metadata_fields! {
    engine_command: String => "engineCommand",
    command_model: String => "commandModel",
    shell_timeout_ms: Option<u64> => "shellTimeoutMs",
    output_bytes: u64 => "outputBytes",
    output_file: String => "outputFile",
    result_file: String => "resultFile",
    output: String => "output",
    exit: i64 => "exit",
    timed_out: bool => "timedOut",
    stopped: bool => "stopped",
    notes: Vec<String> => "notes",
    files: Vec<MetadataFile> => "files",
    diff: String => "diff",
    file_changes: Vec<ToolFileChange> => "fileChanges",
    replacements: usize => "replacements",
    created: bool => "created",
    changes: Vec<HistoryChange> => "changes",
    owner: String => "owner",
    at: String => "at",
    history_error: String => "historyError",
    unrecorded: Vec<String> => "unrecorded",
    checks: Vec<ToolCheck> => "checks",
    check_changed: Vec<String> => "checkChanged",
    check_observed: Vec<String> => "checkObserved",
    formatted: Vec<String> => "formatted",
    diagnostics: Vec<ToolDiagnostic> => "diagnostics",
    images: Vec<ToolImage> => "images",
    lines: Option<usize> => "lines",
    shown: usize => "shown",
    large: bool => "large",
    count: usize => "count",
    total: usize => "total",
    truncated: bool => "truncated",
    capped: bool => "capped",
    withheld: usize => "withheld",
    restricted: usize => "restricted",
    path: String => "path",
    open: usize => "open",
    redirect: String => "redirect",
    content_type: String => "contentType",
    bytes: usize => "bytes",
    server: String => "server",
    uri: String => "uri",
    request_id: String => "requestId",
    asynchronous: bool => "async",
    answers: Vec<Vec<String>> => "answers",
    session_id: String => "sessionId",
    task_id: String => "taskId",
    agent: String => "agent",
    outcome: String => "outcome",
    mode: String => "mode",
    reason: String => "reason",
    delivers: String => "delivers",
    state: String => "state",
    running: bool => "running",
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum MetadataFile {
    Path(String),
    Imported(serde_json::Map<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolFileChange {
    pub file_path: String,
    pub relative_path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub patch: String,
    pub additions: usize,
    pub deletions: usize,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HistoryChange {
    pub path: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub before: Option<Option<String>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub after: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolCheck {
    pub check: String,
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Problems,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolDiagnostic {
    pub file: String,
    pub server: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolImage {
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn metadata_field<T: serde::de::DeserializeOwned + Serialize>(
    fields: &mut serde_json::Map<String, Value>,
    key: &str,
) -> Option<T> {
    let value = fields.get(key)?;
    let parsed: T = serde_json::from_value(value.clone()).ok()?;
    // Imported fields must stay unchanged if native serialization would normalize them.
    if serde_json::to_value(&parsed).ok().as_ref() != Some(value) {
        return None;
    }
    fields.remove(key);

    Some(parsed)
}

fn serialize_metadata_field<M: serde::ser::SerializeMap, T: Serialize>(
    map: &mut M,
    key: &str,
    field: &Option<T>,
) -> Result<(), M::Error> {
    if let Some(value) = field {
        map.serialize_entry(key, value)?;
    }

    Ok(())
}

fn replace_metadata_field<T>(
    field: &mut Option<T>,
    fields: &mut serde_json::Map<String, Value>,
    replacement: &mut Option<T>,
    extra: &serde_json::Map<String, Value>,
    key: &str,
) {
    if replacement.is_some() {
        *field = replacement.take();
        fields.remove(key);
    } else if extra.contains_key(key) {
        *field = None;
    }
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl ToolMetadata {
    pub fn null() -> Self {
        Self {
            legacy: Some(Value::Null),
            ..Self::default()
        }
    }

    pub fn is_null(&self) -> bool {
        self.legacy.as_ref().is_some_and(Value::is_null)
    }

    pub fn file_paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().flatten().filter_map(|file| match file {
            MetadataFile::Path(path) => Some(path.as_str()),
            MetadataFile::Imported(_) => None,
        })
    }
}

impl From<crate::session::snapshot::FileChange> for HistoryChange {
    fn from(change: crate::session::snapshot::FileChange) -> Self {
        Self {
            path: change.path,
            before: Some(change.before),
            after: Some(change.after),
            observed: change.observed.then_some(true),
            extra: Default::default(),
        }
    }
}

impl HistoryChange {
    pub fn snapshot(&self) -> crate::session::snapshot::FileChange {
        crate::session::snapshot::FileChange {
            path: self.path.clone(),
            before: self.before.clone().flatten(),
            after: self.after.clone().flatten(),
            observed: self.observed.unwrap_or(false),
        }
    }
}
