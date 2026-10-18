//! A shared vocabulary for human interfaces, agents, search, and templates.
use crate::{
    model::Provenance,
    store::{Record, Store},
    Result,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Thought,
    Note,
    List,
    Client,
    PrintRequest,
    Template,
}
impl Record {
    pub fn kind(&self) -> RecordKind {
        match self.note.note_type.as_str() {
            "plain" | "thought" => RecordKind::Thought,
            "note" => RecordKind::Note,
            "list" => RecordKind::List,
            "client" => RecordKind::Client,
            "print-queue" => RecordKind::PrintRequest,
            _ => RecordKind::Template,
        }
    }
    pub fn searchable_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.note.title,
            self.note.body,
            self.fields.values().cloned().collect::<Vec<_>>().join(" "),
            self.items
                .iter()
                .map(|i| i.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
    pub fn summary(&self) -> Value {
        json!({"id":self.note.id,"kind":self.kind(),"type":self.note.note_type,"title":self.note.title,"excerpt":self.note.body.chars().take(280).collect::<String>(),"revision":self.note.revision_id,"updated_at":self.note.updated_at,"links":self.links,"client_id":self.note.person_id,"provenance":self.note.provenance})
    }
}
impl Store {
    pub fn record(&self, id: &str) -> Result<Record> {
        self.snapshot()?
            .thoughts
            .into_iter()
            .find(|r| r.note.id == id)
            .ok_or("Record not found".into())
    }
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut records = self.snapshot()?.thoughts;
        records.retain(|r| {
            let haystack = r.searchable_text().to_lowercase();
            terms.iter().all(|term| haystack.contains(term))
                && kind.is_none_or(|kind| {
                    serde_json::to_value(r.kind()).unwrap() == kind || r.note.note_type == kind
                })
        });
        records.sort_by(|a, b| b.note.updated_at.cmp(&a.note.updated_at));
        Ok(records
            .iter()
            .take(limit.clamp(1, 100))
            .map(Record::summary)
            .collect())
    }
    pub fn new_record(
        &mut self,
        kind: &str,
        template_id: Option<&str>,
        title: &str,
        body: &str,
    ) -> Result<Record> {
        self.new_record_as(kind, template_id, title, body, None)
    }
    pub fn new_record_as(
        &mut self,
        kind: &str,
        template_id: Option<&str>,
        title: &str,
        body: &str,
        agent: Option<&str>,
    ) -> Result<Record> {
        let template = match kind {
            "thought" | "plain" => None,
            "note" => Some("note"),
            "template" => Some(template_id.ok_or("Choose a template")?),
            _ => return Err("Choose thought, note, or template".into()),
        };
        let template = template
            .map(|id| {
                self.template_catalog()?
                    .into_iter()
                    .filter(|t| t["id"] == id)
                    .max_by_key(|t| t["version"].as_u64().unwrap_or(0))
                    .ok_or_else(|| "Template not found".to_string())
            })
            .transpose()?;
        let n = self.snapshot()?.thoughts.len();
        // Agent-created thoughts arrive with breathing room, without a row/column grid.
        // Human capture supplies the point selected in the open space.
        let angle = n as f64 * 2.399963229728653;
        let radius = 330.0 * (n as f64).sqrt();
        let mut r = self.blank_record(radius * angle.cos(), radius * angle.sin());
        r.note.title = title.into();
        r.note.body = body.into();
        if let Some(t) = template {
            r.note.note_type = t["id"].as_str().unwrap().into();
            r.note.template_id = Some(r.note.note_type.clone());
            r.note.template_version = Some(t["version"].as_u64().unwrap() as u32);
            r.note.elements =
                serde_json::from_value(t["elements"].clone()).map_err(|e| e.to_string())?;
            for e in &r.note.elements {
                if e["kind"] == "field" {
                    r.fields.insert(
                        e["id"].as_str().unwrap().into(),
                        e["value"].as_str().unwrap_or("").into(),
                    );
                }
            }
        }
        if let Some(agent) = agent {
            r.note.provenance = Some(Provenance {
                actor: "agent".into(),
                agent_id: Some(agent.into()),
                operation_id: Some(Uuid::new_v4().to_string()),
                created_at: Utc::now(),
            });
            self.commit_attributed(r, &[], false)
        } else {
            self.commit(r, &[], false)
        }
    }
    pub fn agent_update(
        &mut self,
        id: &str,
        expected: &str,
        patch: &Value,
        agent: &str,
    ) -> Result<Record> {
        let mut r = self.record(id)?;
        if r.note.revision_id != expected || self.heads()?.get(id).is_some_and(|h| h.len() != 1) {
            return Err("Revision conflict: read the current record before updating".into());
        }
        let object = patch.as_object().ok_or("Patch must be an object")?;
        if object.keys().any(|k| {
            !["title", "body", "links", "fields", "items", "client_id"].contains(&k.as_str())
        }) {
            return Err("Agents may update content and relationships, not canvas positions or record identity".into());
        }
        if let Some(v) = patch.get("title") {
            r.note.title = v.as_str().ok_or("Title must be text")?.into();
        }
        if let Some(v) = patch.get("body") {
            r.note.body = v.as_str().ok_or("Body must be text")?.into();
        }
        if let Some(v) = patch.get("links") {
            r.links = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            for target in &r.links {
                self.record(target)?;
            }
        }
        if let Some(v) = patch.get("fields") {
            let fields: std::collections::BTreeMap<String, String> =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            r.fields.extend(fields);
        }
        if let Some(v) = patch.get("items") {
            r.items = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        }
        if let Some(v) = patch.get("client_id") {
            r.note.person_id = if v.is_null() {
                None
            } else {
                let id = v.as_str().ok_or("Client ID must be text")?;
                if self.record(id)?.kind() != RecordKind::Client {
                    return Err("Selected record is not a client".into());
                }
                Some(id.into())
            };
        }
        r.note.provenance = Some(Provenance {
            actor: "agent".into(),
            agent_id: Some(agent.into()),
            operation_id: Some(Uuid::new_v4().to_string()),
            created_at: Utc::now(),
        });
        self.commit_attributed(r, &[expected.into()], false)
    }
}
