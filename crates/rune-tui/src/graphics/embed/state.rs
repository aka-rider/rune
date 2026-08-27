use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use rune_image::{ImageId, PixelSize};
use rune_md::snapshot::ImageDims;

use crate::graphics::ImageStatus;

pub struct EmbedState {
    pub abs_path: PathBuf,
    pub id: ImageId,
    pub mtime: Option<SystemTime>,
    pub dims: Option<PixelSize>,
    pub status: ImageStatus,
    pub in_flight: Option<u64>,
}

#[derive(Default)]
pub struct EmbedSet {
    pub images: HashMap<String, EmbedState>,
    pub(crate) next_generation: u64,
}

impl EmbedSet {
    pub fn new() -> EmbedSet {
        EmbedSet::default()
    }

    pub fn to_image_dims(&self) -> ImageDims {
        let mut dims = ImageDims::new();
        for (key, state) in &self.images {
            if let ImageStatus::Live { cells, .. } = &state.status {
                dims.insert(key.clone(), cells.cols, cells.rows);
            }
        }
        dims
    }

    pub fn has_wedged(&self) -> bool {
        self.images.values().any(|state| state.in_flight.is_some())
    }
}
