#![allow(missing_docs)]

use crate::{
    driver::{connection::error::Error, Bitrate, Config},
    events::{context_data::DisconnectReason, EventData},
    tracks::{Track, TrackCommand, TrackHandle},
    ConnectionInfo,
};
use flume::{Receiver, Sender};

pub enum CoreMessage<'s> {
    ConnectWithResult(ConnectionInfo, Sender<Result<(), Error>>),
    RetryConnect(usize),
    SignalWsClosure(usize, ConnectionInfo, Option<DisconnectReason>),
    Disconnect,
    SetTrack(Option<Box<TrackContext<'s>>>),
    AddTrack(Box<TrackContext<'s>>),
    SetBitrate(Bitrate),
    AddEvent(EventData),
    RemoveGlobalEvents,
    SetConfig(Config<'s>),
    Mute(bool),
    Reconnect,
    FullReconnect,
    RebuildInterconnect,
    Poison,
}

pub struct TrackContext<'s> {
    pub track: Track<'s>,
    pub handle: TrackHandle,
    pub receiver: Receiver<TrackCommand>,
}
