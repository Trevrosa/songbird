#![allow(missing_docs)]

use super::Interconnect;
use crate::{model::Event as GatewayEvent, ws::WsStream};

pub enum WsMessage<'s> {
    Ws(Box<WsStream>),
    ReplaceInterconnect(Interconnect<'s>),
    SetKeepalive(f64),
    Speaking(bool),
    Deliver(GatewayEvent),
}
