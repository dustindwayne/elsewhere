use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const ALLOWED_ELEMENT_KINDS: &[&str] = &[
    "text",
    "checklist",
    "list",
    "gallery",
    "file",
    "link",
    "field",
    "button",
    "home_control",
    "print_request",
];

pub const FORBIDDEN_BEHAVIOR_KEYS: &[&str] = &[
    "url",
    "href",
    "command",
    "script",
    "on_click",
    "onclick",
    "webhook",
    "device_id",
    "device",
    "credentials",
    "token",
    "api_key",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    #[serde(rename = "type")]
    pub note_type: String,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub template_version: Option<u32>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub elements: Vec<serde_json::Value>,
    #[serde(default)]
    pub person_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Stable identity of the node that first created this record.
    #[serde(default)]
    pub origin_node_id: String,
    /// Current logical revision. Revisions are metadata; they do not alter v0 editing.
    #[serde(default)]
    pub revision_id: String,
    #[serde(default)]
    pub parent_revision_ids: Vec<String>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub actor: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub elements: Option<Vec<serde_json::Value>>,
    #[serde(rename = "type")]
    pub note_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub messenger: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateMeta {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteArm {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreFile {
    #[serde(default)]
    pub node_id: String,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub people: Vec<Person>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub primary: bool,
    #[serde(default)]
    pub original_path: String,
}
