use crate::model::{TemplateMeta, ALLOWED_ELEMENT_KINDS, FORBIDDEN_BEHAVIOR_KEYS};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BUILTINS: &[&str] = &[
    include_str!("../builtin-templates/print-queue.v1.json"),
    include_str!("../builtin-templates/note.v1.json"),
    include_str!("../builtin-templates/list.v1.json"),
    include_str!("../builtin-templates/client.v1.json"),
];

pub fn builtin_documents() -> Result<Vec<Value>, String> {
    BUILTINS
        .iter()
        .map(|raw| {
            let doc: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
            validate_template_doc(&doc)?;
            Ok(doc)
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct Template {
    #[serde(flatten)]
    pub meta: TemplateMeta,
    pub elements: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedTemplate {
    pub template: Value,
    pub signature: String,
}

pub fn parse(raw: &str) -> Result<Template, String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    validate_template_doc(&doc)?;
    serde_json::from_value(doc).map_err(|e| e.to_string())
}

pub fn builtin_templates() -> Result<Vec<TemplateMeta>, String> {
    BUILTINS
        .iter()
        .map(|raw| parse(raw).map(|t| t.meta))
        .collect()
}

pub fn load_builtin(id: &str) -> Result<Template, String> {
    for raw in BUILTINS {
        let template = parse(raw)?;
        if template.meta.id == id {
            return Ok(template);
        }
    }
    Err(format!("unknown built-in template: {id}"))
}

fn reject_behavior(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if FORBIDDEN_BEHAVIOR_KEYS.contains(&key.to_lowercase().as_str()) {
                    return Err(format!("forbidden behavior key: {key}"));
                }
                reject_behavior(value)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_behavior(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_template_doc(doc: &Value) -> Result<(), String> {
    reject_behavior(doc)?;
    let obj = doc.as_object().ok_or("template must be an object")?;
    if obj
        .keys()
        .any(|k| !["id", "version", "name", "description", "elements"].contains(&k.as_str()))
    {
        return Err("unknown template property".into());
    }
    for key in ["id", "name", "description"] {
        if !obj
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(format!("template requires nonempty {key}"));
        }
    }
    if !obj
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|v| v > 0 && v <= u32::MAX as u64)
    {
        return Err("version must be a positive u32".into());
    }
    let elements = obj
        .get("elements")
        .and_then(Value::as_array)
        .ok_or("elements must be an array")?;
    let mut ids = std::collections::HashSet::new();
    for el in elements {
        let map = el.as_object().ok_or("element must be an object")?;
        let kind = el
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("element missing kind")?;
        if !ALLOWED_ELEMENT_KINDS.contains(&kind) {
            return Err(format!("illegal element kind: {kind}"));
        }
        let id = el
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("element missing id")?;
        if !ids.insert(id) {
            return Err(format!("duplicate element id: {id}"));
        }
        for (key, value) in map {
            let valid = match key.as_str() {
                "kind" | "id" | "label" => value.is_string(),
                "value" => ["text", "field", "link"].contains(&kind) && value.is_string(),
                "items" => {
                    ["checklist", "list", "gallery", "file"].contains(&kind)
                        && value.as_array().is_some_and(Vec::is_empty)
                }
                "primary" => kind == "file" && value.is_null(),
                "bind_slot" => ["button", "home_control"].contains(&kind) && value.is_string(),
                _ => false,
            };
            if !valid {
                return Err(format!("unsupported property {key} for {kind}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc() -> Value {
        serde_json::from_str(BUILTINS[0]).unwrap()
    }
    #[test]
    fn shipped_json_is_the_catalog_and_instance_source() {
        let t = load_builtin("print-queue").unwrap();
        assert_eq!(builtin_templates().unwrap().len(), 4);
        assert_eq!(t.elements, doc()["elements"].as_array().unwrap().clone());
        assert!(load_builtin("bookmark").is_err());
    }
    #[test]
    fn rejects_malformed_metadata_elements_and_behavior() {
        for (key, value) in [
            ("version", serde_json::json!(0)),
            ("version", serde_json::json!("1")),
            ("id", serde_json::json!("")),
            ("elements", serde_json::json!({})),
        ] {
            let mut d = doc();
            d[key] = value;
            assert!(validate_template_doc(&d).is_err());
        }
        for el in [
            serde_json::json!({"id":"x","kind":"script"}),
            serde_json::json!({"id":"x","kind":"button","url":"https://example.com"}),
            serde_json::json!({"id":"x","kind":"list","items":[{"script":"run"}]}),
            serde_json::json!({"id":"x","kind":"field","field_type":"executable"}),
            serde_json::json!({"id":"x","kind":"button","value":"run"}),
        ] {
            let mut d = doc();
            d["elements"] = serde_json::json!([el]);
            assert!(validate_template_doc(&d).is_err());
        }
        let mut d = doc();
        d["elements"][1]["id"] = d["elements"][0]["id"].clone();
        assert!(validate_template_doc(&d).is_err());
    }
}
