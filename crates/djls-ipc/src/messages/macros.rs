#[macro_export]
macro_rules! define_messages {
    ($( MessageType::$variant:ident {
        request: $req:ty,
        response: $resp:ty
    } ),+ $(,)?) => {
        $(
            paste::paste! {
                #[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
                pub struct [<$variant Request>] {
                    pub message: Messages,
                    pub data: $req,
                }

                #[derive(Debug, Serialize, Deserialize, JsonSchema)]
                pub struct [<$variant Response>] {
                    pub message: Messages,
                    pub success: bool,
                    pub data: Option<$resp>,
                    pub error: Option<ErrorResponse>,
                }

                #[derive(JsonSchema)]
                #[allow(dead_code)]
                pub struct [<$variant Message>] {
                    request: [<$variant Request>],
                    response: [<$variant Response>],
                }

                impl $crate::messages::Message for $resp {
                    type RequestData = $req;
                    const TYPE: $crate::messages::Messages = $crate::messages::Messages::$variant;
                }
            }
        )+

        pub fn all_message_schemas() -> Result<(String, Vec<(String, schemars::schema::RootSchema)>), SchemaError> {
            let mut combined_schema = schemars::schema::RootSchema {
                meta_schema: Some("http://json-schema.org/draft-07/schema#".to_string()),
                schema: schemars::schema::Schema::Object(schemars::schema::SchemaObject {
                    reference: Some("#/definitions/Messages".to_string()),
                    ..Default::default()
                }).into(),
                definitions: std::collections::BTreeMap::new(),
            };

            combined_schema.definitions.insert(
                "ErrorResponse".to_string(),
                schemars::schema::Schema::Object(schemars::schema_for!(ErrorResponse).schema)
            );
            combined_schema.definitions.insert(
                "Messages".to_string(),
                schemars::schema::Schema::Object(schemars::schema_for!(Messages).schema)
            );
            combined_schema.definitions.insert(
                "Request".to_string(),
                schemars::schema::Schema::Object(schemars::schema_for!(GenericRequest).schema)
            );
            combined_schema.definitions.insert(
                "Response".to_string(),
                schemars::schema::Schema::Object(schemars::schema_for!(GenericResponse).schema)
            );

            $(
                paste::paste! {
                    let name = stringify!($variant)
                        .chars()
                        .enumerate()
                        .fold(String::new(), |mut acc, (i, c)| {
                            if i > 0 && c.is_uppercase() {
                                acc.push('_');
                            }
                            acc.extend(c.to_lowercase());
                            acc
                        });

                    let message_schema = schemars::schema_for!([<$variant Message>]);
                    combined_schema.definitions.extend(message_schema.definitions);
                    combined_schema.definitions.insert(
                        format!("{}_message", name),
                        schemars::schema::Schema::Object(message_schema.schema)
                    );
                }
            )+

            // Set the root schema to be an object with all our definitions
            // This gets rid of the random Model/RootModel from appearing
            combined_schema.schema = schemars::schema::Schema::Object(schemars::schema::SchemaObject {
                ..Default::default()
            }).into();

            Ok((file!().to_string(), vec![
                ("schema".to_string(), combined_schema)
            ]))
        }
    };
}
