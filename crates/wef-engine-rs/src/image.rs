use std::{cell::RefCell, io::Cursor, rc::Rc};

use boa_engine::{
    Context, Finalize, JsData, JsString, JsValue, Trace,
    error::JsNativeError,
    native_function::NativeFunction,
    object::{
        ObjectInitializer,
        builtins::{JsArrayBuffer, JsPromise},
    },
    property::Attribute,
};
use image::{DynamicImage, GenericImage, GenericImageView, ImageFormat};

#[derive(Debug, Clone, Copy)]
pub struct ImageLimits {
    pub max_input_bytes: usize,
    pub max_pixels: u64,
    pub max_output_bytes: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 20 * 1024 * 1024,
            max_pixels: 40_000_000,
            max_output_bytes: 20 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Trace, Finalize, JsData)]
struct ImageApiState {
    #[unsafe_ignore_trace]
    limits: ImageLimits,
}

#[derive(Clone, Trace, Finalize, JsData)]
struct BitmapState {
    #[unsafe_ignore_trace]
    image: Rc<RefCell<DynamicImage>>,
}

pub(crate) fn context_value(limits: ImageLimits, context: &mut Context) -> JsValue {
    ObjectInitializer::with_native_data(ImageApiState { limits }, context)
        .function(
            NativeFunction::from_fn_ptr(decode),
            JsString::from("decode"),
            1,
        )
        .function(
            NativeFunction::from_fn_ptr(create),
            JsString::from("create"),
            2,
        )
        .function(NativeFunction::from_fn_ptr(blit), JsString::from("blit"), 5)
        .function(
            NativeFunction::from_fn_ptr(encode),
            JsString::from("encode"),
            3,
        )
        .build()
        .into()
}

pub(crate) fn array_buffer_bytes(value: &JsValue) -> boa_engine::JsResult<Vec<u8>> {
    let object = value
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("expected ArrayBuffer"))?;
    let buffer = JsArrayBuffer::from_object(object)?;
    Ok(buffer
        .data()
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default())
}

pub(crate) fn array_buffer_value(
    bytes: Vec<u8>,
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let data = boa_engine::object::builtins::AlignedVec::from_iter(0, bytes);
    Ok(JsArrayBuffer::from_byte_block(data, context)?.into())
}

fn api_state(this: &JsValue) -> boa_engine::JsResult<ImageApiState> {
    let object = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("image method called without API"))?;
    object
        .downcast_ref::<ImageApiState>()
        .map(|state| (*state).clone())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("image method called without API")
                .into()
        })
}

fn bitmap(value: &JsValue) -> boa_engine::JsResult<BitmapState> {
    let object = value
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("expected ImageBitmap"))?;
    object
        .downcast_ref::<BitmapState>()
        .map(|state| (*state).clone())
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("expected ImageBitmap")
                .into()
        })
}

fn number(args: &[JsValue], index: usize, context: &mut Context) -> boa_engine::JsResult<u32> {
    let value = args
        .get(index)
        .unwrap_or(&JsValue::undefined())
        .to_number(context)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > u32::MAX as f64 {
        return Err(JsNativeError::range()
            .with_message("expected a non-negative integer")
            .into());
    }
    Ok(value as u32)
}

fn rect(value: &JsValue, context: &mut Context) -> boa_engine::JsResult<(u32, u32, u32, u32)> {
    let object = value
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("expected Rect"))?;
    let mut get = |name| object.get(JsString::from(name), context);
    let values = [get("x")?, get("y")?, get("width")?, get("height")?];
    let mut out = [0; 4];
    for (index, value) in values.iter().enumerate() {
        out[index] = number(std::slice::from_ref(value), 0, context)?;
    }
    Ok((out[0], out[1], out[2], out[3]))
}

fn bitmap_value(image: DynamicImage, context: &mut Context) -> JsValue {
    let (width, height) = image.dimensions();
    ObjectInitializer::with_native_data(
        BitmapState {
            image: Rc::new(RefCell::new(image)),
        },
        context,
    )
    .property(JsString::from("width"), width, Attribute::all())
    .property(JsString::from("height"), height, Attribute::all())
    .build()
    .into()
}

fn decode(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let limits = api_state(this)?.limits;
    let bytes = array_buffer_bytes(args.first().unwrap_or(&JsValue::undefined()))?;
    if bytes.len() > limits.max_input_bytes {
        return Err(JsNativeError::range()
            .with_message("image input exceeds host limit")
            .into());
    }
    let image = image::load_from_memory(&bytes)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    let (width, height) = image.dimensions();
    if u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(JsNativeError::range()
            .with_message("decoded image exceeds host pixel limit")
            .into());
    }
    Ok(JsPromise::resolve(bitmap_value(image, context), context).into())
}

fn create(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let limits = api_state(this)?.limits;
    let (width, height) = (number(args, 0, context)?, number(args, 1, context)?);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(JsNativeError::range()
            .with_message("image dimensions exceed host limit")
            .into());
    }
    Ok(bitmap_value(
        DynamicImage::new_rgba8(width, height),
        context,
    ))
}

fn blit(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let target = bitmap(args.first().unwrap_or(&JsValue::undefined()))?;
    let source = bitmap(args.get(1).unwrap_or(&JsValue::undefined()))?;
    let (sx, sy, sw, sh) = rect(args.get(2).unwrap_or(&JsValue::undefined()), context)?;
    let (tx, ty, tw, th) = rect(args.get(3).unwrap_or(&JsValue::undefined()), context)?;
    if (sw, sh) != (tw, th) {
        return Err(JsNativeError::range()
            .with_message("blit rectangles must have equal dimensions")
            .into());
    }
    let source = source.image.borrow();
    if sx
        .checked_add(sw)
        .is_none_or(|right| right > source.width())
        || sy
            .checked_add(sh)
            .is_none_or(|bottom| bottom > source.height())
    {
        return Err(JsNativeError::range()
            .with_message("source rectangle is outside image")
            .into());
    }
    let copy = source.crop_imm(sx, sy, sw, sh);
    let mut target = target.image.borrow_mut();
    if tx
        .checked_add(tw)
        .is_none_or(|right| right > target.width())
        || ty
            .checked_add(th)
            .is_none_or(|bottom| bottom > target.height())
    {
        return Err(JsNativeError::range()
            .with_message("target rectangle is outside image")
            .into());
    }
    target
        .copy_from(&copy, tx, ty)
        .map_err(|error| JsNativeError::range().with_message(error.to_string()))?;
    Ok(JsValue::undefined())
}

fn encode(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let limits = api_state(this)?.limits;
    let image = bitmap(args.first().unwrap_or(&JsValue::undefined()))?;
    let mime = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .to_string(context)?
        .to_std_string_escaped();
    let format = match mime.as_str() {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("unsupported image MIME type")
                .into());
        }
    };
    let mut bytes = Cursor::new(Vec::new());
    image
        .image
        .borrow()
        .write_to(&mut bytes, format)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    let bytes = bytes.into_inner();
    if bytes.len() > limits.max_output_bytes {
        return Err(JsNativeError::range()
            .with_message("encoded image exceeds host limit")
            .into());
    }
    Ok(JsPromise::resolve(array_buffer_value(bytes, context)?, context).into())
}
