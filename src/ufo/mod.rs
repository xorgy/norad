mod fontinfo;
mod layercontents;
mod glyph;

use serde::Deserialize;

pub use fontinfo::FontInfo;
pub use layercontents::LayerContents;
pub use glyph::{Color, Contour, ContourPoint, Identifier, Guideline, GlifVersion, Glyph, Outline, AffineTransform, Anchor, PointType};

pub struct Ufo {
    meta_info: MetaInfo,
    font_info: Option<FontInfo>,
    layer_contents: Vec<LayerContents>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaInfo {
    pub creator: String,
    pub format_version: u32,
}
