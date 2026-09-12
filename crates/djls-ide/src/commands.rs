use tower_lsp_server::ls_types;

pub const REPORT_UNREADABLE_REGISTRATION_COMMAND: &str = "djls.reportUnreadableRegistration";

/// Wire data sent from the unreadable-registration code action to the server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReportUnreadableRegistrationParams {
    pub module: String,
    pub file: ls_types::Uri,
    /// One-based line of the first unread registration statement.
    pub line: u32,
    /// User-facing description of the unread registration shape.
    pub shape: String,
    pub count: u32,
}

impl ReportUnreadableRegistrationParams {
    #[must_use]
    pub fn into_lsp_value(self) -> ls_types::LSPAny {
        let mut fields = ls_types::LSPObject::new();
        fields.insert("module".to_string(), self.module.into());
        fields.insert("file".to_string(), self.file.to_string().into());
        fields.insert("line".to_string(), self.line.into());
        fields.insert("shape".to_string(), self.shape.into());
        fields.insert("count".to_string(), self.count.into());
        ls_types::LSPAny::Object(fields)
    }

    pub fn from_lsp_value(value: &ls_types::LSPAny) -> Result<Self, String> {
        let Some(fields) = value.as_object() else {
            return Err("argument must be an object".to_string());
        };
        if fields.len() != 5 {
            return Err(
                "argument must contain only module, file, line, shape, and count".to_string(),
            );
        }

        let string_field = |name: &str| {
            fields
                .get(name)
                .and_then(ls_types::LSPAny::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{name} must be a string"))
        };
        let number_field = |name: &str| {
            fields
                .get(name)
                .and_then(ls_types::LSPAny::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| format!("{name} must be an unsigned 32-bit integer"))
        };
        let file = string_field("file")?
            .parse()
            .map_err(|error| format!("file must be a URI: {error}"))?;

        Ok(Self {
            module: string_field("module")?,
            file,
            line: number_field("line")?,
            shape: string_field("shape")?,
            count: number_field("count")?,
        })
    }
}
