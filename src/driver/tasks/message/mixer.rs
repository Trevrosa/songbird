#![allow(missing_docs)]

#[cfg(feature = "receive")]
use super::UdpRxMessage;
use super::{Interconnect, TrackContext, WsMessage};

use crate::{
    driver::{crypto::Cipher, Bitrate, Config, CryptoState},
    input::{AudioStreamError, Compose, Parsed},
};
use flume::Sender;
use std::{
    net::UdpSocket,
    sync::{atomic::AtomicU16, Arc, RwLock},
};
use symphonia_core::{errors::Error as SymphoniaError, formats::SeekedTo};

pub struct MixerConnection {
    pub cipher: Cipher,
    pub crypto_state: CryptoState,
    pub dave_session: Arc<RwLock<Option<davey::DaveSession>>>,
    pub dave_protocol_version: Arc<AtomicU16>,
    #[cfg(feature = "receive")]
    pub udp_rx: Sender<UdpRxMessage>,
    pub udp_tx: UdpSocket,
}

pub enum MixerMessage<'s> {
    AddTrack(Box<TrackContext<'s>>),
    SetTrack(Option<Box<TrackContext<'s>>>),

    SetBitrate(Bitrate),
    SetConfig(Config<'s>),
    SetMute(bool),

    SetConn(MixerConnection, u32),
    Ws(Option<Sender<WsMessage<'s>>>),
    DropConn,

    ReplaceInterconnect(Interconnect<'s>),
    RebuildEncoder,

    Poison,
}

impl MixerMessage<'_> {
    #[must_use]
    pub fn is_mixer_maybe_live(&self) -> bool {
        matches!(
            self,
            Self::AddTrack(_) | Self::SetTrack(Some(_)) | Self::SetConn(..)
        )
    }
}

pub enum MixerInputResultMessage {
    CreateErr(Arc<AudioStreamError>),
    ParseErr(Arc<SymphoniaError>),
    Seek(
        Parsed,
        Option<Box<dyn Compose>>,
        Result<SeekedTo, Arc<SymphoniaError>>,
    ),
    Built(Parsed, Option<Box<dyn Compose>>),
}
