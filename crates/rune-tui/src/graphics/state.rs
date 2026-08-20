use std::sync::Arc;

use rune_image::{CellFootprint, ImageId, PixelSize};

pub enum ImageStatus {
    Pending,
    Live {
        decoded: Arc<rune_image::decode::Decoded>,
        cells: CellFootprint,
    },
    Failed(String),
}

pub struct ImageState {
    pub path: std::path::PathBuf,
    pub bytes_len: u64,
    pub id: ImageId,
    pub dims: Option<PixelSize>,
    pub status: ImageStatus,
    pub in_flight: Option<crate::generation::ImageDecodeGen>,
    pub next_generation: crate::generation::GenCounter<crate::generation::ImageDecode>,
}
