//! Agent configuration fingerprinting and headless plan hashing call this function before
//! serializing their inputs. Recursive key ordering gives equivalent objects the same hash
//! input while retaining the significance of array order.

use serde_json::Value;

/// Recursively sorts JSON object keys while preserving array order.
pub(crate) fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| (*key).clone());
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical_value(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        _ => value.clone(),
    }
}
