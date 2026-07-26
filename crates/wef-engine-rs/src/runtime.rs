use std::{cell::RefCell, rc::Rc};

use boa_engine::native_function::NativeFunction;
use boa_engine::{
    Context, Finalize, JsData, JsString, JsValue, Trace,
    error::{JsError, JsNativeError},
    object::{
        ObjectInitializer,
        builtins::{JsArray, JsPromise},
    },
    property::Attribute,
};
use ego_tree::NodeId;
use scraper::{ElementRef, Html, Selector};
use url::Url;
use wef_core::{Capability, Manifest};

use crate::{
    error::EngineError,
    host::{HostHandle, HttpRequest, WefHost},
};

pub(crate) fn context_value(
    manifest: &Manifest,
    host: Option<&HostHandle>,
    settings: &serde_json::Map<String, serde_json::Value>,
    context: &mut Context,
) -> Result<JsValue, EngineError> {
    let url = ObjectInitializer::new(context)
        .function(
            NativeFunction::from_fn_ptr(resolve_url),
            JsString::from("resolve"),
            2,
        )
        .build();

    let http = if manifest
        .requires
        .iter()
        .any(|capability| matches!(capability, Capability::Http))
    {
        let host = host.ok_or(EngineError::MissingHostCapability { capability: "http" })?;
        Some(
            ObjectInitializer::with_native_data(
                HostState {
                    host: Rc::clone(host),
                },
                context,
            )
            .function(
                NativeFunction::from_fn_ptr(http_request),
                JsString::from("request"),
                1,
            )
            .build(),
        )
    } else {
        None
    };

    let html = if manifest
        .requires
        .iter()
        .any(|capability| matches!(capability, Capability::Html))
    {
        Some(
            ObjectInitializer::new(context)
                .function(
                    NativeFunction::from_fn_ptr(parse_html),
                    JsString::from("parse"),
                    1,
                )
                .build(),
        )
    } else {
        None
    };
    let browser = if manifest
        .requires
        .iter()
        .any(|capability| matches!(capability, Capability::Browser))
    {
        let host = host.ok_or(EngineError::MissingHostCapability {
            capability: "browser",
        })?;
        Some(
            ObjectInitializer::with_native_data(
                HostState {
                    host: Rc::clone(host),
                },
                context,
            )
            .function(
                NativeFunction::from_fn_ptr(browser_run),
                JsString::from("run"),
                1,
            )
            .build(),
        )
    } else {
        None
    };
    let image = manifest
        .requires
        .iter()
        .any(|capability| matches!(capability, Capability::Image))
        .then(|| crate::image::context_value(crate::image::ImageLimits::default(), context));
    let settings = JsValue::from_json(&serde_json::Value::Object(settings.clone()), context)
        .map_err(|error| EngineError::InvalidPackage {
            message: format!("could not expose settings: {error}"),
        })?;
    let mut root = ObjectInitializer::new(context);
    root.property(JsString::from("url"), url, Attribute::all())
        .function(NativeFunction::from_fn_ptr(fail), JsString::from("fail"), 3)
        .property(JsString::from("settings"), settings, Attribute::all());
    if let Some(http) = http {
        root.property(JsString::from("http"), http, Attribute::all());
    }
    if let Some(html) = html {
        root.property(JsString::from("html"), html, Attribute::all());
    }
    if let Some(browser) = browser {
        root.property(JsString::from("browser"), browser, Attribute::all());
    }
    if let Some(image) = image {
        root.property(JsString::from("image"), image, Attribute::all());
    }

    Ok(root.build().into())
}

#[derive(Clone, Trace, Finalize, JsData)]
struct HostState {
    #[unsafe_ignore_trace]
    host: Rc<RefCell<dyn WefHost>>,
}

#[derive(Clone, Trace, Finalize, JsData)]
struct HtmlDocumentState {
    #[unsafe_ignore_trace]
    html: Rc<Html>,
}

#[derive(Clone, Trace, Finalize, JsData)]
struct HtmlElementState {
    #[unsafe_ignore_trace]
    html: Rc<Html>,
    #[unsafe_ignore_trace]
    node_id: NodeId,
}

fn string_argument(
    args: &[JsValue],
    index: usize,
    context: &mut Context,
) -> Result<String, JsError> {
    args.get(index)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
}

