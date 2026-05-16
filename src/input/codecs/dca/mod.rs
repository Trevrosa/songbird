mod metadata;
pub use self::metadata::*;

use crate::constants::{SAMPLE_RATE, SAMPLE_RATE_RAW};

use std::io::{Seek, SeekFrom};
use symphonia::core::{
    audio::sample::SampleFormat,
    codecs::audio::well_known::CODEC_ID_MP2,
    codecs::CodecParameters,
    common::FourCc,
    errors::{self as symph_err, Error as SymphError, Result as SymphResult, SeekErrorKind},
    formats::probe::{ProbeDataMatchSpec, ProbeFormatData, Score},
    formats::probe::{ProbeableFormat, Scoreable},
    formats::{FormatId, FormatInfo, FormatOptions, FormatReader, MediaInfo, Track, TrackFlags},
    formats::{SeekMode, SeekTo, SeekedTo},
    io::ScopedStream,
    io::{MediaSource, MediaSourceStream, ReadBytes, SeekBuffered},
    meta::{Metadata as SymphMetadata, MetadataBuilder, MetadataLog, RawTag, StandardTag, Tag},
    packet::Packet,
    units::{TimeBase, Timestamp},
};

impl ProbeableFormat<'_> for DcaReader<'_> {
    fn probe_data() -> &'static [ProbeFormatData] {
        &[ProbeFormatData {
            info: FormatInfo {
                format: FormatId::new(FourCc::new([b'd', b'c', b'a', b' '])),
                short_name: "dca",
                long_name: "DCA[0/1] Opus Wrapper",
            },
            spec: ProbeDataMatchSpec {
                extensions: &["dca"], // FIXME: is this correct?
                mime_types: &[],
                markers: &[b"DCA1"],
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
        // Read in the magic number to verify it's a DCA file.
        let magic = mss.read_quad_bytes()?;

        // FIXME: make use of the new options.enable_gapless to apply the opus coder delay.

        let read_meta = match &magic {
            b"DCA1" => true,
            _ if &magic[..3] == b"DCA" => {
                return symph_err::unsupported_error("unsupported DCA version");
            },
            _ => {
                mss.seek_buffered_rel(-4);
                false
            },
        };

        let mut codec_params = CodecParameters::new();
        let timebase = TimeBase::new(1.into(), SAMPLE_RATE_RAW.into());

        codec_params
            .for_codec(CODEC_ID_MP2) // FIXME: it is supposed to be CODEC_TYPE_OPUS, but this is the value that has 0x1005, as it was before
            .with_max_frames_per_packet(1)
            .with_sample_rate(SAMPLE_RATE_RAW as u32)
            .with_time_base(timebase)
            .with_sample_format(SampleFormat::F32);

        let mut metas = MetadataLog::default();

        if read_meta {
            let size = mss.read_u32()?;

            // Sanity check
            if (size as i32) < 2 {
                return symph_err::decode_error("missing DCA1 metadata block");
            }

            let raw_json = mss.read_boxed_slice_exact(size as usize)?;

            let metadata: DcaMetadata = serde_json::from_slice::<DcaMetadata>(&raw_json)
                .map_err(|_| SymphError::DecodeError("malformed DCA1 metadata block"))?;

            let mut revision = MetadataBuilder::new(Default::default());

            if let Some(info) = metadata.info {
                if let Some(t) = info.title {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("title", t),
                        StandardTag::TrackTitle(t.into()),
                    ));
                }
                if let Some(t) = info.album {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("album", t),
                        StandardTag::Album(t.into()),
                    ));
                }
                if let Some(t) = info.artist {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("artist", t),
                        StandardTag::Artist(t.into()),
                    ));
                }
                if let Some(t) = info.genre {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("genre", t),
                        StandardTag::Genre(t.into()),
                    ));
                }
                if let Some(t) = info.comments {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("comments", t),
                        StandardTag::Comment(t.into()),
                    ));
                }
                if let Some(_t) = info.cover {
                    // TODO: Add visual, figure out MIME types.
                }
            }

            if let Some(origin) = metadata.origin {
                if let Some(t) = origin.url {
                    revision.add_tag(Tag::new_std(
                        RawTag::new("url", t),
                        StandardTag::Url(t.into()),
                    ));
                }
            }

            metas.push(revision.metadata());
        }

        let bytes_read = mss.pos();

        let reader = Self {
            source: mss,
            track: Some(Track {
                id: 0,
                language: None,
                codec_params,
                delay: None,
                duration: None,
                flags: TrackFlags::DEFAULT,
                num_frames: None,
                padding: None,
                start_ts: Timestamp::ZERO,
                time_base: Some(timebase),
            }),
            metas,
            seek_accel: SeekAccel::new(*opts, bytes_read),
            curr_ts: Timestamp::ZERO,
            max_ts: None,
            held_packet: None,
        };

        Ok(Box::new(reader))
    }
}

impl Scoreable for DcaReader<'_> {
    fn score(_src: ScopedStream<&mut MediaSourceStream<'_>>) -> SymphResult<Score> {
        Ok(Score::Supported(255))
    }
}

struct SeekAccel {
    frame_offsets: Vec<(Timestamp, u64)>,
    seek_index_fill_rate: u16,
    next_ts: Timestamp,
}

impl SeekAccel {
    fn new(options: FormatOptions, first_frame_byte_pos: u64) -> Self {
        let per_s = options.seek_index_fill_rate;
        let next_ts = (per_s as i64) * (SAMPLE_RATE_RAW as i64);

        Self {
            frame_offsets: vec![(Timestamp::ZERO, first_frame_byte_pos)],
            seek_index_fill_rate: per_s,
            next_ts: Timestamp::new(next_ts),
        }
    }

