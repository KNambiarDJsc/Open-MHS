//! Embedded JSON Schemas and structural validation.

use std::sync::LazyLock;

use jsonschema::{Registry, Resource, Validator};

use crate::issue::Issue;

pub const COMMON_SCHEMA: &str = include_str!("../../../spec/schemas/common.schema.json");
pub const DESCRIPTOR_SCHEMA: &str = include_str!("../../../spec/schemas/descriptor.schema.json");
pub const COMMAND_SCHEMA: &str = include_str!("../../../spec/schemas/command.schema.json");
pub const RECEIPT_SCHEMA: &str = include_str!("../../../spec/schemas/receipt.schema.json");
pub const EVENT_SCHEMA: &str = include_str!("../../../spec/schemas/event.schema.json");
pub const ERROR_SCHEMA: &str = include_str!("../../../spec/schemas/error.schema.json");

/// The schemas this crate can validate against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    Descriptor,
    Command,
    Receipt,
    Event,
    Error,
}

impl Schema {
    pub const ALL: [Schema; 5] = [
        Schema::Descriptor,
        Schema::Command,
        Schema::Receipt,
        Schema::Event,
        Schema::Error,
    ];

    fn source(self) -> &'static str {
        match self {
            Schema::Descriptor => DESCRIPTOR_SCHEMA,
            Schema::Command => COMMAND_SCHEMA,
            Schema::Receipt => RECEIPT_SCHEMA,
            Schema::Event => EVENT_SCHEMA,
            Schema::Error => ERROR_SCHEMA,
        }
    }

    /// Error code reported for structural violations of this schema.
    pub fn error_code(self) -> &'static str {
        match self {
            Schema::Descriptor => "descriptor.schema",
            Schema::Command => "command.schema",
            Schema::Receipt => "receipt.schema",
            Schema::Event => "event.schema",
            Schema::Error => "error.schema",
        }
    }
}

fn parse(source: &str) -> serde_json::Value {
    // The schemas are compiled into the binary and covered by tests, so a parse
    // failure is a build defect, not a runtime condition.
    #[allow(clippy::expect_used)]
    serde_json::from_str(source).expect("embedded schema is valid JSON")
}

struct Validators {
    descriptor: Validator,
    command: Validator,
    receipt: Validator,
    event: Validator,
    error: Validator,
}

#[allow(clippy::expect_used)]
static VALIDATORS: LazyLock<Validators> = LazyLock::new(|| {
    let resources = [
        ("urn:openmhs:schema:0.1:common", COMMON_SCHEMA),
        ("urn:openmhs:schema:0.1:descriptor", DESCRIPTOR_SCHEMA),
        ("urn:openmhs:schema:0.1:command", COMMAND_SCHEMA),
        ("urn:openmhs:schema:0.1:receipt", RECEIPT_SCHEMA),
        ("urn:openmhs:schema:0.1:event", EVENT_SCHEMA),
    ];
    let mut builder = Registry::new();
    for (uri, source) in resources {
        builder = builder
            .add(uri, Resource::from_contents(parse(source)))
            .expect("embedded schema registers");
    }
    let registry = builder.prepare().expect("schema registry prepares");
    let build = |s: Schema| {
        jsonschema::draft202012::options()
            .with_registry(&registry)
            .should_validate_formats(true)
            .build(&parse(s.source()))
            .expect("embedded schema compiles")
    };
    Validators {
        descriptor: build(Schema::Descriptor),
        command: build(Schema::Command),
        receipt: build(Schema::Receipt),
        event: build(Schema::Event),
        error: build(Schema::Error),
    }
});

fn validator(schema: Schema) -> &'static Validator {
    let v = &*VALIDATORS;
    match schema {
        Schema::Descriptor => &v.descriptor,
        Schema::Command => &v.command,
        Schema::Receipt => &v.receipt,
        Schema::Event => &v.event,
        Schema::Error => &v.error,
    }
}

/// Validate `instance` against `schema`, returning every structural violation.
pub fn validate(schema: Schema, instance: &serde_json::Value) -> Vec<Issue> {
    validator(schema)
        .iter_errors(instance)
        .map(|e| {
            Issue::new(
                schema.error_code(),
                e.instance_path().to_string(),
                e.to_string(),
            )
        })
        .collect()
}
