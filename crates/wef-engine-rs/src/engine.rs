use std::{cell::RefCell, rc::Rc, time::Instant};

use boa_engine::{
    Context, JsString, JsValue, Source,
    error::JsError,
    module::{Module, SimpleModuleLoader},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use wef_core::{
    Capability, Filter, ImageRequest, ImageRequestInput, MangaListInput, MangaPage, MangaUpdate,
    MangaUpdateInput, MigrateChapterKeyInput, MigrateMangaKeyInput, PagesInput, ResolveUrlInput,
    ResolvedUrl, SearchInput, Setting, SettingKind,
};

use crate::{
    error::EngineError,
    host::{HostHandle, WefHost},
    package::Package,
    runtime::context_value,
};

/// The four core operations defined by WEF 0.0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    GetMangaList,
    Search,
    GetMangaUpdate,
    GetPages,
}

impl Operation {
    const ALL: [Self; 4] = [
        Self::GetMangaList,
        Self::Search,
        Self::GetMangaUpdate,
        Self::GetPages,
    ];

    pub fn export_name(self) -> &'static str {
        match self {
            Self::GetMangaList => "getMangaList",
            Self::Search => "search",
            Self::GetMangaUpdate => "getMangaUpdate",
            Self::GetPages => "getPages",
        }
    }
}

/// Optional source operations enabled by the corresponding manifest capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionOperation {
    GetSettings,
    GetFilters,
    ResolveUrl,
    GetImageRequest,
    MigrateMangaKey,
    MigrateChapterKey,
}

impl ExtensionOperation {
    pub fn export_name(self) -> &'static str {
        match self {
            Self::GetSettings => "getSettings",
            Self::GetFilters => "getFilters",
            Self::ResolveUrl => "resolveUrl",
            Self::GetImageRequest => "getImageRequest",
            Self::MigrateMangaKey => "migrateMangaKey",
            Self::MigrateChapterKey => "migrateChapterKey",
        }
    }

    fn enabled(self, package: &Package) -> bool {
        let caps = &package.manifest().capabilities;
        match self {
            Self::GetSettings => caps.settings,
            Self::GetFilters => caps.filters,
            Self::ResolveUrl => caps.url_resolution,
            Self::GetImageRequest => caps.image_requests,
            Self::MigrateMangaKey | Self::MigrateChapterKey => caps.migrations,
        }
    }
}

/// Executes WEF source modules.
pub struct Engine {
    host: Option<HostHandle>,
    settings: serde_json::Map<String, Value>,
}

/// Binary input for the privileged WEF 0.0.2 `transformImage` operation.
#[derive(Debug, Clone)]
pub struct ImageTransformInput {
    pub request: ImageRequest,
    pub page: wef_core::Page,
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
    pub mime_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageTransformOutput {
    pub mime_type: String,
    pub body: Vec<u8>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::without_host()
    }
}

impl Engine {
    pub fn without_host() -> Self {
        Self {
            host: None,
            settings: serde_json::Map::new(),
        }
    }

    pub fn with_host<H>(host: H) -> Self
    where
        H: WefHost + 'static,
    {
        Self {
            host: Some(Rc::new(RefCell::new(host))),
            settings: serde_json::Map::new(),
        }
    }

    /// Supplies host configuration values to a WEF 0.0.2 source through `ctx.settings`.
    pub fn with_settings(mut self, settings: serde_json::Map<String, Value>) -> Self {
        self.settings = settings;
        self
    }