    fn update(&mut self, ts: Timestamp, pos: u64) {
        if ts >= self.next_ts {
            self.next_ts += (self.seek_index_fill_rate as u64) * (SAMPLE_RATE_RAW as u64);
            self.frame_offsets.push((ts, pos));
        }
    }

    fn get_seek_pos(&self, ts: Timestamp) -> (Timestamp, u64) {
        let index = self.frame_offsets.partition_point(|&(o_ts, _)| o_ts <= ts) - 1;
        self.frame_offsets[index]
    }
}

/// [DCA\[0/1\]](https://github.com/bwmarrin/dca) Format reader for Symphonia.
pub struct DcaReader<'s> {
    source: MediaSourceStream<'s>,
    track: Option<Track>,
    metas: MetadataLog,
    seek_accel: SeekAccel,
    curr_ts: Timestamp,
    max_ts: Option<Timestamp>,
    held_packet: Option<Packet>,
}

impl FormatReader for DcaReader<'_> {
    fn format_info(&self) -> &FormatInfo {
        &FormatInfo {
            format: FormatId::new(FourCc::new([b'd', b'c', b'a', b' '])),
            short_name: "dca",
            long_name: "DCA[0/1] Opus Wrapper",
        }
    }

    fn media_info(&self) -> &MediaInfo {
        MediaInfo::new()
            .with_start_ts(Timestamp::ZERO)
            .with_time_base(TimeBase::new(1.into(), SAMPLE_RATE_RAW.into()))
    }

    fn metadata(&mut self) -> SymphMetadata<'_> {
        self.metas.metadata()
    }

    fn seek(&mut self, _mode: SeekMode, to: SeekTo) -> SymphResult<SeekedTo> {
        let can_backseek = self.source.is_seekable();

        let Some(track) = &self.track else {
            return symph_err::seek_error(SeekErrorKind::Unseekable);
        };

        let rate = track.codec_params.sample_rate;
        let ts = match to {
            SeekTo::Time { time, .. } => {
                if let Some(rate) = rate {
                    TimeBase::new(1.into(), rate)
                        .calc_timestamp(time)
                        .expect("no overflow") // FIXME: ?
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

        let (accel_seek_ts, accel_seek_pos) = self.seek_accel.get_seek_pos(ts);

        if backseek_needed || accel_seek_pos > self.source.pos() {
            self.source.seek(SeekFrom::Start(accel_seek_pos))?;
            self.curr_ts = accel_seek_ts;
        }

        while let Ok(Some(pkt)) = self.next_packet() {
            let pts = pkt.ts;
            let dur = pkt.dur;
            let track_id = pkt.track_id();

            if (pts..pts + dur).contains(&ts) {
                self.held_packet = Some(pkt);
                return Ok(SeekedTo {
                    track_id,
                    required_ts: ts,
                    actual_ts: pts,
                });
            }
        }

        symph_err::seek_error(SeekErrorKind::OutOfRange)
    }

    fn tracks(&self) -> &[Track] {
        // DCA tracks can hold only one track by design.
        // Of course, a zero-length file is technically allowed,
        // in which case no track.
        self.track.as_slice()
    }

    fn default_track(&self, track_type: symphonia_core::formats::TrackType) -> Option<&Track> {
        self.track.as_ref()
    }

    fn next_packet(&mut self) -> SymphResult<Option<Packet>> {
        if let Some(pkt) = self.held_packet.take() {
            return Ok(Some(pkt));
        }

        let frame_pos = self.source.pos();

        let p_len = match self.source.read_u16() {
            Ok(len) => len as i16,
            Err(eof) => {
                self.max_ts = Some(self.curr_ts);
                return Err(eof.into());
            },
        };

        if p_len < 0 {
            return symph_err::decode_error("DCA frame header had a negative length.");
        }

        let buf = self.source.read_boxed_slice_exact(p_len as usize)?;

        if buf.is_empty() || buf.len() > i32::MAX as usize {
            return symph_err::decode_error(
                "Packet was not a valid Opus packet: too large for opus2.",
            );
        }

        let sample_ct = opus2::packet::get_nb_samples(&buf, SAMPLE_RATE).or_else(|_| {
            symph_err::decode_error(
                "Packet was not a valid Opus packet: couldn't read sample count.",
            )
        })? as u64;

        let out = Packet::new_from_boxed_slice(0, self.curr_ts, sample_ct, buf);

        self.seek_accel.update(self.curr_ts, frame_pos);

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

#[cfg(test)]
mod tests {
    use crate::input::input_tests::*;
    use crate::{constants::test_data::FILE_DCA_TARGET, input::File};

    // NOTE: this covers youtube audio in a non-copyright-violating way, since
    // those depend on an HttpRequest internally anyhow.
    #[tokio::test]
    #[ntest::timeout(10_000)]
    async fn dca_track_plays() {
        track_plays_passthrough(|| File::new(FILE_DCA_TARGET)).await;
    }

    #[tokio::test]
    #[ntest::timeout(10_000)]
    async fn dca_forward_seek_correct() {
        forward_seek_correct(|| File::new(FILE_DCA_TARGET)).await;
    }

    #[tokio::test]
    #[ntest::timeout(10_000)]
    async fn dca_backward_seek_correct() {
        backward_seek_correct(|| File::new(FILE_DCA_TARGET)).await;
    }

    #[tokio::test]
    #[ntest::timeout(10_000)]
    async fn opus_passthrough_when_other_tracks_paused() {
        track_plays_passthrough_when_is_only_active(|| File::new(FILE_DCA_TARGET)).await;
    }
}
