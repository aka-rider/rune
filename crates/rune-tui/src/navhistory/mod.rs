use std::path::PathBuf;

use crate::document::DocumentId;

mod record;
#[cfg(test)]
mod tests;
mod travel;

pub use record::{
    departure_origin, observe_jump, record_departure, record_departure_if_moved, record_edit,
};
pub use travel::{back, forward};

const MAX_PLACES: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    Visited,
    Edited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub doc: DocumentId,
    pub path: Option<PathBuf>,
    pub offset: usize,
    pub kind: PlaceKind,
}

#[derive(Clone, Debug, Default)]
pub struct NavHistory {
    places: Vec<Place>,
    current: usize,
}

impl NavHistory {
    pub fn index(&self) -> usize {
        self.current
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    pub fn places(&self) -> &[Place] {
        &self.places
    }

    pub fn can_back(&self) -> bool {
        self.current > 0
    }

    pub fn can_forward(&self) -> bool {
        self.current + 1 < self.places.len()
    }

    fn push(&mut self, place: Place, replace_last: bool) {
        self.places.truncate(self.current);
        if replace_last {
            if let Some(last) = self.places.last_mut() {
                *last = place;
            } else {
                self.places.push(place);
            }
        } else {
            self.places.push(place);
        }
        while self.places.len() > MAX_PLACES {
            self.places.remove(0);
            self.current = self.current.saturating_sub(1);
        }
        self.current = self.places.len();
    }

    fn push_live(&mut self, place: Place) {
        self.places.push(place);
    }

    pub fn back(&mut self, live: Option<Place>) -> Option<Place> {
        if !self.can_back() {
            return None;
        }
        if self.current == self.places.len()
            && let Some(live) = live
        {
            self.push_live(live);
        }
        self.current -= 1;
        self.places.get(self.current).cloned()
    }

    pub fn forward(&mut self) -> Option<Place> {
        if !self.can_forward() {
            return None;
        }
        self.current += 1;
        self.places.get(self.current).cloned()
    }

    pub fn shift(&mut self, doc: DocumentId, start: usize, removed: usize, inserted: usize) {
        let removed_end = start.saturating_add(removed);
        for place in &mut self.places {
            if place.doc != doc {
                continue;
            }
            if place.offset <= start {
                continue;
            }
            if place.offset < removed_end {
                place.offset = start;
            } else {
                place.offset = place
                    .offset
                    .saturating_sub(removed)
                    .saturating_add(inserted);
            }
        }
    }

    pub fn drop_at(&mut self, index: usize) {
        if index >= self.places.len() {
            return;
        }
        self.places.remove(index);
        if index <= self.current {
            self.current = self.current.saturating_sub(1);
        }
    }

    pub fn drop_doc(&mut self, doc: DocumentId) {
        let mut i = 0;
        while let Some(place) = self.places.get(i) {
            if place.doc == doc {
                self.drop_at(i);
            } else {
                i += 1;
            }
        }
    }
}
