//! An `ExportSink` that materializes a DataCore record into a `serde_json::Value`
//! tree while **keeping the GUID of referenced main records**.
//!
//! StarBreaker's stock JSON sink serializes a reference to another main record
//! as a relative path string (`file://./...json`), which loses the record id.
//! Here every such reference becomes an object:
//!
//! ```json
//! { "$ref": "0424e501-...", "$name": "ContractTemplate.X", "$path": "libs/.../x.json" }
//! ```

use serde_json::{Map, Value, json};
use starbreaker_common::CigGuid;
use starbreaker_datacore::sink::ExportSink;

enum Frame {
    Object { name: Option<String>, map: Map<String, Value> },
    Array { name: String, items: Vec<Value> },
}

#[derive(Default)]
pub struct TreeSink {
    stack: Vec<Frame>,
    root: Option<Value>,
}

impl TreeSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the finished tree. Returns `Null` if nothing was written.
    pub fn finish(self) -> Value {
        self.root.unwrap_or(Value::Null)
    }

    fn push_value(&mut self, name: Option<&str>, value: Value) {
        match self.stack.last_mut() {
            Some(Frame::Object { map, .. }) => {
                map.insert(name.unwrap_or("").to_string(), value);
            }
            Some(Frame::Array { items, .. }) => items.push(value),
            None => self.root = Some(value),
        }
    }
}

impl ExportSink for TreeSink {
    type Error = std::io::Error;

    fn extension(&self) -> &str {
        "json"
    }

    fn begin_object(&mut self, name: Option<&str>) -> Result<(), Self::Error> {
        self.stack.push(Frame::Object { name: name.map(str::to_string), map: Map::new() });
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), Self::Error> {
        match self.stack.pop() {
            Some(Frame::Object { name, map }) => {
                self.push_value(name.as_deref(), Value::Object(map));
                Ok(())
            }
            _ => Err(std::io::Error::other("end_object without matching begin_object")),
        }
    }

    fn begin_array(&mut self, name: &str) -> Result<(), Self::Error> {
        self.stack.push(Frame::Array { name: name.to_string(), items: Vec::new() });
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), Self::Error> {
        match self.stack.pop() {
            Some(Frame::Array { name, items }) => {
                self.push_value(Some(&name), Value::Array(items));
                Ok(())
            }
            _ => Err(std::io::Error::other("end_array without matching begin_array")),
        }
    }

    fn write_null(&mut self, name: Option<&str>) -> Result<(), Self::Error> {
        self.push_value(name, Value::Null);
        Ok(())
    }

    fn write_bool(&mut self, name: Option<&str>, value: bool) -> Result<(), Self::Error> {
        self.push_value(name, Value::Bool(value));
        Ok(())
    }

    fn write_i8(&mut self, name: Option<&str>, value: i8) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_i16(&mut self, name: Option<&str>, value: i16) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_i32(&mut self, name: Option<&str>, value: i32) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_i64(&mut self, name: Option<&str>, value: i64) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_u8(&mut self, name: Option<&str>, value: u8) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_u16(&mut self, name: Option<&str>, value: u16) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_u32(&mut self, name: Option<&str>, value: u32) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_u64(&mut self, name: Option<&str>, value: u64) -> Result<(), Self::Error> {
        self.push_value(name, json!(value));
        Ok(())
    }

    fn write_f32(&mut self, name: Option<&str>, value: f32) -> Result<(), Self::Error> {
        self.push_value(name, if value.is_finite() { json!(value) } else { Value::Null });
        Ok(())
    }

    fn write_f64(&mut self, name: Option<&str>, value: f64) -> Result<(), Self::Error> {
        self.push_value(name, if value.is_finite() { json!(value) } else { Value::Null });
        Ok(())
    }

    fn write_str(&mut self, name: Option<&str>, value: &str) -> Result<(), Self::Error> {
        self.push_value(name, Value::String(value.to_string()));
        Ok(())
    }

    fn write_guid(&mut self, name: Option<&str>, value: &CigGuid) -> Result<(), Self::Error> {
        self.push_value(name, Value::String(value.to_string()));
        Ok(())
    }

    fn write_record_ref(
        &mut self,
        name: Option<&str>,
        record_id: &CigGuid,
        record_name: &str,
        path: &str,
    ) -> Result<(), Self::Error> {
        self.push_value(
            name,
            json!({
                "$ref": record_id.to_string(),
                "$name": record_name,
                "$path": path,
            }),
        );
        Ok(())
    }
}

/// Convenience: is this value a `$ref` object produced by [`TreeSink`]?
pub fn ref_id(value: &Value) -> Option<&str> {
    value.get("$ref").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_tree() {
        let mut s = TreeSink::new();
        s.begin_object(None).unwrap();
        s.write_str(Some("name"), "x").unwrap();
        s.begin_array("items").unwrap();
        s.write_i32(None, 1).unwrap();
        s.begin_object(None).unwrap();
        s.write_bool(Some("ok"), true).unwrap();
        s.end_object().unwrap();
        s.end_array().unwrap();
        s.end_object().unwrap();
        let v = s.finish();
        assert_eq!(v["name"], "x");
        assert_eq!(v["items"][0], 1);
        assert_eq!(v["items"][1]["ok"], true);
    }
}