fn resolve_url(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let base = string_argument(args, 0, context)?;
    let value = string_argument(args, 1, context)?;
    let base = Url::parse(&base)
        .map_err(|error| JsNativeError::typ().with_message(format!("invalid base URL: {error}")))?;
    let resolved = base
        .join(&value)
        .map_err(|error| JsNativeError::typ().with_message(format!("invalid URL: {error}")))?;
    Ok(JsValue::from(JsString::from(resolved.to_string())))
}

fn fail(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let code = string_argument(args, 0, context)?;
    let message = string_argument(args, 1, context)?;
    let details = args.get(2).cloned().unwrap_or_else(JsValue::undefined);
    let error = ObjectInitializer::new(context)
        .property(JsString::from("__wefError"), true, Attribute::all())
        .property(
            JsString::from("code"),
            JsString::from(code),
            Attribute::all(),
        )
        .property(
            JsString::from("message"),
            JsString::from(message),
            Attribute::all(),
        )
        .property(JsString::from("details"), details, Attribute::all())
        .build();
    Err(JsError::from_opaque(error.into()))
}

fn http_request(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let object = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("http.request called without host"))?;
    let state = object
        .downcast_ref::<HostState>()
        .ok_or_else(|| JsNativeError::typ().with_message("http.request called without host"))?;
    let request_value = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .to_json(context)?
        .ok_or_else(|| JsNativeError::typ().with_message("http.request requires an object"))?;
    let request: HttpRequest = serde_json::from_value(request_value).map_err(|error| {
        JsNativeError::typ().with_message(format!("invalid HTTP request: {error}"))
    })?;

    let response = state.host.borrow_mut().request(request).map_err(|error| {
        let code = match error {
            crate::host::HostError::ChallengeRequired { .. } => "CHALLENGE_REQUIRED",
            crate::host::HostError::RateLimited => "RATE_LIMITED",
            _ => "HTTP_ERROR",
        };
        let error = ObjectInitializer::new(context)
            .property(JsString::from("__wefError"), true, Attribute::all())
            .property(
                JsString::from("code"),
                JsString::from(code),
                Attribute::all(),
            )
            .property(
                JsString::from("message"),
                JsString::from(error.to_string()),
                Attribute::all(),
            )
            .build();
        JsError::from_opaque(error.into())
    })?;
    let response_value = serde_json::to_value(response).map_err(JsError::from_rust)?;
    let response_value = JsValue::from_json(&response_value, context)?;
    Ok(JsPromise::resolve(response_value, context).into())
}

fn browser_run(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let object = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("browser.run called without host"))?;
    let state = object
        .downcast_ref::<HostState>()
        .ok_or_else(|| JsNativeError::typ().with_message("browser.run called without host"))?;
    let value = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .to_json(context)?
        .ok_or_else(|| JsNativeError::typ().with_message("browser.run requires an object"))?;
    let request = serde_json::from_value(value).map_err(|error| {
        JsNativeError::typ().with_message(format!("invalid browser request: {error}"))
    })?;
    let result = state
        .host
        .borrow_mut()
        .run_browser(request)
        .map_err(|error| source_host_error(error, context, "BROWSER_ERROR"))?;
    let value = serde_json::to_value(result).map_err(JsError::from_rust)?;
    Ok(JsPromise::resolve(JsValue::from_json(&value, context)?, context).into())
}

fn source_host_error(
    error: crate::host::HostError,
    context: &mut Context,
    default_code: &str,
) -> JsError {
    let code = match error {
        crate::host::HostError::ChallengeRequired { .. } => "CHALLENGE_REQUIRED",
        crate::host::HostError::RateLimited => "RATE_LIMITED",
        crate::host::HostError::Unsupported => "UNSUPPORTED",
        _ => default_code,
    };
    let error = ObjectInitializer::new(context)
        .property(JsString::from("__wefError"), true, Attribute::all())
        .property(
            JsString::from("code"),
            JsString::from(code),
            Attribute::all(),
        )
        .property(
            JsString::from("message"),
            JsString::from(error.to_string()),
            Attribute::all(),
        )
        .build();
    JsError::from_opaque(error.into())
}

fn parse_html(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let source = string_argument(args, 0, context)?;
    let html = Rc::new(Html::parse_document(&source));
    Ok(document_value(html, context)?.into())
}