    /// Evaluates a package and checks that every manifest-enabled operation is
    /// exported as a callable function. This does not invoke source operations.
    pub fn validate_package(&self, package: &Package) -> Result<(), EngineError> {
        let loader = Rc::new(SimpleModuleLoader::new(package.root()).map_err(|error| {
            EngineError::InvalidPackage {
                message: format!("could not configure package module loader: {error}"),
            }
        })?);
        let mut context = Context::builder()
            .module_loader(Rc::clone(&loader))
            .build()
            .map_err(|error| EngineError::InvalidPackage {
                message: format!("could not create JavaScript context: {error}"),
            })?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(1_000_000);
        let source = Source::from_filepath(package.entry_path())?;
        let module = Module::parse(source, None, &mut context)
            .map_err(|error| self.javascript_error(error, "package validation", &mut context))?;
        loader.insert(package.entry_path().to_path_buf(), module.clone());
        module
            .load_link_evaluate(&mut context)
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, "package validation", &mut context))?;

        for operation in Operation::ALL {
            self.require_callable(&module, operation.export_name(), &mut context)?;
        }
        for operation in [
            ExtensionOperation::GetSettings,
            ExtensionOperation::GetFilters,
            ExtensionOperation::ResolveUrl,
            ExtensionOperation::GetImageRequest,
            ExtensionOperation::MigrateMangaKey,
            ExtensionOperation::MigrateChapterKey,
        ] {
            if operation.enabled(package) {
                self.require_callable(&module, operation.export_name(), &mut context)?;
            }
        }
        if package.manifest().capabilities.image_transforms {
            self.require_callable(&module, "transformImage", &mut context)?;
        }
        Ok(())
    }

    /// Runs `transformImage` across the non-JSON binary boundary.
    pub fn run_image_transform(
        &self,
        package: &Package,
        input: ImageTransformInput,
    ) -> Result<ImageTransformOutput, EngineError> {
        const MAX_DURATION_MS: u128 = 5_000;
        let limits = crate::image::ImageLimits::default();
        if !package.manifest().capabilities.image_transforms {
            return Err(EngineError::ExtensionNotEnabled {
                operation: "transformImage",
            });
        }
        if input.body.len() > limits.max_input_bytes {
            return Err(EngineError::InvalidInput {
                operation: "transformImage",
                message: "body exceeds host byte limit".into(),
            });
        }
        let loader = Rc::new(SimpleModuleLoader::new(package.root()).map_err(|error| {
            EngineError::InvalidPackage {
                message: format!("could not configure package module loader: {error}"),
            }
        })?);
        let mut context = Context::builder()
            .module_loader(Rc::clone(&loader))
            .build()
            .map_err(|error| EngineError::InvalidPackage {
                message: format!("could not create JavaScript context: {error}"),
            })?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(1_000_000);
        let source = Source::from_filepath(package.entry_path())?;
        let module = Module::parse(source, None, &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        loader.insert(package.entry_path().to_path_buf(), module.clone());
        module
            .load_link_evaluate(&mut context)
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let function = self.require_callable(&module, "transformImage", &mut context)?;
        let value = serde_json::json!({
            "request": input.request,
            "page": input.page,
            "status": input.status,
            "headers": input.headers,
            "mimeType": input.mime_type,
        });
        let js_input = JsValue::from_json(&value, &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let object = js_input
            .as_object()
            .ok_or_else(|| EngineError::InvalidInput {
                operation: "transformImage",
                message: "could not create input object".into(),
            })?;
        object
            .set(
                boa_engine::JsString::from("body"),
                crate::image::array_buffer_value(input.body, &mut context).map_err(|error| {
                    self.javascript_error(error, "transformImage", &mut context)
                })?,
                true,
                &mut context,
            )
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let settings = self.effective_settings(package)?;
        let ctx = context_value(
            package.manifest(),
            self.host.as_ref(),
            &settings,
            &mut context,
        )?;
        let started = Instant::now();
        let result = function
            .call(&JsValue::undefined(), &[ctx, js_input], &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let promise = result
            .as_promise()
            .ok_or_else(|| EngineError::InvalidResponse {
                operation: "transformImage",
                message: "operation must return a Promise".into(),
            })?;
        let result = promise
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        if started.elapsed().as_millis() > MAX_DURATION_MS {
            return Err(EngineError::InvalidResponse {
                operation: "transformImage",
                message: "operation exceeded host duration limit".into(),
            });
        }
        let object = result
            .as_object()
            .ok_or_else(|| EngineError::InvalidResponse {
                operation: "transformImage",
                message: "expected object output".into(),
            })?;
        let mime_type = object
            .get(boa_engine::JsString::from("mimeType"), &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?
            .to_string(&mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?
            .to_std_string_escaped();
        let body = object
            .get(boa_engine::JsString::from("body"), &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let body = crate::image::array_buffer_bytes(&body)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        if body.len() > limits.max_output_bytes {
            return Err(EngineError::InvalidResponse {
                operation: "transformImage",
                message: "body exceeds host byte limit".into(),
            });
        }
        Ok(ImageTransformOutput { mime_type, body })
    }

    /// Runs one core operation and validates its JSON result against the WEF model.
    pub fn run(
        &self,
        package: &Package,
        operation: Operation,
        input: Value,
    ) -> Result<Value, EngineError> {
        self.validate_runtime_capabilities(package)?;
        self.validate_input(package, operation, &input)?;
        let settings = self.effective_settings(package)?;
        let output = self
            .invoke(package, operation.export_name(), &input, true, &settings)
            .map_err(|error| redact_settings_error(error, &self.settings))?;
        self.validate_output(operation, &input, &output)?;
        Ok(output)
    }

    /// Runs a manifest-enabled optional operation and validates its result.
    pub fn run_extension(
        &self,
        package: &Package,
        operation: ExtensionOperation,
        input: Value,
    ) -> Result<Value, EngineError> {
        self.validate_runtime_capabilities(package)?;
        if !operation.enabled(package) {
            return Err(EngineError::ExtensionNotEnabled {
                operation: operation.export_name(),
            });
        }
        self.validate_extension_input(operation, &input)?;
        let settings = if matches!(operation, ExtensionOperation::GetSettings) {
            self.settings.clone()
        } else {
            self.effective_settings(package)?
        };
        let output = self
            .invoke(package, operation.export_name(), &input, true, &settings)
            .map_err(|error| redact_settings_error(error, &self.settings))?;
        self.validate_extension_output(operation, &output)?;
        Ok(output)
    }

    fn invoke(
        &self,
        package: &Package,
        export_name: &'static str,
        input: &Value,
        require_core: bool,
        settings: &serde_json::Map<String, Value>,
    ) -> Result<Value, EngineError> {
        let loader = Rc::new(SimpleModuleLoader::new(package.root()).map_err(|error| {
            EngineError::InvalidPackage {
                message: format!("could not configure package module loader: {error}"),
            }
        })?);
        let mut context = Context::builder()
            .module_loader(Rc::clone(&loader))
            .build()
            .map_err(|error| EngineError::InvalidPackage {
                message: format!("could not create JavaScript context: {error}"),
            })?;
        let source = Source::from_filepath(package.entry_path())?;
        let module = Module::parse(source, None, &mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        loader.insert(package.entry_path().to_path_buf(), module.clone());
        module
            .load_link_evaluate(&mut context)
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;

        if require_core {
            for operation in Operation::ALL {
                self.require_callable(&module, operation.export_name(), &mut context)?;
            }
        }
        let function = self.require_callable(&module, export_name, &mut context)?;
        let input_value = JsValue::from_json(input, &mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        let context_value = context_value(
            package.manifest(),
            self.host.as_ref(),
            settings,
            &mut context,
        )?;
        let result = function
            .call(
                &JsValue::undefined(),
                &[context_value, input_value],
                &mut context,
            )
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        let promise = result
            .as_promise()
            .ok_or_else(|| EngineError::InvalidResponse {
                operation: export_name,
                message: "operation must return a Promise".into(),
            })?;
        let result = promise
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        result
            .to_json(&mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?
            .ok_or_else(|| EngineError::InvalidResponse {
                operation: export_name,
                message: "operation returned undefined".into(),
            })
    }

    fn effective_settings(
        &self,
        package: &Package,
    ) -> Result<serde_json::Map<String, Value>, EngineError> {
        if !package.manifest().capabilities.settings {
            return Ok(self.settings.clone());
        }
        let schema = self.invoke(package, "getSettings", &Value::Null, true, &self.settings)?;
        let settings: Vec<Setting> =
            serde_json::from_value(schema).map_err(|error| EngineError::InvalidResponse {
                operation: "getSettings",
                message: format!("expected Setting[]: {error}"),
            })?;
        let mut effective = self.settings.clone();
        for setting in settings {
            if !effective.contains_key(&setting.id)
                && let Some(default) = setting_default(&setting.kind)
            {
                effective.insert(setting.id, default);
            }
        }
        Ok(effective)
    }

    fn require_callable(
        &self,
        module: &Module,
        export_name: &'static str,
        context: &mut Context,
    ) -> Result<boa_engine::object::JsObject, EngineError> {
        let exported = module
            .get_value(JsString::from(export_name), context)
            .map_err(|error| self.javascript_error(error, export_name, context))?;
        exported
            .as_object()
            .filter(|object| object.is_callable())
            .ok_or(EngineError::MissingExport {
                operation: export_name,
            })
    }

    fn validate_runtime_capabilities(&self, package: &Package) -> Result<(), EngineError> {
        if let Some(host) = &self.host {
            host.borrow_mut().set_rate_limit(
                package
                    .manifest()
                    .network
                    .as_ref()
                    .and_then(|network| network.rate_limit.clone()),
            );
        }
        for capability in &package.manifest().requires {
            if self.host.is_none() {
                let capability = match capability {
                    Capability::Http => Some("http"),
                    Capability::Browser => Some("browser"),
                    _ => None,
                };
                if let Some(capability) = capability {
                    return Err(EngineError::MissingHostCapability { capability });
                }
            }
        }
        Ok(())
    }

    fn validate_input(
        &self,
        package: &Package,
        operation: Operation,
        input: &Value,
    ) -> Result<(), EngineError> {
        match operation {
            Operation::GetMangaList => {
                let input: MangaListInput = parse_input(input, operation.export_name())?;
                if input.page == 0 {
                    return Err(invalid_input(
                        operation.export_name(),
                        "page must start at 1",
                    ));
                }
                if !package.manifest().has_listing(&input.listing_id) {
                    return Err(invalid_input(
                        operation.export_name(),
                        format!("unknown listing id {:?}", input.listing_id),
                    ));
                }
            }
            Operation::Search => {
                let input: SearchInput = parse_input(input, operation.export_name())?;
                if input.page == 0 {
                    return Err(invalid_input(
                        operation.export_name(),
                        "page must start at 1",
                    ));
                }
            }
            Operation::GetMangaUpdate => {
                parse_input::<MangaUpdateInput>(input, operation.export_name())?
                    .validate()
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?
            }
            Operation::GetPages => {
                let input: PagesInput = parse_input(input, operation.export_name())?;
                input
                    .manga
                    .validate()
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
                input
                    .chapter
                    .validate()
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
            }
        }
        Ok(())
    }

    fn validate_output(
        &self,
        operation: Operation,
        input: &Value,
        output: &Value,
    ) -> Result<(), EngineError> {
        let invalid = |message: String| EngineError::InvalidResponse {
            operation: operation.export_name(),
            message,
        };
        match operation {
            Operation::GetMangaList | Operation::Search => {
                let page: MangaPage = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected MangaPage: {e}")))?;
                page.validate().map_err(|e| invalid(e.to_string()))?;
            }
            Operation::GetMangaUpdate => {
                let input: MangaUpdateInput = parse_input(input, operation.export_name())?;
                let update: MangaUpdate = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected MangaUpdate: {e}")))?;
                update
                    .validate_for(&input)
                    .map_err(|e| invalid(e.to_string()))?;
            }
            Operation::GetPages => {
                let pages: Vec<wef_core::Page> = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected Page[]: {e}")))?;
                for page in pages {
                    page.validate().map_err(|e| invalid(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    fn validate_extension_input(
        &self,
        operation: ExtensionOperation,
        input: &Value,
    ) -> Result<(), EngineError> {
        match operation {
            ExtensionOperation::GetSettings | ExtensionOperation::GetFilters => {
                if !input.is_null() {
                    return Err(invalid_input(operation.export_name(), "input must be null"));
                }
            }
            ExtensionOperation::ResolveUrl => {
                parse_input::<ResolveUrlInput>(input, operation.export_name())?;
            }
            ExtensionOperation::GetImageRequest => {
                parse_input::<ImageRequestInput>(input, operation.export_name())?;
            }
            ExtensionOperation::MigrateMangaKey => {
                parse_input::<MigrateMangaKeyInput>(input, operation.export_name())?;
            }
            ExtensionOperation::MigrateChapterKey => {
                parse_input::<MigrateChapterKeyInput>(input, operation.export_name())?;
            }
        }
        Ok(())
    }

    fn validate_extension_output(
        &self,
        operation: ExtensionOperation,
        output: &Value,
    ) -> Result<(), EngineError> {
        let invalid = |message: String| EngineError::InvalidResponse {
            operation: operation.export_name(),
            message,
        };
        match operation {
            ExtensionOperation::GetSettings => {
                let settings: Vec<Setting> = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected Setting[]: {e}")))?;
                validate_unique_ids(
                    settings.iter().map(|setting| setting.id.as_str()),
                    "setting",
                    &invalid,
                )?;
            }
            ExtensionOperation::GetFilters => {
                let filters: Vec<Filter> = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected Filter[]: {e}")))?;
                validate_filters(&filters, &invalid)?;
            }
            ExtensionOperation::ResolveUrl => {
                if !output.is_null() {
                    serde_json::from_value::<ResolvedUrl>(output.clone())
                        .map_err(|e| invalid(format!("expected resolved URL or null: {e}")))?;
                }
            }
            ExtensionOperation::GetImageRequest => {
                let request: ImageRequest = serde_json::from_value(output.clone())
                    .map_err(|e| invalid(format!("expected ImageRequest: {e}")))?;
                if request.url.is_empty()
                    || request.candidates.as_ref().is_some_and(|candidates| {
                        candidates.iter().any(|candidate| candidate.url.is_empty())
                    })
                {
                    return Err(invalid("image request URLs must not be empty".into()));
                }
            }
            ExtensionOperation::MigrateMangaKey | ExtensionOperation::MigrateChapterKey => {
                if output.as_str().is_none_or(str::is_empty) {
                    return Err(invalid("expected a non-empty key string".into()));
                }
            }
        }
        Ok(())
    }

    fn javascript_error(
        &self,
        error: JsError,
        operation: &'static str,
        context: &mut Context,
    ) -> EngineError {
        if error
            .as_native()
            .is_some_and(boa_engine::error::JsNativeError::is_runtime_limit)
        {
            return EngineError::InvalidResponse {
                operation,
                message: "operation exceeded host execution limit".into(),
            };
        }
        let opaque = error.to_opaque(context);
        if let Ok(Some(Value::Object(object))) = opaque.to_json(context)
            && object
                .get("__wefError")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return EngineError::Source {
                operation,
                code: object
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("SOURCE_ERROR")
                    .into(),
                message: object
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("source reported an error")
                    .into(),
                details: object.get("details").cloned(),
            };
        }
        EngineError::Javascript {
            operation,
            message: error.to_string(),
        }
    }
}

fn parse_input<T: DeserializeOwned>(
    input: &Value,
    operation: &'static str,
) -> Result<T, EngineError> {
    serde_json::from_value(input.clone())
        .map_err(|error| invalid_input(operation, error.to_string()))
}

fn invalid_input(operation: &'static str, message: impl Into<String>) -> EngineError {
    EngineError::InvalidInput {
        operation,
        message: message.into(),
    }
}

fn redact_settings_error(
    mut error: EngineError,
    settings: &serde_json::Map<String, Value>,
) -> EngineError {
    fn redact(value: &mut Value, settings: &serde_json::Map<String, Value>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if settings.contains_key(key) {
                        *value = Value::String("[REDACTED]".into());
                    } else {
                        redact(value, settings);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    redact(item, settings);
                }
            }
            _ => {}
        }
    }
    if let EngineError::Source {
        message, details, ..
    } = &mut error
    {
        for value in settings.values().filter_map(Value::as_str) {
            *message = message.replace(value, "[REDACTED]");
        }
        if let Some(details) = details {
            redact(details, settings);
        }
    }
    error
}

fn setting_default(kind: &SettingKind) -> Option<Value> {
    match kind {
        SettingKind::Text { default } | SettingKind::Select { default, .. } => {
            default.clone().map(Value::String)
        }
        SettingKind::Toggle { default } => default.map(Value::Bool),
        SettingKind::MultiSelect { default, .. } => default
            .clone()
            .map(|items| Value::Array(items.into_iter().map(Value::String).collect())),
    }
}

fn validate_unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    kind: &'static str,
    invalid: &impl Fn(String) -> EngineError,
) -> Result<(), EngineError> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if id.is_empty() || !seen.insert(id) {
            return Err(invalid(format!("{kind} IDs must be non-empty and unique")));
        }
    }
    Ok(())
}

fn validate_filters(
    filters: &[Filter],
    invalid: &impl Fn(String) -> EngineError,
) -> Result<(), EngineError> {
    fn visit<'a>(filters: &'a [Filter], ids: &mut Vec<&'a str>) {
        for filter in filters {
            ids.push(&filter.id);
            if let wef_core::FilterKind::Group { children, .. } = &filter.kind {
                visit(children, ids);
            }
        }
    }
    let mut ids = Vec::new();
    visit(filters, &mut ids);
    validate_unique_ids(ids.into_iter(), "filter", invalid)
}
