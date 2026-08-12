//! `Document`'s graphics accessors (split out of `document.rs` for the
//! 500-line-file budget): the `image`/`embeds` read/write pairs plus
//! `ensure_embeds`, the sole place `graphics` ever transitions into the
//! `Embeds` variant.

use rune_core::assert_invariant;

use crate::graphics::{EmbedSet, Graphics, ImageState};

use super::Document;

impl Document {
    pub fn image(&self) -> Option<&ImageState> {
        match &self.graphics {
            Graphics::Image(state) => Some(state),
            _ => None,
        }
    }

    pub fn image_mut(&mut self) -> Option<&mut ImageState> {
        match &mut self.graphics {
            Graphics::Image(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn set_image(&mut self, state: ImageState) {
        self.graphics = Graphics::Image(state);
    }

    pub fn embeds(&self) -> Option<&EmbedSet> {
        match &self.graphics {
            Graphics::Embeds(set) => Some(set),
            _ => None,
        }
    }

    pub fn embeds_mut(&mut self) -> Option<&mut EmbedSet> {
        match &mut self.graphics {
            Graphics::Embeds(set) => Some(set),
            _ => None,
        }
    }

    pub(crate) fn ensure_embeds(&mut self) -> &mut EmbedSet {
        assert_invariant!(
            !matches!(self.graphics, Graphics::Image(_)),
            || "ensure_embeds called on an image document"
        );
        if !matches!(self.graphics, Graphics::Embeds(_)) {
            self.graphics = Graphics::Embeds(EmbedSet::new());
        }
        let Graphics::Embeds(set) = &mut self.graphics else {
            unreachable!()
        };
        set
    }
}
