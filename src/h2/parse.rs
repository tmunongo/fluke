//! HTTP/2 parser
//!
//! HTTP/2 https://httpwg.org/specs/rfc9113.html
//! HTTP semantics https://httpwg.org/specs/rfc9110.html

use std::fmt;

use enum_repr::EnumRepr;
use enumflags2::{bitflags, BitFlags};
use nom::{
    combinator::map,
    number::streaming::{be_u24, be_u8},
    sequence::tuple,
    IResult,
};

use crate::{Roll, WriteOwned};

/// This is sent by h2 clients after negotiating over ALPN, or when doing h2c.
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub fn preface(i: Roll) -> IResult<Roll, ()> {
    let (i, _) = nom::bytes::streaming::tag(PREFACE)(i)?;
    Ok((i, ()))
}

/// See https://httpwg.org/specs/rfc9113.html#FrameTypes
#[EnumRepr(type = "u8")]
#[derive(Debug, Clone, Copy)]
pub enum RawFrameType {
    Data = 0,
    Headers = 1,
    Priority = 2,
    RstStream = 3,
    Settings = 4,
    PushPromise = 5,
    Ping = 6,
    GoAway = 7,
    WindowUpdate = 8,
    Continuation = 9,
}

/// Typed flags for various frame types
#[derive(Debug, Clone, Copy)]
pub enum FrameType {
    Data(BitFlags<DataFlags>),
    Headers(BitFlags<HeadersFlags>),
    Priority,
    RstStream,
    Settings(BitFlags<SettingsFlags>),
    PushPromise,
    Ping(BitFlags<PingFlags>),
    GoAway,
    WindowUpdate,
    Continuation,
    Unknown(EncodedFrameType),
}

/// See https://httpwg.org/specs/rfc9113.html#DATA
#[bitflags]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DataFlags {
    Padded = 0x08,
    EndStream = 0x01,
}

/// See https://httpwg.org/specs/rfc9113.html#rfc.section.6.2
#[bitflags]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HeadersFlags {
    Priority = 0x20,
    Padded = 0x08,
    EndHeaders = 0x04,
    EndStream = 0x01,
}

/// See https://httpwg.org/specs/rfc9113.html#SETTINGS
#[bitflags]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SettingsFlags {
    Ack = 0x01,
}

/// See https://httpwg.org/specs/rfc9113.html#PING
#[bitflags]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PingFlags {
    Ack = 0x01,
}

#[derive(Debug, Clone, Copy)]
pub struct EncodedFrameType {
    pub ty: u8,
    pub flags: u8,
}

impl EncodedFrameType {
    fn parse(i: Roll) -> IResult<Roll, Self> {
        let (i, (ty, flags)) = tuple((be_u8, be_u8))(i)?;
        Ok((i, Self { ty, flags }))
    }
}

impl From<(RawFrameType, u8)> for EncodedFrameType {
    fn from((ty, flags): (RawFrameType, u8)) -> Self {
        Self {
            ty: ty.repr(),
            flags,
        }
    }
}

impl FrameType {
    pub(crate) fn encode(self) -> EncodedFrameType {
        match self {
            FrameType::Data(f) => (RawFrameType::Data, f.bits()).into(),
            FrameType::Headers(f) => (RawFrameType::Headers, f.bits()).into(),
            FrameType::Priority => (RawFrameType::Priority, 0).into(),
            FrameType::RstStream => (RawFrameType::RstStream, 0).into(),
            FrameType::Settings(f) => (RawFrameType::Settings, f.bits()).into(),
            FrameType::PushPromise => (RawFrameType::PushPromise, 0).into(),
            FrameType::Ping(f) => (RawFrameType::Ping, f.bits()).into(),
            FrameType::GoAway => (RawFrameType::GoAway, 0).into(),
            FrameType::WindowUpdate => (RawFrameType::WindowUpdate, 0).into(),
            FrameType::Continuation => (RawFrameType::Continuation, 0).into(),
            FrameType::Unknown(ft) => ft,
        }
    }

