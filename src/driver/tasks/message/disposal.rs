#![allow(missing_docs)]

use crate::{driver::tasks::mixer::InternalTrack, tracks::TrackHandle};

#[allow(dead_code)] // We don't read because all we are doing is dropping.
pub enum DisposalMessage<'s> {
    Track(Box<InternalTrack<'s>>),
    Handle(TrackHandle),
}
