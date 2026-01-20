use crate::api::error::{ApiError, ApiResult};
use crate::config::Config;
use crate::content::{flash, FileContents};
use crate::model::enums::PostType;
use image::{
    DynamicImage, ImageBuffer, ImageFormat, ImageReader, ImageResult, Limits, Rgb, RgbImage,
};
use jxl::decode::Decoder as JxlDecoder;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;
use swf::Tag;
use tracing::error;
use video_rs::Decoder as VideoDecoder;
use video_rs::ffmpeg::format::Pixel;
use video_rs::ffmpeg::media::Type;

/// Returns a representative image for the given content.
pub fn representative_image(
    config: &Config,
    file_contents: &FileContents,
    file_path: &Path,
) -> ApiResult<DynamicImage> {
    match PostType::from(file_contents.mime_type) {
        PostType::Image | PostType::Animation => {
            // If image-rs does not recognize the format, try JPEG XL
            if file_contents.mime_type.to_image_format().is_none() {
                image_jxl(&file_contents.data)
            } else {
                let image_format = file_contents
                    .mime_type
                    .to_image_format()
                    .expect("Format checked above");

                image(&file_contents.data, image_format).map_err(ApiError::from)
            }
        }
        PostType::Video => {
            video_frame(file_path).and_then(|frame| frame.ok_or(ApiError::EmptyVideo))
        }
        PostType::Flash => {
            flash_image(config, file_path).and_then(|frame| frame.ok_or(ApiError::EmptySwf))
        }
    }
}

/// Returns if the video at `path` has an audio channel.
pub fn video_has_audio(path: &Path) -> Result<bool, video_rs::Error> {
    video_rs::ffmpeg::format::input(path)
        .map(|context| context.streams().best(Type::Audio).is_some())
        .map_err(video_rs::Error::from)
}

/// Returns if the swf at `path` has audio.
pub fn swf_has_audio(path: &Path) -> ApiResult<bool> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let swf_buf = swf::decompress_swf(reader)?;
    let swf = swf::parse_swf(&swf_buf)?;

    Ok(swf.tags.iter().any(|tag| {
        matches!(
            tag,
            Tag::DefineButtonSound(_)
                | Tag::DefineSound(_)
                | Tag::SoundStreamBlock(_)
                | Tag::SoundStreamHead(_)
                | Tag::SoundStreamHead2(_)
                | Tag::StartSound(_)
                | Tag::StartSound2 { .. }
        )
    }))
}

/// Decodes a raw array of bytes into pixel data (image-rs formats only).
pub fn image(bytes: &[u8], format: ImageFormat) -> ImageResult<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(format);
    reader.limits(image_reader_limits());
    reader.decode()
}

/// Decode a JPEG XL image using `jxl = 0.2.2`.
fn image_jxl(bytes: &[u8]) -> ApiResult<DynamicImage> {
    let mut decoder = JxlDecoder::new(bytes);
    let image = decoder.decode().map_err(ApiError::from)?;

    let width = image.width() as u32;
    let height = image.height() as u32;
    let rgba = image.into_rgba8();

    let buffer: ImageBuffer<image::Rgba<u8>, _> =
        ImageBuffer::from_raw(width, height, rgba)
            .expect("JXL decoder returned invalid buffer");

    Ok(DynamicImage::ImageRgba8(buffer))
}

/// Decodes first frame of video contents.
fn video_frame(path: &Path) -> ApiResult<Option<DynamicImage>> {
    let mut decoder = VideoDecoder::new(path)?;
    let frame = decoder.decode_raw()?;

    if frame.planes() == 0 {
        return Ok(None);
    }

    let frame_data = frame.data(0);
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride(0);

    match frame.format() {
        Pixel::RGB24 => Ok(Some(rgb24_frame(frame_data, width, height, stride))),
        format => Err(ApiError::UnimplementedFrameFormat(format)),
    }
}

/// Search swf tags for the largest decodable image.
fn flash_image(config: &Config, path: &Path) -> ApiResult<Option<DynamicImage>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let swf_buf = swf::decompress_swf(reader)?;
    let swf = swf::parse_swf(&swf_buf)?;

    let encoding_table = swf.tags.iter().find_map(|tag| {
        if let Tag::JpegTables(table) = tag {
            Some(table)
        } else {
            None
        }
    });

    let mut images: Vec<_> = swf
        .tags
        .iter()
        .filter_map(|tag| match tag {
            Tag::DefineBits { jpeg_data, .. } => {
                let jpeg_data = flash::glue_tables_to_jpeg(jpeg_data, encoding_table.copied());
                Some(
                    image::load_from_memory_with_format(&jpeg_data, ImageFormat::Jpeg)
                        .map_err(flash::Error::from),
                )
            }
            Tag::DefineBitsLossless(bits) => flash::decode_define_bits_lossless(bits).transpose(),
            Tag::DefineBitsJpeg2 { jpeg_data, .. } => {
                Some(flash::decode_define_bits_jpeg(jpeg_data, None))
            }
            Tag::DefineBitsJpeg3(bits) => {
                Some(flash::decode_define_bits_jpeg(bits.data, Some(bits.alpha_data)))
            }
            _ => None,
        })
        .filter_map(|image_result| match image_result {
            Ok(image) => Some(image),
            Err(err) => {
                error!("Failure to decode flash image: {err}");
                None
            }
        })
        .collect();

    if let Ok(Some(frame)) = video_frame(path) {
        images.push(frame);
    }

    images.sort_by_key(|image| {
        let (thumbnail_width, thumbnail_height) = config.thumbnails.post_dimensions();
        let effective_width =
            if image.width() * thumbnail_height > thumbnail_width * image.height() {
                image.height() * thumbnail_width / thumbnail_height
            } else {
                image.width()
            };
        u32::MAX - effective_width
    });

    Ok(images.into_iter().next())
}

/// Returns maximum decoded image size.
fn image_reader_limits() -> Limits {
    const GB: u64 = 1024_u64.pow(3);
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(4 * GB);
    limits
}

/// Converts raw `video_frame` into a [`DynamicImage`].
fn rgb24_frame(video_frame: &[u8], width: u32, height: u32, stride: usize) -> DynamicImage {
    let rgb_image = RgbImage::from_fn(width, height, |x, y| {
        let offset = y as usize * stride + x as usize * 3;
        Rgb([
            video_frame[offset],
            video_frame[offset + 1],
            video_frame[offset + 2],
        ])
    });
    DynamicImage::ImageRgb8(rgb_image)
}
