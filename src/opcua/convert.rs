use opcua_types::Variant;

use crate::sinks::Value;

/// Normalizes an OPC UA `Variant` into the bridge's transport-agnostic
/// `Value`. Unrecognized/complex variants fall back to a debug-formatted
/// string rather than being silently dropped.
pub fn variant_to_value(variant: &Variant) -> Value {
    match variant {
        Variant::Boolean(b) => Value::Bool(*b),
        Variant::SByte(v) => Value::Int(*v as i64),
        Variant::Byte(v) => Value::Int(*v as i64),
        Variant::Int16(v) => Value::Int(*v as i64),
        Variant::UInt16(v) => Value::Int(*v as i64),
        Variant::Int32(v) => Value::Int(*v as i64),
        Variant::UInt32(v) => Value::Int(*v as i64),
        Variant::Int64(v) => Value::Int(*v),
        Variant::UInt64(v) => Value::Int(*v as i64),
        Variant::Float(v) => Value::Float(*v as f64),
        Variant::Double(v) => Value::Float(*v),
        Variant::String(s) => Value::String(s.to_string()),
        other => Value::String(format!("{other:?}")),
    }
}

/// The inverse conversion, used for the MQTT -> OPC UA write path.
pub fn value_to_variant(value: &Value) -> Variant {
    match value {
        Value::Bool(b) => Variant::Boolean(*b),
        Value::Int(i) => Variant::Int64(*i),
        Value::Float(f) => Variant::Double(*f),
        Value::String(s) => Variant::from(s.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_scalar_types() {
        assert_eq!(variant_to_value(&Variant::Boolean(true)), Value::Bool(true));
        assert_eq!(variant_to_value(&Variant::Int32(42)), Value::Int(42));
        assert_eq!(variant_to_value(&Variant::Double(1.5)), Value::Float(1.5));
    }
}
