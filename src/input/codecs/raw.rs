use std::io::{Seek, SeekFrom};
use symphonia::core::{
    audio::{sample::SampleFormat, Channels},
    codecs::audio::well_known::CODEC_ID_PCM_F32LE,
    codecs::CodecParameters,
    common::FourCc,
    errors::{self as symph_err, Result as SymphResult, SeekErrorKind},
    formats::probe::{ProbeDataMatchSpec, ProbeFormatData, Score},
    formats::probe::{ProbeableFormat, Scoreable},
    formats::{FormatId, FormatInfo, FormatOptions, FormatReader, MediaInfo, Track, TrackFlags},
    formats::{SeekMode, SeekTo, SeekedTo},
    io::ScopedStream,
    io::{MediaSource, MediaSourceStream, ReadBytes, SeekBuffered},
    meta::{Metadata as SymphMetadata, MetadataLog},
    packet::Packet,
    units::{TimeBase, Timestamp},
};

impl ProbeableFormat<'_> for RawReader<'_> {
    fn probe_data() -> &'static [ProbeFormatData] {
        &[ProbeFormatData {
            info: FormatInfo {
                format: FormatId::new(FourCc::new([b's', b'r', b'a', b'w'])),
                short_name: "raw",
                long_name: "Raw arbitrary-length f32 audio container.",
            },
            spec: ProbeDataMatchSpec {
                extensions: &["rawf32"],
                mime_types: &[],
                markers: &[b"SbirdRaw"],
            },
        }]
    }
    fn try_probe_new(
        mss: MediaSourceStream<'_>,
        opts: FormatOptions,
    ) -> SymphResult<Box<dyn FormatReader + '_>>
    where
        Self: Sized,
    {
        let mut magic = [0u8; 8];
        ReadBytes::read_buf_exact(&mut mss, &mut magic[..])?;

        if &magic != b"SbirdRaw" {
            mss.seek_buffered_rel(-(magic.len() as isize));
            return symph_err::decode_error("rawf32: illegal magic byte sequence.");
        }

        let sample_rate = mss.read_u32()?;
        let n_chans = mss.read_u32()?;

        let chans = match n_chans {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => {
                return symph_err::decode_error(
                    "rawf32: channel layout is not stereo or mono for fmt_pcm",
                )
            },
        };

        let timebase = TimeBase::new(1.into(), sample_rate.into());
        let mut codec_params = CodecParameters::new();

        codec_params
            .for_codec(CODEC_ID_PCM_F32LE)
            .with_bits_per_coded_sample((std::mem::size_of::<f32>() as u32) * 8)
            .with_bits_per_sample((std::mem::size_of::<f32>() as u32) * 8)
            .with_sample_rate(sample_rate)
            .with_time_base(timebase)
            .with_sample_format(SampleFormat::F32)
            .with_max_frames_per_packet(sample_rate as u64 / 50)
            .with_channels(chans);

        let reader = Self {
            source: mss,
            track: Track {
                id: 0,
                language: None,
                codec_params,
                delay: None,
                duration: None,
                flags: TrackFlags::DEFAULT,
                time_base: Some(timebase),
                num_frames: None,
                start_ts: Timestamp::ZERO,
                padding: None,
            },
            meta: MetadataLog::default(),
            curr_ts: Timestamp::ZERO,
            max_ts: None,
        };

        Ok(Box::new(reader))
    }
}

impl Scoreable for RawReader<'_> {
    fn score(_src: ScopedStream<&mut MediaSourceStream<'_>>) -> SymphResult<Score> {
        Ok(Score::Supported(255))
    }
}

/// Symphonia support for a simple container for raw f32-PCM data of unknown duration.
///
/// Contained files have a simple header:
/// * the 8-byte signature `b"SbirdRaw"`,
/// * the sample rate, as a little-endian `u32`,
/// * the channel count, as a little-endian `u32`.
///
/// The remainder of the file is interleaved little-endian `f32` samples.
pub struct RawReader<'s> {
    source: MediaSourceStream<'s>,
    track: Track,
    meta: MetadataLog,
    curr_ts: Timestamp,
    max_ts: Option<Timestamp>,
}

impl FormatReader for RawReader<'_> {
    fn format_info(&self) -> &FormatInfo {
        &FormatInfo {
            format: FormatId::new(FourCc::new([b's', b'r', b'a', b'w'])),
            short_name: "raw",
            long_name: "Raw arbitrary-length f32 audio container.",
        }
    }

    fn media_info(&self) -> &MediaInfo {
        // FIXME: maybe needs the timebase but needs read
        MediaInfo::new().with_start_ts(Timestamp::ZERO)
    }

    fn metadata(&mut self) -> SymphMetadata<'_> {
        self.meta.metadata()
    }

    fn seek(&mut self, _mode: SeekMode, to: SeekTo) -> SymphResult<SeekedTo> {
        let can_backseek = self.source.is_seekable();

        let track = &self.track;
        let rate = track.codec_params.sample_rate;
        let ts = match to {
            SeekTo::Time { time, .. } => {
                if let Some(rate) = rate {
                    TimeBase::new(1.into(), rate)
                        .calc_timestamp(time)
                        .expect("should not overflow?") // FIXME: is this true
                } else {
                    return symph_err::seek_error(SeekErrorKind::Unseekable);
                }
            },
            SeekTo::TimeStamp { ts, .. } => ts,
        };

        if let Some(max_ts) = self.max_ts {
            if ts > max_ts {
                return symph_err::seek_error(SeekErrorKind::OutOfRange);
            }
        }

        let backseek_needed = self.curr_ts > ts;

        if backseek_needed && !can_backseek {
            return symph_err::seek_error(SeekErrorKind::ForwardOnly);
        }

        let chan_count = track
            .codec_params
            .channels
            .expect("Channel count is built into format.")
            .count() as u64;

        let seek_pos = 16 + (std::mem::size_of::<f32>() as u64) * (ts * chan_count);

        self.source.seek(SeekFrom::Start(seek_pos))?;
        self.curr_ts = ts;

        Ok(SeekedTo {
            track_id: track.id,
            required_ts: ts,
            actual_ts: ts,
        })
    }

    fn tracks(&self) -> &[Track] {
        std::slice::from_ref(&self.track)
    }

    fn default_track(&self, track_type: symphonia_core::formats::TrackType) -> Option<&Track> {
        Some(&self.track)
    }

    fn next_packet(&mut self) -> SymphResult<Option<Packet>> {
        let track = &self.track;
        let rate = track
            .codec_params
            .sample_rate
            .expect("Sample rate is built into format.") as usize;

        let chan_count = track
            .codec_params
            .channels
            .expect("Channel count is built into format.")
            .count();

        let sample_unit = std::mem::size_of::<f32>() * chan_count;

        // Aim for 20ms (50Hz).
        let buf = self.source.read_boxed_slice((rate / 50) * sample_unit)?;

        let sample_ct = (buf.len() / sample_unit) as u64;
        let out = Packet::new_from_boxed_slice(0, self.curr_ts, sample_ct, buf);

        self.curr_ts += sample_ct;

        Ok(out)
    }

    fn into_inner<'s>(self: Box<Self>) -> MediaSourceStream<'s>
    where
        Self: 's,
    {
        self.source
    }
}
