//! `TextureDataMapping.h` and `TextureDataProvider.h/.cpp`: painted feature data on meshes.

use crate::geometry::point2ll::Point2LL;
use crate::geometry::points_f::Point2F;
use crate::mesh::Image;
use crate::slicer::SlicedUVCoordinates;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Describes a bit field in a pixel of a texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureBitField {
    /// The index of the first bit of the field
    pub bit_range_start_index: usize,
    /// The index of the last bit of the field
    pub bit_range_end_index: usize,
}

/// Gives the bit fields description of every feature stored in the texture.
pub type TextureDataMapping = BTreeMap<String, TextureBitField>;

/// `enum class TextureArea` (stored as its raw value, like a C++ enum class can be).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureArea(pub u32);

impl TextureArea {
    /// Area is to be treated as usual
    pub const NORMAL: TextureArea = TextureArea(0);
    /// Area is to be preferred
    pub const PREFERRED: TextureArea = TextureArea(1);
    /// Area is to be avoided
    pub const AVOID: TextureArea = TextureArea(2);
}

pub struct TextureDataProvider {
    uv_coordinates: Arc<SlicedUVCoordinates>,
    texture: Arc<Image>,
    texture_data_mapping: Arc<TextureDataMapping>,
}

impl TextureDataProvider {
    pub fn new(uv_coordinates: Arc<SlicedUVCoordinates>, texture: Arc<Image>, texture_data_mapping: Arc<TextureDataMapping>) -> Self {
        TextureDataProvider { uv_coordinates, texture, texture_data_mapping }
    }

    pub fn get_texture(&self) -> &Arc<Image> {
        &self.texture
    }

    pub fn get_value_pixel(&self, pixel_x: usize, pixel_y: usize, feature: &str) -> Option<u32> {
        texture_value(&self.texture, &self.texture_data_mapping, pixel_x, pixel_y, feature)
    }

    pub fn get_value_uv(&self, uv_coordinates: &Point2F, feature: &str) -> Option<u32> {
        let (x, y) = self.texture.get_pixel_coordinates(uv_coordinates);
        self.get_value_pixel(x, y, feature)
    }

    pub fn get_value(&self, position: &Point2LL, feature: &str) -> Option<u32> {
        let point_uv_coordinates = self.uv_coordinates.get_closest_uv_coordinates(position)?;
        self.get_value_uv(&point_uv_coordinates, feature)
    }

    pub fn get_area_preference(&self, position: &Point2LL, feature: &str) -> Option<TextureArea> {
        self.get_value(position, feature).map(TextureArea)
    }
}

/// `TextureDataProvider::getValue(pixel_x, pixel_y, feature)` without the UV lookup part.
pub fn texture_value(texture: &Image, texture_data_mapping: &TextureDataMapping, pixel_x: usize, pixel_y: usize, feature: &str) -> Option<u32> {
    let bit_field = texture_data_mapping.get(feature)?;
    let pixel_data = texture.get_pixel(pixel_x, pixel_y);

    // Extract relevant bits by rotating the pixel data left then right, which will insert 0s where appropriate
    let shl = (32usize.wrapping_sub(1).wrapping_sub(bit_field.bit_range_end_index)) as u32;
    let shr = (32usize.wrapping_sub(1).wrapping_sub(bit_field.bit_range_end_index.wrapping_sub(bit_field.bit_range_start_index))) as u32;
    Some(pixel_data.wrapping_shl(shl).wrapping_shr(shr))
}
