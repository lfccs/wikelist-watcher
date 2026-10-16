//! Materializes DataCore records into JSON trees (via [`TreeSink`]) and
//! resolves cross-record references by GUID, with a cache.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;
use starbreaker_common::CigGuid;
use starbreaker_datacore::Database;
use starbreaker_datacore::types::Record;

use crate::tree::TreeSink;

pub struct RefResolver<'db, 'a> {
    db: &'db Database<'a>,
    cache: HashMap<String, Rc<Value>>,
    pub warnings: Vec<String>,
}

impl<'db, 'a> RefResolver<'db, 'a> {
    pub fn new(db: &'db Database<'a>) -> Self {
        Self { db, cache: HashMap::new(), warnings: Vec::new() }
    }

    pub fn db(&self) -> &'db Database<'a> {
        self.db
    }

    /// Materialize a record into a JSON tree (`{_RecordName_, _RecordId_, _RecordValue_}`).
    pub fn materialize(&mut self, record: &Record) -> Result<Rc<Value>, starbreaker_datacore::ExportError> {
        let id = record.id;
        let key = id.to_string();
        if let Some(v) = self.cache.get(&key) {
            return Ok(Rc::clone(v));
        }
        let mut sink = TreeSink::new();
        starbreaker_datacore::walker::walk_record(self.db, record, &mut sink)?;
        let v = Rc::new(sink.finish());
        self.cache.insert(key, Rc::clone(&v));
        Ok(v)
    }

    /// Materialize the record with the given GUID, if it exists.
    pub fn get(&mut self, id: &str) -> Option<Rc<Value>> {
        if let Some(v) = self.cache.get(id) {
            return Some(Rc::clone(v));
        }
        let guid: CigGuid = id.parse().ok()?;
        let record = *self.db.record_by_id(&guid)?;
        match self.materialize(&record) {
            Ok(v) => Some(v),
            Err(e) => {
                self.warnings.push(format!("cannot materialize record {id}: {e}"));
                None
            }
        }
    }

    /// Resolve a reference-like JSON value to the referenced record's `_RecordValue_`.
    ///
    /// Accepts the `$ref` objects emitted by [`TreeSink`], the
    /// `{_RecordId_, _RecordName_, ...}` objects the walker emits for sub-records
    /// (inline ones are returned as-is), and plain GUID strings.
    pub fn resolve_value(&mut self, v: &Value) -> Option<Rc<Value>> {
        match v {
            Value::Object(map) => {
                if let Some(id) = map.get("$ref").and_then(Value::as_str) {
                    return self.get(id).map(|r| record_value(&r));
                }
                if let Some(id) = map.get("_RecordId_").and_then(Value::as_str) {
                    // Inline sub-record carries its own fields besides the metadata.
                    let inline = map.keys().any(|k| !matches!(k.as_str(), "_RecordId_" | "_RecordName_" | "_RecordPath_"));
                    if inline {
                        return Some(Rc::new(v.clone()));
                    }
                    return self.get(id).map(|r| record_value(&r));
                }
                None
            }
            Value::String(s) if s.len() == 36 && s.matches('-').count() == 4 => self.get(s).map(|r| record_value(&r)),
            _ => None,
        }
    }

    /// Record name (e.g. `EntityClassDefinition.Foo`) for a reference-like value.
    pub fn ref_name(&self, v: &Value) -> Option<String> {
        v.get("$name").or_else(|| v.get("_RecordName_")).and_then(Value::as_str).map(str::to_string)
    }

    /// Find main records of a struct type whose record name matches exactly.
    pub fn find_by_name(&self, type_name: &str, record_name: &str) -> Option<Record> {
        self.db
            .records_by_type_name(type_name)
            .find(|r| {
                let off = r.name_offset;
                self.db.resolve_string2(off) == record_name
            })
            .copied()
    }

    /// Every main record of a struct type, with its record name.
    pub fn records_of_type(&self, type_name: &str) -> Vec<(String, Record)> {
        self.db
            .records_by_type_name(type_name)
            .filter(|r| self.db.is_main_record(r))
            .map(|r| {
                let off = r.name_offset;
                (self.db.resolve_string2(off).to_string(), *r)
            })
            .collect()
    }
}

/// GUID of a reference-like value (`$ref` object, sub-record object or GUID string).
pub fn ref_id(v: &Value) -> Option<&str> {
    match v {
        Value::Object(map) => map
            .get("$ref")
            .or_else(|| map.get("_RecordId_"))
            .and_then(Value::as_str),
        Value::String(s) if s.len() == 36 && s.matches('-').count() == 4 => Some(s.as_str()),
        _ => None,
    }
}

/// `_RecordValue_` of a materialized record (or the value itself if absent).
pub fn record_value(v: &Rc<Value>) -> Rc<Value> {
    match v.get("_RecordValue_") {
        Some(inner) => Rc::new(inner.clone()),
        None => Rc::clone(v),
    }
}

/// Record name of a materialized record tree.
pub fn record_name(v: &Value) -> Option<&str> {
    v.get("_RecordName_").and_then(Value::as_str)
}