    fn decode(ft: EncodedFrameType) -> Self {
        match RawFrameType::from_repr(ft.ty) {
            Some(ty) => match ty {
                RawFrameType::Data => {
                    FrameType::Data(BitFlags::<DataFlags>::from_bits_truncate(ft.flags))
                }
                RawFrameType::Headers => {
                    FrameType::Headers(BitFlags::<HeadersFlags>::from_bits_truncate(ft.flags))
                }
                RawFrameType::Priority => FrameType::Priority,
                RawFrameType::RstStream => FrameType::RstStream,
                RawFrameType::Settings => {
                    FrameType::Settings(BitFlags::<SettingsFlags>::from_bits_truncate(ft.flags))
                }
                RawFrameType::PushPromise => FrameType::PushPromise,
                RawFrameType::Ping => {
                    FrameType::Ping(BitFlags::<PingFlags>::from_bits_truncate(ft.flags))
                }
                RawFrameType::GoAway => FrameType::GoAway,
                RawFrameType::WindowUpdate => FrameType::WindowUpdate,
                RawFrameType::Continuation => FrameType::Continuation,
            },
            None => FrameType::Unknown(ft),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(u32);

impl StreamId {
    /// Stream ID used for connection control frames
    pub const CONNECTION: Self = Self(0);
}

#[derive(Debug, thiserror::Error)]
#[error("invalid stream id: {0}")]
pub struct StreamIdOutOfRange(u32);

impl TryFrom<u32> for StreamId {
    type Error = StreamIdOutOfRange;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value & 0x8000_0000 != 0 {
            Err(StreamIdOutOfRange(value))
        } else {
            Ok(Self(value))
        }
    }
}

impl fmt::Debug for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// See https://httpwg.org/specs/rfc9113.html#FrameHeader
#[derive(Debug)]
pub struct Frame {
    pub frame_type: FrameType,
    pub reserved: u8,
    pub stream_id: StreamId,
    pub len: u32,
}

impl Frame {
    /// Create a new frame with the given type and stream ID.
    pub fn new(frame_type: FrameType, stream_id: StreamId) -> Self {
        Self {
            frame_type,
            reserved: 0,
            stream_id,
            len: 0,
        }
    }

    /// Set the frame's length.
    pub fn with_len(mut self, len: u32) -> Self {
        self.len = len;
        self
    }

    /// Parse a frame from the given slice. This also takes the payload from the
    /// slice, and copies it to the heap, which may not be ideal for a production
    /// implementation.
    pub fn parse(i: Roll) -> IResult<Roll, Self> {
        let (i, (len, frame_type, (reserved, stream_id))) = tuple((
            be_u24,
            EncodedFrameType::parse,
            parse_reserved_and_stream_id,
        ))(i)?;

        let frame = Frame {
            frame_type: FrameType::decode(frame_type),
            reserved,
            stream_id,
            len,
        };
        Ok((i, frame))
    }

    /// Write a frame (without payload)
    pub async fn write(&self, w: &impl WriteOwned) -> eyre::Result<()> {
        let mut header = vec![0u8; 9];
        {
            use byteorder::{BigEndian, WriteBytesExt};
            let mut header = &mut header[..];
            header.write_u24::<BigEndian>(self.len as _)?;
            let ft = self.frame_type.encode();
            header.write_u8(ft.ty)?;
            header.write_u8(ft.flags)?;
            // TODO: do we ever need to write the reserved bit?
            header.write_u32::<BigEndian>(self.stream_id.0)?;
        }

        let (res, _) = w.write_all(header).await;
        res?;
        Ok(())
    }
}

/// See https://httpwg.org/specs/rfc9113.html#FrameHeader - the first bit
/// is reserved, and the rest is a 32-bit stream id
fn parse_reserved_and_stream_id(i: Roll) -> IResult<Roll, (u8, StreamId)> {
    fn reserved(i: (Roll, usize)) -> IResult<(Roll, usize), u8> {
        nom::bits::streaming::take(1_usize)(i)
    }

    fn stream_id(i: (Roll, usize)) -> IResult<(Roll, usize), StreamId> {
        nom::combinator::map(nom::bits::streaming::take(31_usize), StreamId)(i)
    }

    nom::bits::bits(tuple((reserved, stream_id)))(i)
}

// cf. https://httpwg.org/specs/rfc9113.html#HEADERS
#[derive(Debug)]
pub(crate) struct PrioritySpec {
    pub exclusive: bool,
    pub stream_dependency: StreamId,
    // 0-255 => 1-256
    pub weight: u8,
}

impl PrioritySpec {
    pub(crate) fn parse(i: Roll) -> IResult<Roll, Self> {
        map(
            tuple((parse_reserved_and_stream_id, be_u8)),
            |((exclusive, stream_dependency), weight)| Self {
                exclusive: exclusive != 0,
                stream_dependency,
                weight,
            },
        )(i)
    }
}
