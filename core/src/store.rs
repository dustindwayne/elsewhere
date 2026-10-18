use crate::{model::Note, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileData {
    pub id: String,
    pub name: String,
    pub mime: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub blob: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    #[serde(flatten)]
    pub note: Note,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub files: Vec<FileData>,
    #[serde(default)]
    pub items: Vec<ListItem>,
}
fn default_color() -> String {
    "iris".into()
}

pub type Thought = Record;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    pub node: String,
    pub entity: String,
    pub parents: Vec<String>,
    pub thought: Option<Thought>,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub key: String,
    pub channel: String,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub node: String,
    pub name: String,
    pub thoughts: Vec<Thought>,
    pub conflicts: BTreeMap<String, Vec<Revision>>,
    pub peers: Vec<Value>,
}

pub struct Store {
    db: Connection,
    pub node: String,
    pub name: String,
}

impl Store {
    pub(crate) fn open_agent_memory(&self) -> Result<Self> {
        let path = self.db.path().ok_or("User storage path is unavailable")?;
        let dir = Path::new(path)
            .parent()
            .ok_or("User storage directory is unavailable")?;
        Self::open(&dir.join("agent-memory"))
    }

    pub(crate) fn setting(&self, key: &str) -> Option<String> {
        self.db
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .ok()
    }
    pub(crate) fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO meta VALUES (?1,?2)",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let db = Connection::open(dir.join("elsewhere.db")).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS revisions (id TEXT PRIMARY KEY, entity TEXT NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS peers (id TEXT PRIMARY KEY, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS templates (id TEXT PRIMARY KEY, json TEXT NOT NULL);").map_err(|e|e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS blobs(id TEXT PRIMARY KEY,data TEXT NOT NULL); CREATE TABLE IF NOT EXISTS revision_parents(child TEXT NOT NULL,parent TEXT NOT NULL,PRIMARY KEY(child,parent)); CREATE INDEX IF NOT EXISTS parent_lookup ON revision_parents(parent); CREATE INDEX IF NOT EXISTS record_revisions ON revisions(entity);").map_err(|e|e.to_string())?;
        db.execute(
            "INSERT OR IGNORE INTO meta VALUES ('node', ?1)",
            [Uuid::new_v4().to_string()],
        )
        .map_err(|e| e.to_string())?;
        db.execute(
            "INSERT OR IGNORE INTO meta VALUES ('name', 'My Elsewhere')",
            [],
        )
        .map_err(|e| e.to_string())?;
        let node = db
            .query_row("SELECT value FROM meta WHERE key='node'", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let name = db
            .query_row("SELECT value FROM meta WHERE key='name'", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut store = Self { db, node, name };
        store.migrate_storage(dir)?;
        Ok(store)
    }
    pub fn revisions(&self) -> Result<Vec<Revision>> {
        let mut stmt = self
            .db
            .prepare("SELECT json FROM revisions ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    pub fn heads(&self) -> Result<BTreeMap<String, Vec<Revision>>> {
        let mut stmt=self.db.prepare("SELECT r.json FROM revisions r WHERE NOT EXISTS (SELECT 1 FROM revision_parents p WHERE p.parent=r.id)").map_err(|e|e.to_string())?;
        let all = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .map(|r| {
                serde_json::from_str::<Revision>(&r.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        let mut heads: BTreeMap<String, Vec<Revision>> = BTreeMap::new();
        for r in &all {
            heads.entry(r.entity.clone()).or_default().push(r.clone());
        }
        Ok(heads)
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut thoughts = Vec::new();
        let mut conflicts = BTreeMap::new();
        for (id, mut versions) in self.heads()? {
            versions.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
            // A concurrent deletion must never hide an unmerged edit.
            if let Some(t) = versions.iter().rev().find_map(|r| r.thought.clone()) {
                thoughts.push(t);
            }
            if versions.len() > 1 {
                conflicts.insert(id, versions);
            }
        }
        let peers = self
            .peers()?
            .into_iter()
            .map(|p| json!({"id":p.id,"name":p.name,"address":p.address}))
            .collect();
        Ok(Snapshot {
            node: self.node.clone(),
            name: self.name.clone(),
            thoughts,
            conflicts,
            peers,
        })
    }
    pub fn rename(&mut self, name: &str) -> Result<()> {
        if name.trim().is_empty() || name.len() > 100 {
            return Err("Choose a device name under 100 characters".into());
        }
        self.db
            .execute("UPDATE meta SET value=?1 WHERE key='name'", [name])
            .map_err(|e| e.to_string())?;
        self.name = name.into();
        Ok(())
    }
    pub fn listen_port(&self) -> u16 {
        self.db
            .query_row("SELECT value FROM meta WHERE key='listen_port'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    pub fn set_listen_port(&self, port: u16) -> Result<()> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO meta VALUES ('listen_port',?1)",
                [port.to_string()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn create(&mut self, x: f64, y: f64) -> Result<Thought> {
        let record = self.blank_record(x, y);
        self.commit(record, &[], false)
    }
    pub fn blank_record(&self, x: f64, y: f64) -> Record {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        let note = Note {
            id: id.clone(),
            note_type: "plain".into(),
            template_id: None,
            template_version: None,
            title: String::new(),
            body: String::new(),
            elements: vec![],
            person_id: None,
            attachments: vec![],
            origin_node_id: self.node.clone(),
            revision_id: String::new(),
            parent_revision_ids: vec![],
            provenance: None,
            created_at: now,
            updated_at: now,
        };
        Record {
            note,
            x,
            y,
            color: default_color(),
            links: vec![],
            fields: BTreeMap::new(),
            files: vec![],
            items: vec![],
        }
    }
    pub fn commit(
        &mut self,
        mut thought: Thought,
        parents: &[String],
        resolve: bool,
    ) -> Result<Thought> {
        thought.note.provenance = Some(crate::model::Provenance {
            actor: "user".into(),
            agent_id: None,
            operation_id: None,
            created_at: Utc::now(),
        });
        self.commit_attributed(thought, parents, resolve)
    }
    pub fn commit_attributed(
        &mut self,
        mut thought: Record,
        parents: &[String],
        resolve: bool,
    ) -> Result<Record> {
        let heads = self.heads()?.remove(&thought.note.id).unwrap_or_default();
        let known: HashSet<String> = self
            .revisions()?
            .into_iter()
            .filter(|r| r.entity == thought.note.id)
            .map(|r| r.id)
            .collect();
        if !heads.is_empty() && parents.is_empty() {
            return Err("This thought already exists; reload before editing".into());
        }
        if parents.iter().any(|p| !known.contains(p)) {
            return Err("Unknown parent revision".into());
        }
        if resolve && heads.iter().any(|r| !parents.contains(&r.id)) {
            return Err("Another version arrived. Review all versions before resolving".into());
        }
        thought.note.updated_at = Utc::now();
        thought.note.revision_id = Uuid::new_v4().to_string();
        thought.note.parent_revision_ids = parents.to_vec();
        let r = Revision {
            id: thought.note.revision_id.clone(),
            node: self.node.clone(),
            entity: thought.note.id.clone(),
            parents: parents.to_vec(),
            thought: Some(thought.clone()),
            at: thought.note.updated_at.to_rfc3339(),
        };
        self.merge(&[r])?;
        self.record(&thought.note.id)
    }
    pub fn delete(&mut self, id: &str, parents: &[String]) -> Result<()> {
        let heads = self.heads()?.remove(id).ok_or("Thought not found")?;
        if heads.iter().any(|r| !parents.contains(&r.id)) {
            return Err("The thought changed; review it before deleting".into());
        }
        self.merge(&[Revision {
            id: Uuid::new_v4().to_string(),
            node: self.node.clone(),
            entity: id.into(),
            parents: heads.iter().map(|r| r.id.clone()).collect(),
            thought: None,
            at: Utc::now().to_rfc3339(),
        }])
    }
    pub fn merge(&mut self, incoming: &[Revision]) -> Result<()> {
        let mut normalized = incoming.to_vec();
        let mut new_blobs = BTreeMap::new();
        for revision in &mut normalized {
            normalize_files(revision, &mut new_blobs)?;
        }
        let incoming = &normalized;
        if incoming.len() > 100_000 {
            return Err("Too many revisions".into());
        }
        let existing = self.revisions()?;
        let mut all: BTreeMap<String, Revision> =
            existing.into_iter().map(|r| (r.id.clone(), r)).collect();
        for r in incoming {
            validate(r)?;
            if let Some(old) = all.get(&r.id) {
                if serde_json::to_value(old).unwrap() != serde_json::to_value(r).unwrap() {
                    return Err("Revision identity collision".into());
                }
            } else {
                all.insert(r.id.clone(), r.clone());
            }
        }
        for r in incoming {
            for parent in &r.parents {
                let p = all.get(parent).ok_or("Missing parent revision")?;
                if p.entity != r.entity {
                    return Err("Parent belongs to another thought".into());
                }
            }
        }
        // Kahn's algorithm rejects cycles without recursion on untrusted history.
        let mut degrees: BTreeMap<String, usize> = all
            .iter()
            .map(|(id, r)| (id.clone(), r.parents.len()))
            .collect();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, r) in &all {
            for p in &r.parents {
                children.entry(p.clone()).or_default().push(id.clone());
            }
        }
        let mut ready: Vec<String> = degrees
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let mut count = 0;
        while let Some(id) = ready.pop() {
            count += 1;
            if let Some(cs) = children.get(&id) {
                for c in cs {
                    let d = degrees.get_mut(c).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.push(c.clone());
                    }
                }
            }
        }
        if count != all.len() {
            return Err("Cyclic revision history".into());
        }
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        for (id, data) in new_blobs {
            tx.execute(
                "INSERT OR IGNORE INTO blobs VALUES (?1,?2)",
                params![id, data],
            )
            .map_err(|e| e.to_string())?;
        }
        for r in incoming {
            if let Some(record) = &r.thought {
                for file in &record.files {
                    if !file.blob.is_empty()
                        && !tx
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM blobs WHERE id=?1)",
                                [&file.blob],
                                |r| r.get::<_, bool>(0),
                            )
                            .map_err(|e| e.to_string())?
                    {
                        return Err("Missing attachment blob".into());
                    }
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO revisions VALUES (?1,?2,?3)",
                params![r.id, r.entity, serde_json::to_string(r).unwrap()],
            )
            .map_err(|e| e.to_string())?;
            for p in &r.parents {
                tx.execute(
                    "INSERT OR IGNORE INTO revision_parents VALUES (?1,?2)",
                    params![r.id, p],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }
    fn migrate_storage(&mut self, dir: &Path) -> Result<()> {
        if self.setting("storage_version").as_deref() == Some("2") {
            return Ok(());
        }
        let mut revisions = self.revisions()?;
        let mut blobs = BTreeMap::new();
        if !revisions.is_empty() {
            let backup = dir.join("before-knowledge-v2.db");
            if !backup.exists() {
                self.db
                    .execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
                    .map_err(|e| format!("Could not back up the existing space: {e}"))?;
            }
        }
        for r in &mut revisions {
            normalize_files(r, &mut blobs)?;
        }
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        for (id, data) in blobs {
            tx.execute(
                "INSERT OR IGNORE INTO blobs VALUES (?1,?2)",
                params![id, data],
            )
            .map_err(|e| e.to_string())?;
        }
        for r in &revisions {
            tx.execute(
                "UPDATE revisions SET json=?2 WHERE id=?1",
                params![r.id, serde_json::to_string(r).unwrap()],
            )
            .map_err(|e| e.to_string())?;
            for p in &r.parents {
                tx.execute(
                    "INSERT OR IGNORE INTO revision_parents VALUES (?1,?2)",
                    params![r.id, p],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta VALUES ('storage_version','2')",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn revision_ids(&self) -> Result<Vec<String>> {
        let mut s = self
            .db
            .prepare("SELECT id FROM revisions")
            .map_err(|e| e.to_string())?;
        let out = s
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(out)
    }
    pub fn blob_ids(&self) -> Result<Vec<String>> {
        let mut s = self
            .db
            .prepare("SELECT id FROM blobs")
            .map_err(|e| e.to_string())?;
        let out = s
            .query_map([], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(out)
    }
    pub fn blobs_missing(&self, known: &[String]) -> Result<BTreeMap<String, String>> {
        let known: HashSet<&String> = known.iter().collect();
        let mut out = BTreeMap::new();
        for id in self.blob_ids()? {
            if !known.contains(&id) {
                out.insert(id.clone(), self.blob(&id)?);
            }
        }
        Ok(out)
    }
    pub fn blob(&self, id: &str) -> Result<String> {
        self.db
            .query_row("SELECT data FROM blobs WHERE id=?1", [id], |r| r.get(0))
            .map_err(|_| "Attachment not found".into())
    }
    pub fn receive_blobs(&mut self, blobs: &BTreeMap<String, String>) -> Result<()> {
        let mut total = 0usize;
        for (id, data) in blobs {
            let actual = blob_hash(data)?;
            if &actual != id {
                return Err("Attachment hash mismatch".into());
            }
            total += data.len();
            if total > 45 * 1024 * 1024 {
                return Err("Attachment exchange exceeds the current packet limit".into());
            }
        }
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        for (id, data) in blobs {
            tx.execute(
                "INSERT OR IGNORE INTO blobs VALUES (?1,?2)",
                params![id, data],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn peers(&self) -> Result<Vec<Peer>> {
        let mut s = self
            .db
            .prepare("SELECT json FROM peers")
            .map_err(|e| e.to_string())?;
        let result = s
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .map(|r| {
                serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
            })
            .collect();
        result
    }
    pub fn put_peer(&mut self, p: &Peer) -> Result<()> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO peers VALUES (?1,?2)",
                params![p.id, serde_json::to_string(p).unwrap()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn remove_peer(&mut self, id: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM peers WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn import(&mut self, value: Value) -> Result<usize> {
        let mut items = value
            .get("notes")
            .or_else(|| value.get("thoughts"))
            .and_then(Value::as_array)
            .ok_or("Choose an Exist, Mindspace, or Elsewhere notes export")?
            .clone();
        if let Some(people) = value.get("people").and_then(Value::as_array) {
            for p in people {
                let now = Utc::now().to_rfc3339();
                items.push(json!({"id":p["id"],"type":"client","title":p["name"],"body":"","template_id":"client","template_version":1,"fields":{"name":p["name"],"phone":p.get("phone").and_then(Value::as_str).unwrap_or(""),"messenger":p.get("messenger").and_then(Value::as_str).unwrap_or("")},"created_at":now,"updated_at":now}));
            }
        }
        let mut revisions = Vec::new();
        let known = self.heads()?;
        for (i, item) in items.iter().enumerate() {
            let mut t: Thought = if item.get("text").is_some() && item.get("body").is_none() {
                let now = Utc::now();
                let id = item["id"]
                    .as_str()
                    .ok_or("Mindspace thought is missing an ID")?
                    .to_string();
                Thought {
                    note: Note {
                        id,
                        note_type: "plain".into(),
                        template_id: None,
                        template_version: None,
                        title: String::new(),
                        body: item["text"]
                            .as_str()
                            .ok_or("Invalid Mindspace thought text")?
                            .into(),
                        elements: vec![],
                        person_id: None,
                        attachments: vec![],
                        origin_node_id: self.node.clone(),
                        revision_id: String::new(),
                        parent_revision_ids: vec![],
                        provenance: None,
                        created_at: now,
                        updated_at: now,
                    },
                    x: item["x"].as_f64().unwrap_or(0.0),
                    y: item["y"].as_f64().unwrap_or(0.0),
                    color: default_color(),
                    links: vec![],
                    fields: BTreeMap::new(),
                    files: vec![],
                    items: vec![],
                }
            } else {
                serde_json::from_value(item.clone()).map_err(|e| format!("Note {}: {e}", i + 1))?
            };
            if known.contains_key(&t.note.id) {
                continue;
            }
            if item.get("x").is_none() {
                t.x = (i % 4) as f64 * 310.0;
                t.y = (i / 4) as f64 * 230.0;
            }
            for element in &t.note.elements {
                if element["kind"] == "field" {
                    if let (Some(id), Some(value)) =
                        (element["id"].as_str(), element["value"].as_str())
                    {
                        t.fields.entry(id.into()).or_insert(value.into());
                    }
                }
            }
            t.note.revision_id = Uuid::new_v4().to_string();
            t.note.parent_revision_ids.clear();
            revisions.push(Revision {
                id: t.note.revision_id.clone(),
                node: self.node.clone(),
                entity: t.note.id.clone(),
                parents: vec![],
                at: Utc::now().to_rfc3339(),
                thought: Some(t),
            });
        }
        self.merge(&revisions)?;
        Ok(revisions.len())
    }
    pub fn template_catalog(&self) -> Result<Vec<Value>> {
        let mut out = crate::templates::builtin_documents()?;
        let mut s = self
            .db
            .prepare("SELECT json FROM templates")
            .map_err(|e| e.to_string())?;
        for row in s
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
        {
            let p: crate::templates::SignedTemplate =
                serde_json::from_str(&row.map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            crate::signatures::verify_template(&p.template, &p.signature)?;
            out.push(p.template);
        }
        Ok(out)
    }
    pub fn install_template(&mut self, p: crate::templates::SignedTemplate) -> Result<()> {
        crate::signatures::verify_template(&p.template, &p.signature)?;
        let id = format!(
            "{}@{}",
            p.template["id"].as_str().unwrap(),
            p.template["version"]
        );
        if self
            .template_catalog()?
            .iter()
            .any(|t| format!("{}@{}", t["id"].as_str().unwrap(), t["version"]) == id)
        {
            return Err("This template version is already installed".into());
        }
        self.db
            .execute(
                "INSERT INTO templates VALUES (?1,?2)",
                params![id, serde_json::to_string(&p).unwrap()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn blob_hash(data: &str) -> Result<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    if data.len() > 7_000_000 {
        return Err("Attachments must be no larger than 5 MB".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "Invalid attachment encoding")?;
    if bytes.len() > 5 * 1024 * 1024 {
        return Err("Attachments must be no larger than 5 MB".into());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn normalize_files(revision: &mut Revision, blobs: &mut BTreeMap<String, String>) -> Result<()> {
    if let Some(record) = &mut revision.thought {
        for file in &mut record.files {
            if !file.data.is_empty() {
                let hash = blob_hash(&file.data)?;
                if !file.blob.is_empty() && file.blob != hash {
                    return Err("Attachment hash mismatch".into());
                }
                file.blob = hash.clone();
                blobs.insert(hash, std::mem::take(&mut file.data));
            }
            if !file.blob.is_empty()
                && (file.blob.len() != 64 || !file.blob.chars().all(|c| c.is_ascii_hexdigit()))
            {
                return Err("Invalid attachment reference".into());
            }
        }
    }
    Ok(())
}
fn validate(r: &Revision) -> Result<()> {
    for id in [&r.id, &r.node, &r.entity] {
        Uuid::parse_str(id).map_err(|_| "Invalid record identity")?;
    }
    chrono::DateTime::parse_from_rfc3339(&r.at).map_err(|_| "Invalid revision date")?;
    if r.parents.len() > 100 || r.parents.iter().collect::<HashSet<_>>().len() != r.parents.len() {
        return Err("Invalid parents".into());
    }
    if let Some(t) = &r.thought {
        if t.note.id != r.entity
            || t.note.revision_id != r.id
            || t.note.parent_revision_ids != r.parents
        {
            return Err("Mismatched thought revision".into());
        }
        if !t.x.is_finite()
            || !t.y.is_finite()
            || t.x.abs() > 1_000_000.0
            || t.y.abs() > 1_000_000.0
        {
            return Err("Invalid position".into());
        }
        if !["iris", "rose", "mint", "amber", "sky"].contains(&t.color.as_str()) {
            return Err("Unknown color".into());
        }
        if t.note.title.len() > 10_000
            || t.note.body.len() > 1_000_000
            || t.files.len() > 20
            || t.links.len() > 100
            || t.items.len() > 1000
            || t.items.iter().any(|i| i.text.len() > 10_000)
        {
            return Err("Thought is too large".into());
        }
        use base64::Engine;
        for f in &t.files {
            if f.name.len() > 500
                || f.data.len() > 7_000_000
                || base64::engine::general_purpose::STANDARD
                    .decode(&f.data)
                    .is_err()
            {
                return Err("Invalid attachment (maximum 5 MB per file)".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachments_deduplicate_and_existing_databases_migrate_with_backup() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path()).unwrap();
        let mut r = s.create(0.0, 0.0).unwrap();
        let parent = r.note.revision_id.clone();
        let data = STANDARD.encode(b"one file, many revisions");
        r.files.push(FileData {
            id: Uuid::new_v4().to_string(),
            name: "reference.txt".into(),
            mime: "text/plain".into(),
            data: data.clone(),
            blob: String::new(),
        });
        r = s.commit(r, &[parent], false).unwrap();
        assert!(r.files[0].data.is_empty());
        let hash = r.files[0].blob.clone();
        assert_eq!(s.blob(&hash).unwrap(), data);
        for _ in 0..3 {
            let parent = r.note.revision_id.clone();
            r.note.body.push('a');
            r = s.commit(r, &[parent], false).unwrap();
        }
        assert_eq!(s.blob_ids().unwrap().len(), 1);
        assert!(!serde_json::to_string(&s.revisions().unwrap())
            .unwrap()
            .contains(&data));
        // Simulate the original on-disk JSON shape, before blob references existed.
        let mut history = s.revisions().unwrap();
        for rev in &mut history {
            if let Some(record) = &mut rev.thought {
                for f in &mut record.files {
                    f.data = data.clone();
                    f.blob.clear();
                }
            }
            s.db.execute(
                "UPDATE revisions SET json=?2 WHERE id=?1",
                params![rev.id, serde_json::to_string(rev).unwrap()],
            )
            .unwrap();
        }
        s.set_setting("storage_version", "1").unwrap();
        drop(s);
        let restored = Store::open(dir.path()).unwrap();
        assert!(dir.path().join("before-knowledge-v2.db").exists());
        assert_eq!(restored.blob(&hash).unwrap(), data);
        assert!(restored.snapshot().unwrap().thoughts[0].files[0]
            .data
            .is_empty());
    }
    #[test]
    fn imported_clients_are_shared_records_and_lists_survive_sync() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::open(d.path()).unwrap();
        let client = Uuid::new_v4().to_string();
        let job = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let count=s.import(json!({"notes":[{"id":job,"type":"print-queue","person_id":client,"title":"A print","body":"","created_at":now,"updated_at":now}],"people":[{"id":client,"name":"Example client","phone":"555-0100","messenger":""}]})).unwrap();
        assert_eq!(count, 2);
        let snapshot = s.snapshot().unwrap();
        assert_eq!(
            snapshot
                .thoughts
                .iter()
                .find(|t| t.note.id == job)
                .unwrap()
                .note
                .person_id
                .as_deref(),
            Some(client.as_str())
        );
        assert_eq!(
            snapshot
                .thoughts
                .iter()
                .find(|t| t.note.id == client)
                .unwrap()
                .note
                .note_type,
            "client"
        );
        let mut list = s.create(0.0, 0.0).unwrap();
        let parent = list.note.revision_id.clone();
        list.note.note_type = "list".into();
        list.items.push(ListItem {
            id: Uuid::new_v4().to_string(),
            text: "Remember this".into(),
            done: true,
        });
        s.commit(list, &[parent], false).unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let mut other = Store::open(d2.path()).unwrap();
        other.merge(&s.revisions().unwrap()).unwrap();
        let list = other
            .snapshot()
            .unwrap()
            .thoughts
            .into_iter()
            .find(|t| t.note.note_type == "list")
            .unwrap();
        assert_eq!(list.items[0].text, "Remember this");
        assert!(list.items[0].done);
    }
    #[test]
    fn offline_conflicts_converge_and_resolve() {
        let da = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        let mut a = Store::open(da.path()).unwrap();
        let mut b = Store::open(db.path()).unwrap();
        let base = a.create(0.0, 0.0).unwrap();
        b.merge(&a.revisions().unwrap()).unwrap();
        let mut ta = base.clone();
        ta.note.body = "phone edit".into();
        a.commit(ta, &[base.note.revision_id.clone()], false)
            .unwrap();
        let mut tb = base.clone();
        tb.note.body = "desktop edit".into();
        b.commit(tb, &[base.note.revision_id], false).unwrap();
        a.merge(&b.revisions().unwrap()).unwrap();
        b.merge(&a.revisions().unwrap()).unwrap();
        let sa = a.snapshot().unwrap();
        let sb = b.snapshot().unwrap();
        assert_eq!(
            serde_json::to_value(&sa.thoughts).unwrap(),
            serde_json::to_value(&sb.thoughts).unwrap()
        );
        assert_eq!(sa.conflicts[&base.note.id].len(), 2);
        let parents = sa.conflicts[&base.note.id]
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>();
        let mut t = sa.thoughts[0].clone();
        t.note.body = "both edits retained".into();
        a.commit(t, &parents, true).unwrap();
        b.merge(&a.revisions().unwrap()).unwrap();
        assert!(b.snapshot().unwrap().conflicts.is_empty());
        drop(a);
        assert_eq!(
            Store::open(da.path()).unwrap().snapshot().unwrap().thoughts[0]
                .note
                .body,
            "both edits retained"
        );
    }
    #[test]
    fn concurrent_delete_keeps_unmerged_edit() {
        let d = tempfile::tempdir().unwrap();
        let mut a = Store::open(d.path()).unwrap();
        let t = a.create(0.0, 0.0).unwrap();
        a.delete(&t.note.id, &[t.note.revision_id.clone()]).unwrap();
        let mut edit = t.clone();
        edit.note.body = "keep this".into();
        a.commit(edit, &[t.note.revision_id], false).unwrap();
        assert_eq!(a.snapshot().unwrap().thoughts[0].note.body, "keep this");
        assert_eq!(a.snapshot().unwrap().conflicts.len(), 1);
    }
    #[test]
    fn rejected_batch_is_atomic() {
        let d = tempfile::tempdir().unwrap();
        let mut a = Store::open(d.path()).unwrap();
        let t = a.create(0.0, 0.0).unwrap();
        let mut rev = a.revisions().unwrap()[0].clone();
        rev.id = Uuid::new_v4().to_string();
        rev.parents = vec!["missing".into()];
        rev.thought.as_mut().unwrap().note.revision_id = rev.id.clone();
        rev.thought.as_mut().unwrap().note.parent_revision_ids = rev.parents.clone();
        assert!(a.merge(&[rev]).is_err());
        assert_eq!(a.revisions().unwrap().len(), 1);
        assert_eq!(a.snapshot().unwrap().thoughts[0].note.id, t.note.id);
    }
}
