use std::{collections::BTreeMap, rc::Rc, time::Instant};

use boa_engine::{Context, JsString, JsValue, Source, error::JsError, module::Module};
use serde_json::Value;
use wef_core::{
    Capability, Filter, ImageRequest, ImageRequestInput, MangaListInput, MangaPage, MangaUpdate,
    MangaUpdateInput, MigrateChapterKeyInput, MigrateMangaKeyInput, PagesInput, ResolveUrlInput,
    ResolvedUrl, SearchInput, Setting, SettingKind,
};

use crate::{
    error::EngineError,
    host::{HostHandle, StoreRegistry, WefHost},
    loader::JailedModuleLoader,
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
    stores: StoreRegistry,
}

/// Binary input for the privileged WEF 0.0.2 `transformImage` operation.
#[derive(Debug, Clone)]
pub struct ImageTransformInput {
    pub request: ImageRequest,
    pub page: wef_core::Page,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
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
            stores: StoreRegistry::new(),
        }
    }

    /// Ports: `init(host: WefHost)` in Kotlin/Swift — the host arrives as an
    /// interface value; Rust spells that `Box<dyn WefHost>`.
    pub fn new(host: Box<dyn crate::host::WefHost>) -> Self {
        Self {
            host: Some(HostHandle::new(host)),
            settings: serde_json::Map::new(),
            stores: StoreRegistry::new(),
        }
    }

    /// Ports: same `init(host: WefHost)` call with any host implementation.
    /// The `'static` bound only says the engine may keep the host for as
    /// long as it lives — Kotlin/Swift hold the same reference.
    pub fn with_host(host: impl WefHost + 'static) -> Self {
        Self::new(Box::new(host))
    }

    /// Supplies host configuration values to a WEF 0.0.2 source through `ctx.settings`.
    pub fn with_settings(mut self, settings: serde_json::Map<String, Value>) -> Self {
        self.settings = settings;
        self
    }

    /// Snapshots every source store as plain JSON (for CLI `--store` files).
    pub fn store_snapshot(&self) -> Value {
        self.stores.snapshot()
    }

    /// Restores a snapshot produced by [`Engine::store_snapshot`]; malformed
    /// entries are skipped, never fatal.
    pub fn restore_store(&self, snapshot: &Value) {
        self.stores.restore(snapshot);
    }

    /// Evaluates a package and checks that every manifest-enabled operation is
    /// exported as a callable function. This does not invoke source operations.
    pub fn validate_package(&self, package: &Package) -> Result<(), EngineError> {
        let (mut context, module) = self.load_module(package, "package validation")?;

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
        let (mut context, module) = self.load_module(package, "transformImage")?;
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
        let object = match js_input.as_object() {
            Some(object) => object,
            None => {
                return Err(EngineError::InvalidInput {
                    operation: "transformImage",
                    message: "could not create input object".into(),
                });
            }
        };
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
        let ctx = self.context_for(package, &settings, &mut context)?;
        let started = Instant::now();
        let result = function
            .call(&JsValue::undefined(), &[ctx, js_input], &mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        let promise = match result.as_promise() {
            Some(promise) => promise,
            None => {
                return Err(EngineError::InvalidResponse {
                    operation: "transformImage",
                    message: "operation must return a Promise".into(),
                });
            }
        };
        let result = promise
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, "transformImage", &mut context))?;
        if started.elapsed().as_millis() > MAX_DURATION_MS {
            return Err(EngineError::InvalidResponse {
                operation: "transformImage",
                message: "operation exceeded host duration limit".into(),
            });
        }
        let object = match result.as_object() {
            Some(object) => object,
            None => {
                return Err(EngineError::InvalidResponse {
                    operation: "transformImage",
                    message: "expected object output".into(),
                });
            }
        };
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
            .invoke(package, operation.export_name(), &input, &settings)
            .map_err(|error| self.redact_error(error, package))?;
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
        let settings = if operation == ExtensionOperation::GetSettings {
            self.settings.clone()
        } else {
            self.effective_settings(package)?
        };
        let output = self
            .invoke(package, operation.export_name(), &input, &settings)
            .map_err(|error| self.redact_error(error, package))?;
        self.validate_extension_output(operation, &output)?;
        Ok(output)
    }

    /// Scrubs secret-equivalent values (host settings + this source's store)
    /// from source errors. Ports: same two-map scrub after every invocation.
    fn redact_error(&self, error: EngineError, package: &Package) -> EngineError {
        let snapshot = self.stores.snapshot_for(&package.manifest().id);
        redact_error(error, &self.settings, &snapshot)
    }

    /// Builds the `ctx` value for one invocation, attaching the source store
    /// only when the manifest declares `requires: ["storage"]`.
    /// Ports: `if manifest.requires_storage { ctx.store = stores.getOrPut(id) }`.
    fn context_for(
        &self,
        package: &Package,
        settings: &serde_json::Map<String, Value>,
        context: &mut Context,
    ) -> Result<JsValue, EngineError> {
        let manifest = package.manifest();
        let store = if manifest.requires.contains(&Capability::Storage) {
            Some(self.stores.store_for(&manifest.id))
        } else {
            None
        };
        context_value(
            manifest,
            self.host.as_ref(),
            settings,
            store.as_ref(),
            context,
        )
    }

    fn load_module(
        &self,
        package: &Package,
        operation: &'static str,
    ) -> Result<(Context, Module), EngineError> {
        let loader = Rc::new(
            JailedModuleLoader::new(package.root().to_path_buf()).map_err(|error| {
                EngineError::InvalidPackage {
                    message: format!("could not configure package module loader: {error}"),
                }
            })?,
        );
        let mut context = Context::builder()
            .module_loader(Rc::clone(&loader))
            .build()
            .map_err(|error| EngineError::InvalidPackage {
                message: format!("could not create JavaScript context: {error}"),
            })?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(1_000_000);
        let source = Source::from_filepath(package.entry_path()).map_err(EngineError::Io)?;
        let module = Module::parse(source, None, &mut context)
            .map_err(|error| self.javascript_error(error, operation, &mut context))?;
        loader.insert(package.entry_path().to_path_buf(), module.clone());
        module
            .load_link_evaluate(&mut context)
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, operation, &mut context))?;
        Ok((context, module))
    }

    fn invoke(
        &self,
        package: &Package,
        export_name: &'static str,
        input: &Value,
        settings: &serde_json::Map<String, Value>,
    ) -> Result<Value, EngineError> {
        let (mut context, module) = self.load_module(package, export_name)?;

        for operation in Operation::ALL {
            self.require_callable(&module, operation.export_name(), &mut context)?;
        }
        let function = self.require_callable(&module, export_name, &mut context)?;
        let input_value = JsValue::from_json(input, &mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        let context_value = self.context_for(package, settings, &mut context)?;
        let result = function
            .call(
                &JsValue::undefined(),
                &[context_value, input_value],
                &mut context,
            )
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        let promise = match result.as_promise() {
            Some(promise) => promise,
            None => {
                return Err(EngineError::InvalidResponse {
                    operation: export_name,
                    message: "operation must return a Promise".into(),
                });
            }
        };
        let result = promise
            .await_blocking(&mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        let json = result
            .to_json(&mut context)
            .map_err(|error| self.javascript_error(error, export_name, &mut context))?;
        match json {
            Some(json) => Ok(json),
            None => Err(EngineError::InvalidResponse {
                operation: export_name,
                message: "operation returned undefined".into(),
            }),
        }
    }

    fn effective_settings(
        &self,
        package: &Package,
    ) -> Result<serde_json::Map<String, Value>, EngineError> {
        if !package.manifest().capabilities.settings {
            return Ok(self.settings.clone());
        }
        let schema = self.invoke(package, "getSettings", &Value::Null, &self.settings)?;
        let settings: Vec<Setting> = match serde_json::from_value(schema) {
            Ok(settings) => settings,
            Err(error) => {
                return Err(EngineError::InvalidResponse {
                    operation: "getSettings",
                    message: format!("expected Setting[]: {error}"),
                });
            }
        };
        let mut effective = self.settings.clone();
        for setting in settings {
            if effective.contains_key(&setting.id) {
                continue;
            }
            if let Some(default) = setting_default(&setting.kind) {
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
        let object = match exported.as_object() {
            Some(object) => object,
            None => {
                return Err(EngineError::MissingExport {
                    operation: export_name,
                });
            }
        };
        if !object.is_callable() {
            return Err(EngineError::MissingExport {
                operation: export_name,
            });
        }
        Ok(object)
    }

    fn validate_runtime_capabilities(&self, package: &Package) -> Result<(), EngineError> {
        if let Some(host) = &self.host {
            let rate_limit = match package.manifest().network.as_ref() {
                Some(network) => network.rate_limit.clone(),
                None => None,
            };
            host.set_rate_limit(rate_limit);
            // The manifest allowlist is authoritative per run: plain HTTP,
            // image fetches, and every redirect hop may only contact URLs
            // under these entries. Ports: same push before dispatch.
            host.set_allowed_urls(&package.manifest().base_urls);
        }
        for capability in &package.manifest().requires {
            if self.host.is_none() {
                if *capability == Capability::Http {
                    return Err(EngineError::MissingHostCapability { capability: "http" });
                }
                if *capability == Capability::Browser {
                    return Err(EngineError::MissingHostCapability {
                        capability: "browser",
                    });
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
                let input: MangaListInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
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
                let input: SearchInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
                if input.page == 0 {
                    return Err(invalid_input(
                        operation.export_name(),
                        "page must start at 1",
                    ));
                }
            }
            Operation::GetMangaUpdate => {
                let input: MangaUpdateInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
                input
                    .validate()
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
            }
            Operation::GetPages => {
                let input: PagesInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(operation.export_name(), error.to_string()))?;
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
        let name = operation.export_name();
        match operation {
            Operation::GetMangaList | Operation::Search => {
                let page: MangaPage = match serde_json::from_value(output.clone()) {
                    Ok(page) => page,
                    Err(error) => {
                        return Err(invalid_response(
                            name,
                            format!("expected MangaPage: {error}"),
                        ));
                    }
                };
                page.validate()
                    .map_err(|error| invalid_response(name, error.to_string()))?;
            }
            Operation::GetMangaUpdate => {
                let input: MangaUpdateInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(name, error.to_string()))?;
                let update: MangaUpdate = match serde_json::from_value(output.clone()) {
                    Ok(update) => update,
                    Err(error) => {
                        return Err(invalid_response(
                            name,
                            format!("expected MangaUpdate: {error}"),
                        ));
                    }
                };
                update
                    .validate_for(&input)
                    .map_err(|error| invalid_response(name, error.to_string()))?;
            }
            Operation::GetPages => {
                let pages: Vec<wef_core::Page> = match serde_json::from_value(output.clone()) {
                    Ok(pages) => pages,
                    Err(error) => {
                        return Err(invalid_response(name, format!("expected Page[]: {error}")));
                    }
                };
                for page in pages {
                    page.validate()
                        .map_err(|error| invalid_response(name, error.to_string()))?;
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
        let name = operation.export_name();
        match operation {
            ExtensionOperation::GetSettings | ExtensionOperation::GetFilters => {
                if !input.is_null() {
                    return Err(invalid_input(name, "input must be null"));
                }
            }
            ExtensionOperation::ResolveUrl => {
                let _: ResolveUrlInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(name, error.to_string()))?;
            }
            ExtensionOperation::GetImageRequest => {
                let _: ImageRequestInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(name, error.to_string()))?;
            }
            ExtensionOperation::MigrateMangaKey => {
                let _: MigrateMangaKeyInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(name, error.to_string()))?;
            }
            ExtensionOperation::MigrateChapterKey => {
                let _: MigrateChapterKeyInput = serde_json::from_value(input.clone())
                    .map_err(|error| invalid_input(name, error.to_string()))?;
            }
        }
        Ok(())
    }

    fn validate_extension_output(
        &self,
        operation: ExtensionOperation,
        output: &Value,
    ) -> Result<(), EngineError> {
        let name = operation.export_name();
        match operation {
            ExtensionOperation::GetSettings => {
                let settings: Vec<Setting> = match serde_json::from_value(output.clone()) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Err(invalid_response(
                            name,
                            format!("expected Setting[]: {error}"),
                        ));
                    }
                };
                let mut ids: Vec<String> = Vec::new();
                for setting in &settings {
                    ids.push(setting.id.clone());
                }
                validate_unique_ids("setting", &ids, name)?;
            }
            ExtensionOperation::GetFilters => {
                let filters: Vec<Filter> = match serde_json::from_value(output.clone()) {
                    Ok(filters) => filters,
                    Err(error) => {
                        return Err(invalid_response(
                            name,
                            format!("expected Filter[]: {error}"),
                        ));
                    }
                };
                validate_filters(&filters, name)?;
            }
            ExtensionOperation::ResolveUrl => {
                if !output.is_null() {
                    let _: ResolvedUrl = match serde_json::from_value(output.clone()) {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            return Err(invalid_response(
                                name,
                                format!("expected resolved URL or null: {error}"),
                            ));
                        }
                    };
                }
            }
            ExtensionOperation::GetImageRequest => {
                let request: ImageRequest = match serde_json::from_value(output.clone()) {
                    Ok(request) => request,
                    Err(error) => {
                        return Err(invalid_response(
                            name,
                            format!("expected ImageRequest: {error}"),
                        ));
                    }
                };
                if request.url.is_empty() {
                    return Err(invalid_response(
                        name,
                        "image request URLs must not be empty".into(),
                    ));
                }
                if let Some(candidates) = request.candidates.as_ref() {
                    for candidate in candidates {
                        if candidate.url.is_empty() {
                            return Err(invalid_response(
                                name,
                                "image request URLs must not be empty".into(),
                            ));
                        }
                    }
                }
            }
            ExtensionOperation::MigrateMangaKey | ExtensionOperation::MigrateChapterKey => {
                let key = match output.as_str() {
                    Some(key) => key,
                    None => {
                        return Err(invalid_response(
                            name,
                            "expected a non-empty key string".into(),
                        ));
                    }
                };
                if key.is_empty() {
                    return Err(invalid_response(
                        name,
                        "expected a non-empty key string".into(),
                    ));
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
        let hit_execution_limit = match error.as_native() {
            Some(native) => boa_engine::error::JsNativeError::is_runtime_limit(native),
            None => false,
        };
        if hit_execution_limit {
            return EngineError::InvalidResponse {
                operation,
                message: "operation exceeded host execution limit".into(),
            };
        }
        let opaque = error.to_opaque(context);
        let error_object = match opaque.to_json(context) {
            Ok(Some(Value::Object(object))) => object,
            _ => {
                return EngineError::Javascript {
                    operation,
                    message: error.to_string(),
                };
            }
        };
        if error_object
            .get("__wefError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return EngineError::Source {
                operation,
                code: error_object
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("SOURCE_ERROR")
                    .into(),
                message: error_object
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("source reported an error")
                    .into(),
                details: error_object.get("details").cloned(),
            };
        }
        EngineError::Javascript {
            operation,
            message: error.to_string(),
        }
    }
}

fn invalid_input(operation: &'static str, message: impl Into<String>) -> EngineError {
    EngineError::InvalidInput {
        operation,
        message: message.into(),
    }
}

fn invalid_response(operation: &'static str, message: String) -> EngineError {
    EngineError::InvalidResponse { operation, message }
}

/// Scrubs secret-equivalent values (host settings + this source's store
/// entries) from source errors. Ports: same two-map scrub after every
/// invocation — replace every secret string in `message`, and blank any
/// `details` object field named like a secret key.
fn redact_error(
    mut error: EngineError,
    settings: &serde_json::Map<String, Value>,
    store: &serde_json::Map<String, Value>,
) -> EngineError {
    if let EngineError::Source {
        message, details, ..
    } = &mut error
    {
        for secrets in [settings, store] {
            for value in secrets.values() {
                if let Some(secret) = value.as_str() {
                    *message = message.replace(secret, "[REDACTED]");
                }
            }
        }
        if let Some(details) = details {
            redact_json_value(details, settings, store);
        }
    }
    error
}

fn redact_json_value(
    value: &mut Value,
    settings: &serde_json::Map<String, Value>,
    store: &serde_json::Map<String, Value>,
) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if settings.contains_key(key) || store.contains_key(key) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_json_value(value, settings, store);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item, settings, store);
            }
        }
        _ => {}
    }
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

/// Rejects empty or repeated ids. Ports: same check over the id list.
fn validate_unique_ids(
    kind: &str,
    ids: &[String],
    operation: &'static str,
) -> Result<(), EngineError> {
    let mut seen: Vec<&str> = Vec::new();
    for id in ids {
        if id.is_empty() || seen.contains(&id.as_str()) {
            return Err(invalid_response(
                operation,
                format!("{kind} IDs must be non-empty and unique"),
            ));
        }
        seen.push(id.as_str());
    }
    Ok(())
}

fn validate_filters(filters: &[Filter], operation: &'static str) -> Result<(), EngineError> {
    let mut ids: Vec<String> = Vec::new();
    collect_filter_ids(filters, &mut ids);
    validate_unique_ids("filter", &ids, operation)
}

/// Collects every filter id, descending into groups. Ports: same
/// depth-first walk before the uniqueness check.
fn collect_filter_ids(filters: &[Filter], ids: &mut Vec<String>) {
    for filter in filters {
        ids.push(filter.id.clone());
        if let wef_core::FilterKind::Group { children, .. } = &filter.kind {
            collect_filter_ids(children, ids);
        }
    }
}
