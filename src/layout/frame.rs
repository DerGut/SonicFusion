use std::collections::HashMap;

use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use crate::RenderConfig;

/// Exact mono frame-signal schema, including sample-rate and layout metadata.
/// Frame samples are finite, unitless values in [-1, 1].
pub fn frame_schema(config: &RenderConfig) -> SchemaRef {
    SchemaRef::new(frame_schema_val(config))
}

/// Returns an owned Schema value. This should only be used by tests
/// that need to further modify the schema. Otherwise, this is backing
/// the intended API [`frame_schema`].
pub(crate) fn frame_schema_val(config: &RenderConfig) -> Schema {
    let frame = Field::new("frame", DataType::UInt64, false);
    let sample = Field::new("sample", DataType::Float32, false);

    Schema::new_with_metadata(
        vec![frame, sample],
        HashMap::from([
            (
                "audio.sample_rate_hz".to_string(),
                config.sample_rate_hz().to_string(),
            ),
            ("audio.channels".to_string(), "1".to_string()),
            ("audio.layout".to_string(), "frame".to_string()),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_schema_contract() {
        let config = RenderConfig::builder()
            .sample_rate_hz(96_000)
            .build()
            .expect("test configuration should be valid");
        let schema = frame_schema(&config);

        assert_eq!(schema.fields().len(), 2);
        assert_eq!(schema.field(0).name(), "frame");
        assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
        assert!(!schema.field(0).is_nullable());
        assert_eq!(schema.field(1).name(), "sample");
        assert_eq!(schema.field(1).data_type(), &DataType::Float32);
        assert!(!schema.field(1).is_nullable());

        assert_eq!(schema.metadata().len(), 3);
        assert_eq!(
            schema
                .metadata()
                .get("audio.sample_rate_hz")
                .map(String::as_str),
            Some("96000")
        );
        assert_eq!(
            schema.metadata().get("audio.channels").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            schema.metadata().get("audio.layout").map(String::as_str),
            Some("frame")
        );
    }
}
