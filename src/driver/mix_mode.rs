use opus2::Channels as OpusChannels;
use symphonia_core::audio::{layouts, Channels};

use crate::constants::{MONO_FRAME_SIZE, STEREO_FRAME_SIZE};

/// Mixing behaviour for sent audio sources processed within the driver.
///
/// This has no impact on Opus packet passthrough, which will pass packets
/// irrespective of their channel count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixMode {
    /// Audio sources will be downmixed into a mono buffer.
    Mono,
    /// Audio sources will be mixed into into a stereo buffer, where mono sources
    /// will be duplicated into both channels.
    Stereo,
}

impl MixMode {
    pub(crate) const fn to_opus(self) -> OpusChannels {
        match self {
            Self::Mono => OpusChannels::Mono,
            Self::Stereo => OpusChannels::Stereo,
        }
    }

    pub(crate) const fn sample_count_in_frame(self) -> usize {
        match self {
            Self::Mono => MONO_FRAME_SIZE,
            Self::Stereo => STEREO_FRAME_SIZE,
        }
    }

    pub(crate) const fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }

    pub(crate) const fn symph_layout(self) -> Channels {
        match self {
            Self::Mono => layouts::CHANNEL_LAYOUT_MONO,
            Self::Stereo => layouts::CHANNEL_LAYOUT_STEREO,
        }
    }
}

impl From<MixMode> for Channels {
    fn from(val: MixMode) -> Self {
        val.symph_layout()
    }
}

impl From<MixMode> for OpusChannels {
    fn from(val: MixMode) -> Self {
        val.to_opus()
    }
}