fn document_value(
    html: Rc<Html>,
    context: &mut Context,
) -> boa_engine::JsResult<boa_engine::object::JsObject> {
    Ok(
        ObjectInitializer::with_native_data(HtmlDocumentState { html }, context)
            .function(
                NativeFunction::from_fn_ptr(document_select),
                JsString::from("select"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(document_select_all),
                JsString::from("selectAll"),
                1,
            )
            .build(),
    )
}

fn element_value(
    html: Rc<Html>,
    node_id: NodeId,
    context: &mut Context,
) -> boa_engine::JsResult<boa_engine::object::JsObject> {
    Ok(
        ObjectInitializer::with_native_data(HtmlElementState { html, node_id }, context)
            .function(
                NativeFunction::from_fn_ptr(element_select),
                JsString::from("select"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(element_select_all),
                JsString::from("selectAll"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(element_text),
                JsString::from("text"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(element_html),
                JsString::from("html"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(element_attr),
                JsString::from("attr"),
                1,
            )
            .build(),
    )
}

fn selector_argument(args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<Selector> {
    let value = string_argument(args, 0, context)?;
    Ok(Selector::parse(&value).map_err(|error| {
        JsNativeError::typ().with_message(format!("invalid CSS selector: {error}"))
    })?)
}

fn document_state(this: &JsValue) -> boa_engine::JsResult<HtmlDocumentState> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("HTML document method called without document")
    })?;
    let state = object.downcast_ref::<HtmlDocumentState>().ok_or_else(|| {
        JsNativeError::typ().with_message("HTML document method called without document")
    })?;
    Ok((*state).clone())
}

fn element_state(this: &JsValue) -> boa_engine::JsResult<HtmlElementState> {
    let object = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("HTML element method called without element")
    })?;
    let state = object.downcast_ref::<HtmlElementState>().ok_or_else(|| {
        JsNativeError::typ().with_message("HTML element method called without element")
    })?;
    Ok((*state).clone())
}

fn element_from_state(state: &HtmlElementState) -> boa_engine::JsResult<ElementRef<'_>> {
    state
        .html
        .tree
        .get(state.node_id)
        .and_then(ElementRef::wrap)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("HTML element no longer exists")
                .into()
        })
}

fn document_select(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let selector = selector_argument(args, context)?;
    let state = document_state(this)?;
    match state.html.select(&selector).next() {
        Some(element) => Ok(element_value(Rc::clone(&state.html), element.id(), context)?.into()),
        None => Ok(JsValue::null()),
    }
}

fn document_select_all(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let selector = selector_argument(args, context)?;
    let state = document_state(this)?;
    let nodes = state
        .html
        .select(&selector)
        .map(|element| element.id())
        .collect::<Vec<_>>();
    let array = JsArray::new(context);
    for node_id in nodes {
        array.push(
            element_value(Rc::clone(&state.html), node_id, context)?,
            context,
        )?;
    }
    Ok(array.into())
}

fn element_select(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let selector = selector_argument(args, context)?;
    let state = element_state(this)?;
    let result = element_from_state(&state)?
        .select(&selector)
        .next()
        .map(|element| element.id());
    match result {
        Some(node_id) => Ok(element_value(Rc::clone(&state.html), node_id, context)?.into()),
        None => Ok(JsValue::null()),
    }
}

fn element_select_all(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let selector = selector_argument(args, context)?;
    let state = element_state(this)?;
    let nodes = element_from_state(&state)?
        .select(&selector)
        .map(|element| element.id())
        .collect::<Vec<_>>();
    let html = Rc::clone(&state.html);
    let array = JsArray::new(context);
    for node_id in nodes {
        array.push(element_value(Rc::clone(&html), node_id, context)?, context)?;
    }
    Ok(array.into())
}

fn element_text(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let state = element_state(this)?;
    Ok(JsString::from(element_from_state(&state)?.text().collect::<String>()).into())
}

fn element_html(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let state = element_state(this)?;
    Ok(JsString::from(element_from_state(&state)?.html()).into())
}

fn element_attr(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name = string_argument(args, 0, context)?;
    let state = element_state(this)?;
    Ok(element_from_state(&state)?
        .attr(&name)
        .map(JsString::from)
        .map(JsValue::from)
        .unwrap_or_else(JsValue::undefined))
}
