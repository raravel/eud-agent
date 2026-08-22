use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits, RgbaImage};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::attachment::{ResolvedImageAttachment, MAX_IMAGE_BYTES};
use crate::map_model::{rows_from_cells, MapLayer, MapOperation, MapRevision, RowSpan, Tileset};
use crate::map_verify::MapRequestAuthority;

pub const MAP_IMAGE_QUANTIZER_VERSION: &str = "sd-bayer8-v1";
pub const MAX_DECODE_PIXELS: u64 = 16_777_216;
pub const MAX_SOURCE_EDGE: u32 = 16_384;
pub const MAX_OUTPUT_EDGE: u16 = 256;
pub const MAX_OUTPUT_CELLS: usize = 65_536;
const NORMALIZED_SOURCE_EDGE: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImagePlacement {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl MapImagePlacement {
    pub fn validate(self, map_width: u16, map_height: u16) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("image placement width and height must be at least one tile".to_string());
        }
        if self.width > MAX_OUTPUT_EDGE || self.height > MAX_OUTPUT_EDGE {
            return Err(format!(
                "image placement output exceeds {MAX_OUTPUT_EDGE}x{MAX_OUTPUT_EDGE} tiles"
            ));
        }
        let cells = usize::from(self.width)
            .checked_mul(usize::from(self.height))
            .filter(|cells| *cells <= MAX_OUTPUT_CELLS)
            .ok_or_else(|| "image placement output cell count overflow".to_string())?;
        if cells == 0 {
            return Err("image placement output is empty".to_string());
        }
        let right = self
            .x
            .checked_add(self.width)
            .ok_or_else(|| "image placement x extent overflow".to_string())?;

        let bottom = self
            .y
            .checked_add(self.height)
            .ok_or_else(|| "image placement y extent overflow".to_string())?;
        if right > map_width || bottom > map_height {
            return Err("image placement is outside the current map dimensions".to_string());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImagePlaceInput {
    pub image_ref: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl MapImagePlaceInput {
    pub fn placement(&self) -> MapImagePlacement {
        MapImagePlacement {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImageDescriptor {
    pub attachment_id: String,
    pub name: String,
    pub mime: String,
    pub attachment_sha256: String,
    pub source_dimensions: MapImageDimensions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImageRequestRef {
    pub image_ref: String,
    pub name: String,
    pub mime: String,
    pub source_dimensions: MapImageDimensions,
}

#[derive(Debug, Clone)]
pub struct MapImageBinding {
    pub image_ref: String,
    pub session_id: String,
    pub request_id: String,
    pub attachment: ResolvedImageAttachment,
    pub source_dimensions: MapImageDimensions,
    pub candidate_revision_key: String,
    pub baseline_hash: String,
}

impl MapImageBinding {
    pub fn request_ref(&self) -> MapImageRequestRef {
        MapImageRequestRef {
            image_ref: self.image_ref.clone(),
            name: self.attachment.descriptor.name.clone(),
            mime: self.attachment.descriptor.mime.clone(),
            source_dimensions: self.source_dimensions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImageConversionMetadata {
    pub kind: String,
    pub attachment_sha256: String,
    pub source_dimensions: MapImageDimensions,
    pub placement: MapImagePlacement,
    pub quantizer_version: String,

    pub tile_grid_sha256: String,
    pub changed_cells: Vec<RowSpan>,
    pub walkability_changed_cells: u32,
    pub height_changed_cells: u32,
}
impl MapImageConversionMetadata {
    pub fn validate_operation(&self, operation: &MapOperation) -> Result<(), String> {
        if self.kind != "image_conversion" || self.quantizer_version != MAP_IMAGE_QUANTIZER_VERSION
        {
            return Err(
                "image conversion metadata kind or quantizer version is invalid".to_string(),
            );
        }
        let MapOperation::TerrainBlit { x, y, tiles } = operation else {
            return Err("image conversion metadata requires one TerrainBlit operation".to_string());
        };
        if *x != self.placement.x
            || *y != self.placement.y
            || tiles.len() != usize::from(self.placement.height)
            || tiles
                .iter()
                .any(|row| row.len() != usize::from(self.placement.width))
        {
            return Err(
                "image conversion metadata placement does not match TerrainBlit".to_string(),
            );
        }
        let flat = tiles.iter().flatten().copied().collect::<Vec<_>>();
        if tile_grid_sha256(self.placement.width, self.placement.height, &flat)
            != self.tile_grid_sha256
        {
            return Err(
                "image conversion metadata tile-grid digest does not match TerrainBlit".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImageConversionReport {
    pub source_dimensions: MapImageDimensions,
    pub placement: MapImagePlacement,
    pub changed_cells: u32,
    pub changed_rows: Vec<RowSpan>,
    pub unique_tile_count: u32,
    pub walkability_changed_cells: u32,
    pub height_changed_cells: u32,
    pub protected_conflicts: u32,
    pub outside_authority_conflicts: u32,
    pub tile_grid_sha256: String,
    pub quantizer_version: String,
}

#[derive(Debug, Clone)]
pub struct MapImageConversion {
    pub operation: MapOperation,
    pub metadata: MapImageConversionMetadata,
    pub report: MapImageConversionReport,
    pub preview_png: Vec<u8>,
}

pub struct MapImageMapContext<'a> {
    pub map_path: &'a Path,
    pub revision: &'a MapRevision,
    pub authority: &'a MapRequestAuthority,
    pub starcraft_path: &'a Path,
}

#[derive(Debug, Clone)]
struct CachedSource {
    key: String,
    source_dimensions: MapImageDimensions,
    normalized: Arc<RgbaImage>,
}

struct MapImageServiceInner {
    cache: Mutex<HashMap<String, CachedSource>>,
}

#[derive(Clone)]
pub struct MapImageService {
    inner: Arc<MapImageServiceInner>,
}

impl Default for MapImageService {
    fn default() -> Self {
        Self::new()
    }
}

impl MapImageService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MapImageServiceInner {
                cache: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn clear_session(&self, session_id: &str) {
        self.inner.cache.lock().remove(session_id);
    }

    pub fn describe(
        &self,
        session_id: &str,
        attachment: &ResolvedImageAttachment,
    ) -> Result<MapImageDescriptor, String> {
        let source = self.load_source(session_id, attachment)?;
        Ok(MapImageDescriptor {
            attachment_id: attachment.descriptor.id.clone(),
            name: attachment.descriptor.name.clone(),
            mime: attachment.descriptor.mime.clone(),
            attachment_sha256: attachment.sha256.clone(),
            source_dimensions: source.source_dimensions,
        })
    }

    pub fn bind_request_images(
        &self,
        session_id: &str,
        request_id: &str,
        attachments: &[ResolvedImageAttachment],
        candidate_revision_key: &str,
        baseline_hash: &str,
    ) -> Result<Vec<MapImageBinding>, String> {
        let mut bindings = Vec::with_capacity(attachments.len());
        for (index, attachment) in attachments.iter().enumerate() {
            let source_dimensions = self.load_source(session_id, attachment)?.source_dimensions;
            bindings.push(MapImageBinding {
                image_ref: format!("image-{}", index + 1),
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                attachment: attachment.clone(),
                source_dimensions,
                candidate_revision_key: candidate_revision_key.to_string(),
                baseline_hash: baseline_hash.to_string(),
            });
        }
        Ok(bindings)
    }

    pub fn convert(
        &self,
        session_id: &str,
        attachment: &ResolvedImageAttachment,
        placement: MapImagePlacement,
        context: MapImageMapContext<'_>,
    ) -> Result<MapImageConversion, String> {
        placement.validate(context.revision.width, context.revision.height)?;
        let source = self.load_source(session_id, attachment)?;
        let fitted = fit_dimensions(source.source_dimensions, placement.width, placement.height)?;
        if fitted != (placement.width, placement.height) {
            return Err(format!(
                "image placement {}x{} does not preserve the source aspect ratio; use {}x{}",
                placement.width, placement.height, fitted.0, fitted.1
            ));
        }

        let chk = isom::chk_extract(context.map_path).map_err(|error| {
            format!("candidate terrain could not be read for image placement: {error}")
        })?;
        let digest = crate::chk::digest_chk(&chk);
        if digest.map.width != context.revision.width
            || digest.map.height != context.revision.height
            || digest.map.tileset != tileset_name(context.revision.tileset)
        {
            return Err(
                "candidate map dimensions or tileset changed before image placement".to_string(),
            );
        }
        let before = placement_tiles(
            &digest.tiles,
            context.revision.width,
            context.revision.height,
            placement,
        )?;
        let resized = image::imageops::resize(
            source.normalized.as_ref(),
            u32::from(placement.width),
            u32::from(placement.height),
            FilterType::Lanczos3,
        );
        let quantized = isom::image_quantize(
            context.starcraft_path,
            context.revision.tileset.era(),
            resized.as_raw(),
            placement.width,
            placement.height,
            &before,
        )
        .map_err(|error| format!("native image-to-terrain quantization failed: {error}"))?;

        let mut changed = BTreeSet::new();
        let mut protected_conflicts = 0_u32;
        let mut outside_authority_conflicts = 0_u32;
        for (index, (&before_tile, &after_tile)) in
            before.iter().zip(quantized.tiles.iter()).enumerate()
        {
            if before_tile == after_tile {
                continue;
            }
            let x = placement.x + (index % usize::from(placement.width)) as u16;
            let y = placement.y + (index / usize::from(placement.width)) as u16;
            changed.insert((x, y));
            if context.authority.forbids(MapLayer::Terrain, x, y) {
                protected_conflicts += 1;
            }
            if !context.authority.allows(MapLayer::Terrain, x, y) {
                outside_authority_conflicts += 1;
            }
        }
        let changed_rows = rows_from_cells(&changed);
        let tile_grid_sha256 =
            tile_grid_sha256(quantized.width, quantized.height, &quantized.tiles);
        let tiles = quantized
            .tiles
            .chunks(usize::from(placement.width))
            .map(<[u16]>::to_vec)
            .collect();
        let preview_png = encode_rgb_png(
            u32::from(quantized.width),
            u32::from(quantized.height),
            &quantized.preview_rgb,
        )?;
        let metadata = MapImageConversionMetadata {
            kind: "image_conversion".to_string(),
            attachment_sha256: attachment.sha256.clone(),
            source_dimensions: source.source_dimensions,
            placement,
            quantizer_version: MAP_IMAGE_QUANTIZER_VERSION.to_string(),
            tile_grid_sha256: tile_grid_sha256.clone(),
            changed_cells: changed_rows.clone(),
            walkability_changed_cells: quantized.walkability_changed_cells,
            height_changed_cells: quantized.height_changed_cells,
        };
        let report = MapImageConversionReport {
            source_dimensions: source.source_dimensions,
            placement,
            changed_cells: u32::try_from(changed.len())
                .map_err(|_| "image changed-cell count overflow".to_string())?,
            changed_rows,
            unique_tile_count: quantized.unique_tile_count,
            walkability_changed_cells: quantized.walkability_changed_cells,
            height_changed_cells: quantized.height_changed_cells,
            protected_conflicts,
            outside_authority_conflicts,
            tile_grid_sha256,
            quantizer_version: MAP_IMAGE_QUANTIZER_VERSION.to_string(),
        };
        Ok(MapImageConversion {
            operation: MapOperation::TerrainBlit {
                x: placement.x,
                y: placement.y,
                tiles,
            },
            metadata,
            report,
            preview_png,
        })
    }

    fn load_source(
        &self,
        session_id: &str,
        attachment: &ResolvedImageAttachment,
    ) -> Result<CachedSource, String> {
        let key = format!("{}:{}", attachment.descriptor.id, attachment.sha256);
        if let Some(source) = self
            .inner
            .cache
            .lock()
            .get(session_id)
            .filter(|source| source.key == key)
            .cloned()
        {
            return Ok(source);
        }
        let source = decode_source(attachment, key)?;
        self.inner
            .cache
            .lock()
            .insert(session_id.to_string(), source.clone());
        Ok(source)
    }
}

pub fn fit_dimensions(
    source: MapImageDimensions,
    max_width: u16,
    max_height: u16,
) -> Result<(u16, u16), String> {
    if source.width == 0 || source.height == 0 || max_width == 0 || max_height == 0 {
        return Err("image aspect resolver requires non-empty dimensions".to_string());
    }
    let source_width = u64::from(source.width);
    let source_height = u64::from(source.height);
    let max_width_u64 = u64::from(max_width);
    let max_height_u64 = u64::from(max_height);
    let (width, height) = if max_width_u64
        .checked_mul(source_height)
        .ok_or_else(|| "image aspect comparison overflow".to_string())?
        <= max_height_u64
            .checked_mul(source_width)
            .ok_or_else(|| "image aspect comparison overflow".to_string())?
    {
        let height = source_height
            .checked_mul(max_width_u64)
            .and_then(|value| value.checked_add(source_width / 2))
            .map(|value| value / source_width)
            .ok_or_else(|| "image aspect height overflow".to_string())?;
        (max_width_u64, height.max(1).min(max_height_u64))
    } else {
        let width = source_width
            .checked_mul(max_height_u64)
            .and_then(|value| value.checked_add(source_height / 2))
            .map(|value| value / source_height)
            .ok_or_else(|| "image aspect width overflow".to_string())?;
        (width.max(1).min(max_width_u64), max_height_u64)
    };
    Ok((
        u16::try_from(width).map_err(|_| "image aspect width exceeds u16".to_string())?,
        u16::try_from(height).map_err(|_| "image aspect height exceeds u16".to_string())?,
    ))
}

fn decode_source(
    attachment: &ResolvedImageAttachment,
    key: String,
) -> Result<CachedSource, String> {
    let encoded_size = std::fs::metadata(&attachment.path)
        .map_err(|error| format!("attached image metadata could not be read: {error}"))?
        .len();
    if encoded_size == 0 || encoded_size > MAX_IMAGE_BYTES as u64 {
        return Err("attached image is empty or exceeds the 10 MiB limit".to_string());
    }
    let mut header_reader = ImageReader::open(&attachment.path)
        .map_err(|error| format!("attached image could not be opened: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("attached image format could not be detected: {error}"))?;
    let format = header_reader
        .format()
        .filter(is_supported_format)
        .ok_or_else(|| "attached image format must be PNG, JPEG, WebP, or GIF".to_string())?;
    header_reader.limits(decode_limits());
    let (width, height) = header_reader
        .into_dimensions()
        .map_err(|error| format!("attached image dimensions could not be decoded: {error}"))?;
    validate_source_dimensions(width, height)?;

    let mut reader = ImageReader::open(&attachment.path)
        .map_err(|error| format!("attached image could not be opened: {error}"))?;
    reader.set_format(format);
    reader.limits(decode_limits());
    let decoded = reader.decode().map_err(|error| {
        format!("attached image is corrupt, truncated, or unsupported: {error}")
    })?;
    if decoded.width() != width || decoded.height() != height {
        return Err("attached image dimensions changed while decoding".to_string());
    }
    let rgba = decoded.to_rgba8();
    let normalized_dimensions =
        if width <= NORMALIZED_SOURCE_EDGE && height <= NORMALIZED_SOURCE_EDGE {
            (width as u16, height as u16)
        } else {
            fit_dimensions(
                MapImageDimensions { width, height },
                NORMALIZED_SOURCE_EDGE as u16,
                NORMALIZED_SOURCE_EDGE as u16,
            )?
        };
    let normalized = if rgba.width() == u32::from(normalized_dimensions.0)
        && rgba.height() == u32::from(normalized_dimensions.1)
    {
        rgba
    } else {
        image::imageops::resize(
            &rgba,
            u32::from(normalized_dimensions.0),
            u32::from(normalized_dimensions.1),
            FilterType::Lanczos3,
        )
    };
    Ok(CachedSource {
        key,
        source_dimensions: MapImageDimensions { width, height },
        normalized: Arc::new(normalized),
    })
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_PIXELS * 4);
    limits
}

fn is_supported_format(format: &ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    )
}

fn validate_source_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return Err(format!(
            "attached image dimensions must be within 1..={MAX_SOURCE_EDGE}"
        ));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|pixels| *pixels <= MAX_DECODE_PIXELS)
        .ok_or_else(|| format!("attached image exceeds the {MAX_DECODE_PIXELS} pixel limit"))?;
    pixels
        .checked_mul(4)
        .ok_or_else(|| "attached image RGBA allocation size overflow".to_string())?;
    Ok(())
}

fn placement_tiles(
    tiles: &[u16],
    map_width: u16,
    map_height: u16,
    placement: MapImagePlacement,
) -> Result<Vec<u16>, String> {
    let map_cells = usize::from(map_width)
        .checked_mul(usize::from(map_height))
        .ok_or_else(|| "candidate terrain dimensions overflow".to_string())?;
    if tiles.len() != map_cells {
        return Err("candidate terrain tile count does not match DIM".to_string());
    }
    let cell_count = usize::from(placement.width)
        .checked_mul(usize::from(placement.height))
        .ok_or_else(|| "image placement tile count overflow".to_string())?;
    let mut result = Vec::with_capacity(cell_count);
    for row in 0..placement.height {
        let start =
            usize::from(placement.y + row) * usize::from(map_width) + usize::from(placement.x);
        let end = start + usize::from(placement.width);
        result.extend_from_slice(&tiles[start..end]);
    }
    Ok(result)
}

fn tile_grid_sha256(width: u16, height: u16, tiles: &[u16]) -> String {
    let mut digest = Sha256::new();
    digest.update(width.to_le_bytes());
    digest.update(height.to_le_bytes());
    for tile in tiles {
        digest.update(tile.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn encode_rgb_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| "image preview dimensions overflow".to_string())?;
    if rgb.len() != expected {
        return Err("native image preview RGB length does not match dimensions".to_string());
    }
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("image preview PNG header failed: {error}"))?;
        writer
            .write_image_data(rgb)
            .map_err(|error| format!("image preview PNG encode failed: {error}"))?;
    }
    Ok(output)
}

fn tileset_name(tileset: Tileset) -> String {
    match tileset {
        Tileset::Badlands => "badlands",
        Tileset::Platform => "platform",
        Tileset::Installation => "installation",
        Tileset::Ashworld => "ashworld",
        Tileset::Jungle => "jungle",
        Tileset::Desert => "desert",
        Tileset::Arctic => "arctic",
        Tileset::Twilight => "twilight",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_image(path: std::path::PathBuf, mime: &str) -> ResolvedImageAttachment {
        ResolvedImageAttachment {
            descriptor: crate::attachment::AttachmentDescriptor {
                id: uuid::Uuid::new_v4().to_string(),
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap()
                    .to_string(),
                mime: mime.to_string(),
                kind: crate::attachment::AttachmentKind::Image,
                size: std::fs::metadata(&path).unwrap().len(),
            },
            path,
            sha256: "fixture-sha256".to_string(),
        }
    }

    #[test]
    fn png_jpeg_webp_and_gif_decode_to_bounded_first_images() {
        let root = std::env::temp_dir().join(format!("map-image-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 70) as u8, (y * 90) as u8, 120, 255])
        }));
        for (extension, mime, format) in [
            ("png", "image/png", ImageFormat::Png),
            ("jpg", "image/jpeg", ImageFormat::Jpeg),
            ("webp", "image/webp", ImageFormat::WebP),
            ("gif", "image/gif", ImageFormat::Gif),
        ] {
            let path = root.join(format!("source.{extension}"));
            let mut encoded = std::io::Cursor::new(Vec::new());
            source.write_to(&mut encoded, format).unwrap();
            std::fs::write(&path, encoded.into_inner()).unwrap();
            let decoded =
                decode_source(&resolved_image(path, mime), format!("fixture-{extension}")).unwrap();
            assert_eq!(
                decoded.source_dimensions,
                MapImageDimensions {
                    width: 3,
                    height: 2,
                },
                "{extension}"
            );
            assert_eq!(decoded.normalized.dimensions(), (3, 2), "{extension}");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_and_truncated_supported_images_fail_explicitly() {
        let root = std::env::temp_dir().join(format!("map-image-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("truncated.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nbroken").unwrap();
        let error =
            decode_source(&resolved_image(path, "image/png"), "bad".to_string()).unwrap_err();
        assert!(
            error.contains("dimensions could not be decoded")
                || error.contains("corrupt, truncated, or unsupported")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn aspect_resolver_handles_small_portrait_landscape_and_square_inputs() {
        assert_eq!(
            fit_dimensions(
                MapImageDimensions {
                    width: 1,
                    height: 1,
                },
                256,
                256,
            )
            .unwrap(),
            (256, 256)
        );
        assert_eq!(
            fit_dimensions(
                MapImageDimensions {
                    width: 400,
                    height: 200,
                },
                64,
                64,
            )
            .unwrap(),
            (64, 32)
        );
        assert_eq!(
            fit_dimensions(
                MapImageDimensions {
                    width: 200,
                    height: 400,
                },
                64,
                64,
            )
            .unwrap(),
            (32, 64)
        );
        assert_eq!(
            fit_dimensions(
                MapImageDimensions {
                    width: 1920,
                    height: 1080,
                },
                1,
                1,
            )
            .unwrap(),
            (1, 1)
        );
    }

    #[test]
    fn source_and_output_limits_reject_zero_caps_and_overflow() {
        assert!(validate_source_dimensions(0, 1).is_err());
        assert!(validate_source_dimensions(MAX_SOURCE_EDGE + 1, 1).is_err());
        assert!(validate_source_dimensions(4096, 4097).is_err());
        assert!(MapImagePlacement {
            x: u16::MAX,
            y: 0,
            width: 2,
            height: 1,
        }
        .validate(256, 256)
        .is_err());
        assert!(MapImagePlacement {
            x: 0,
            y: 0,
            width: 257,
            height: 1,
        }
        .validate(512, 512)
        .is_err());
    }

    #[test]
    fn tile_grid_digest_is_stable_and_dimension_bound() {
        let tiles = [1, 2, 3, 4];
        let first = tile_grid_sha256(2, 2, &tiles);
        assert_eq!(first, tile_grid_sha256(2, 2, &tiles));
        assert_ne!(first, tile_grid_sha256(1, 4, &tiles));
        assert_ne!(first, tile_grid_sha256(2, 2, &[1, 2, 4, 3]));
    }

    #[test]
    #[ignore = "requires installed StarCraft terrain assets"]
    fn target_protect_and_transparent_cells_use_actual_terrain_changes_only() {
        let root =
            std::env::temp_dir().join(format!("map-image-authority-{}", uuid::Uuid::new_v4()));
        let dirs = crate::config::DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let map = root.join("source.scx");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &map).unwrap();
        let revision = crate::map_context::MapContextService::new(dirs)
            .revision_for_path("project".to_string(), &map)
            .unwrap();
        let image_path = root.join("alpha.png");
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, 4, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[
                    255, 255, 255, 0, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255,
                ])
                .unwrap();
        }
        std::fs::write(&image_path, encoded).unwrap();
        let attachment = resolved_image(image_path, "image/png");
        let selection =
            |id: &str, role: crate::map_model::SelectionRole, y: u16, left: u16, right: u16| {
                crate::map_model::SelectionMask::canonical(
                    id,
                    id,
                    "r0",
                    role,
                    [MapLayer::Terrain].into_iter().collect(),
                    crate::map_model::MaskGrid {
                        width: revision.width,
                        height: revision.height,
                        rows: vec![RowSpan {
                            y,
                            spans: vec![(left, right)],
                        }],
                    },
                )
                .unwrap()
            };
        let target = selection("target", crate::map_model::SelectionRole::Target, 0, 0, 4);
        let authority = MapRequestAuthority::calculate(
            "map-session".to_string(),
            "request".to_string(),
            0,
            revision.width,
            revision.height,
            vec![target.clone()],
            Vec::new(),
        )
        .unwrap();
        let service = MapImageService::new();
        let starcraft = std::path::Path::new(r"C:\Program Files (x86)\StarCraft");
        let inside = service
            .convert(
                "map-session",
                &attachment,
                MapImagePlacement {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 1,
                },
                MapImageMapContext {
                    map_path: &map,
                    revision: &revision,
                    authority: &authority,
                    starcraft_path: starcraft,
                },
            )
            .unwrap();
        assert!(inside.report.changed_cells > 0);
        assert_eq!(inside.report.outside_authority_conflicts, 0);

        let outside = service
            .convert(
                "map-session",
                &attachment,
                MapImagePlacement {
                    x: 0,
                    y: 1,
                    width: 4,
                    height: 1,
                },
                MapImageMapContext {
                    map_path: &map,
                    revision: &revision,
                    authority: &authority,
                    starcraft_path: starcraft,
                },
            )
            .unwrap();
        assert!(outside.report.outside_authority_conflicts > 0);

        let transparent_protect = selection(
            "transparent-protect",
            crate::map_model::SelectionRole::Protect,
            0,
            0,
            4,
        );
        let transparent_authority = MapRequestAuthority::calculate(
            "map-session".to_string(),
            "request".to_string(),
            0,
            revision.width,
            revision.height,
            vec![target.clone()],
            vec![transparent_protect],
        )
        .unwrap();
        let transparent_path = root.join("transparent.png");
        let mut transparent_encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut transparent_encoded, 4, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[0; 16])
                .unwrap();
        }
        std::fs::write(&transparent_path, transparent_encoded).unwrap();
        let transparent_attachment = resolved_image(transparent_path, "image/png");
        let transparent = service
            .convert(
                "map-session",
                &transparent_attachment,
                MapImagePlacement {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 1,
                },
                MapImageMapContext {
                    map_path: &map,
                    revision: &revision,
                    authority: &transparent_authority,
                    starcraft_path: starcraft,
                },
            )
            .unwrap();
        assert_eq!(transparent.report.protected_conflicts, 0);
        assert_eq!(transparent.report.changed_cells, 0);

        let actual_protect = selection(
            "actual-protect",
            crate::map_model::SelectionRole::Protect,
            0,
            0,
            4,
        );
        let protected_authority = MapRequestAuthority::calculate(
            "map-session".to_string(),
            "request".to_string(),
            0,
            revision.width,
            revision.height,
            vec![target],
            vec![actual_protect],
        )
        .unwrap();
        let protected = service
            .convert(
                "map-session",
                &attachment,
                MapImagePlacement {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 1,
                },
                MapImageMapContext {
                    map_path: &map,
                    revision: &revision,
                    authority: &protected_authority,
                    starcraft_path: starcraft,
                },
            )
            .unwrap();
        assert_eq!(
            protected.report.protected_conflicts,
            protected.report.changed_cells
        );
        assert!(protected.report.protected_conflicts > 0);
        std::fs::remove_dir_all(root).ok();
    }
}
