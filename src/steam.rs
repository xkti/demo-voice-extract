//! Steam voice codec parsing and decoding, adapted from the `steam-audio-codec` crate
//! (https://codeberg.org/demostf/steam-audio-codec), inlined here (rather than pulled in as
//! a dependency) so this binary stays a single self-contained crate, matching how `celt.rs`
//! is a local module rather than an external dependency.
//!
//! The "steam" audio codec is a thin wrapper around Opus: each `VoiceData` message is a
//! small stream of sub-packets (sample rate, silence run-length, Opus/PLC frames) prefixed
//! by a SteamID64 header and CRC32 checksum. See
//! https://zhenyangli.me/posts/reversing-steam-voice-codec/ for the original reverse
//! engineering this is based on.

use opus::{Channels, Decoder};
use std::fmt::{Debug, Display};

#[derive(Debug)]
pub enum SteamAudioError {
    CrcMismatch { expected: u32, actual: u32 },
    InsufficientData,
    InsufficientOutputBuffer,
    UnknownPacketType { ty: u8 },
    Opus(opus::Error),
    NoSampleRate,
}

impl Display for SteamAudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteamAudioError::CrcMismatch { expected, actual } => write!(
                f,
                "crc mismatch for packet, got {actual}, expected: {expected}"
            ),
            SteamAudioError::InsufficientData => write!(f, "insufficient number of bytes provided"),
            SteamAudioError::InsufficientOutputBuffer => {
                write!(f, "insufficient space in output buffer")
            }
            SteamAudioError::UnknownPacketType { ty } => write!(f, "unknown packet type: {ty}"),
            SteamAudioError::Opus(e) => write!(f, "{e}"),
            SteamAudioError::NoSampleRate => {
                write!(f, "audio data received before sample rate is set")
            }
        }
    }
}

impl From<opus::Error> for SteamAudioError {
    fn from(e: opus::Error) -> Self {
        SteamAudioError::Opus(e)
    }
}

/// The voice packet types seen in the voice data
#[derive(Debug)]
#[repr(u8)]
enum PacketType {
    Silence = 0,
    OpusPlc = 6,
    SampleRate = 11,
}

impl TryFrom<u8> for PacketType {
    type Error = SteamAudioError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Silence),
            6 => Ok(Self::OpusPlc),
            11 => Ok(Self::SampleRate),
            _ => Err(SteamAudioError::UnknownPacketType { ty: value }),
        }
    }
}

fn read_bytes<const N: usize>(data: &[u8]) -> Result<([u8; N], &[u8]), SteamAudioError> {
    if data.len() < N {
        Err(SteamAudioError::InsufficientData)
    } else {
        let (result, rest) = data.split_at(N);
        Ok((result.try_into().unwrap(), rest))
    }
}

fn read_u16(data: &[u8]) -> Result<(u16, &[u8]), SteamAudioError> {
    let (bytes, data) = read_bytes(data)?;
    Ok((u16::from_le_bytes(bytes), data))
}

/// A packet contained in the voice data
#[derive(Debug)]
pub enum Packet<'a> {
    /// A number of samples of silence
    Silence(u16),
    /// Opus PLC data
    OpusPlc(SteamOpusData<'a>),
    /// The sample rate for the opus packets
    SampleRate(u16),
}

impl<'a> Packet<'a> {
    pub fn read(data: &'a [u8]) -> Result<(Self, &'a [u8]), SteamAudioError> {
        let ty = PacketType::try_from(*data.first().ok_or(SteamAudioError::InsufficientData)?)?;
        let data = &data[1..];

        let (next, data) = read_u16(data)?;

        Ok(match ty {
            PacketType::Silence => (Packet::Silence(next), data),
            PacketType::OpusPlc => {
                if data.len() < next as usize {
                    return Err(SteamAudioError::InsufficientData);
                } else {
                    let (result, data) = data.split_at(next as usize);
                    (Packet::OpusPlc(SteamOpusData { data: result }), data)
                }
            }
            PacketType::SampleRate => (Packet::SampleRate(next), data),
        })
    }
}

/// A light-parsed voice data wrapper
///
/// Each bit of voice data contains two or three smaller packets
///
/// - The sample rate
/// - A number of silence samples since the last voice data
/// - The opus voice data
#[derive(Debug)]
pub struct SteamVoiceData<'a> {
    // Kept for API parity with the upstream steam-audio-codec crate this was adapted from;
    // this binary identifies speakers via the demo's userinfo string table instead (see
    // `client_steam_ids`), so nothing here reads it.
    #[allow(dead_code)]
    pub steam_id: u64,
    packet_data: &'a [u8],
}

impl<'a> SteamVoiceData<'a> {
    /// Parse the header of the voice data and validate the CRC checksum
    pub fn new(data: &'a [u8]) -> Result<Self, SteamAudioError> {
        if data.len() < 4 {
            return Err(SteamAudioError::InsufficientData);
        }
        let (data, crc_data) = data.split_at(data.len() - 4);
        let expected_crc = u32::from_le_bytes(crc_data.try_into().unwrap());
        let calculated_crc = crc32b(data);
        if expected_crc != calculated_crc {
            return Err(SteamAudioError::CrcMismatch {
                actual: calculated_crc,
                expected: expected_crc,
            });
        }

        let (steam_id_bytes, data) = read_bytes(data)?;
        let steam_id = u64::from_le_bytes(steam_id_bytes);
        Ok(SteamVoiceData {
            steam_id,
            packet_data: data,
        })
    }

    /// Get the packets contained in the data
    pub fn packets(&self) -> impl Iterator<Item = Result<Packet<'a>, SteamAudioError>> {
        SteamPacketIterator {
            data: self.packet_data,
        }
    }

    /// Peek the sample rate this voice data declares, without decoding any audio.
    ///
    /// Returns the rate from the first `SampleRate` packet found, or `None` if there isn't
    /// one (e.g. the data is malformed, or only contains `Silence`/`OpusPlc` packets).
    pub fn sample_rate(&self) -> Option<u16> {
        self.packets().find_map(|packet| match packet {
            Ok(Packet::SampleRate(rate)) => Some(rate),
            _ => None,
        })
    }
}

struct SteamPacketIterator<'a> {
    data: &'a [u8],
}

impl Debug for SteamPacketIterator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamPacketIterator")
            .field("data_length", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Iterator for SteamPacketIterator<'a> {
    type Item = Result<Packet<'a>, SteamAudioError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            None
        } else {
            match Packet::read(self.data) {
                Ok((packet, rest)) => {
                    self.data = rest;
                    Some(Ok(packet))
                }
                Err(e) => Some(Err(e)),
            }
        }
    }
}

fn crc32b(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (-((crc & 1) as i32)) as u32;
            crc = (crc >> 1) ^ (0xEDB88320 & mask);
        }
    }
    !crc
}

/// Raw opus data
pub struct SteamOpusData<'a> {
    data: &'a [u8],
}

impl Debug for SteamOpusData<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamOpusData")
            .field("data_length", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// A decoder for the steam voice data
///
/// This transforms the "lightly parsed" `SteamVoiceData` into 16-bit PCM voice data
#[derive(Default)]
pub struct SteamVoiceDecoder {
    decoder: Option<Decoder>,
    sample_rate: u16,
    // `None` means no frame has been seen yet by the current `decoder` instance, so there is
    // no prior state to reconstruct via PLC and any observed sequence number is the start.
    seq: Option<u16>,
}

impl SteamVoiceDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a `SteamVoiceData` into 16-bit PCM sound data
    ///
    /// The raw samples are written into the provided output buffer. The number of samples written into the buffer is returned.
    pub fn decode(
        &mut self,
        voice_data: SteamVoiceData,
        output_buffer: &mut [i16],
    ) -> Result<usize, SteamAudioError> {
        let mut total = 0;
        for packet in voice_data.packets() {
            let packet = packet?;
            match packet {
                Packet::SampleRate(rate) => {
                    if self.sample_rate != rate {
                        self.decoder = Some(Decoder::new(rate as u32, Channels::Mono)?);
                        self.sample_rate = rate;
                        self.seq = None;
                    }
                }
                Packet::OpusPlc(opus) => {
                    let count = self.decode_opus(opus.data, &mut output_buffer[total..])?;
                    total += count;
                    if total >= output_buffer.len() {
                        return Err(SteamAudioError::InsufficientOutputBuffer);
                    }
                }
                Packet::Silence(silence) => {
                    let silence = silence as usize;
                    if total + silence > output_buffer.len() {
                        return Err(SteamAudioError::InsufficientOutputBuffer);
                    }
                    // Explicitly zero this range: `output_buffer` is reused across calls by
                    // callers decoding a continuous stream, so without this a silence gap
                    // would play back whatever stale, non-silent samples were left over from
                    // a previous call instead of true silence.
                    output_buffer[total..total + silence].fill(0);
                    total += silence;
                }
            }
        }
        Ok(total)
    }

    fn decode_opus(
        &mut self,
        mut data: &[u8],
        output_buffer: &mut [i16],
    ) -> Result<usize, SteamAudioError> {
        let mut total = 0;
        let Some(decoder) = self.decoder.as_mut() else {
            return Err(SteamAudioError::NoSampleRate);
        };

        while data.len() > 2 {
            let (len, remainder) = read_u16(data)?;
            data = remainder;
            if len == u16::MAX {
                decoder.reset_state()?;
                self.seq = None;
                continue;
            }
            let (seq, remainder) = read_u16(data)?;
            data = remainder;

            match self.seq {
                // No prior frame in this decoder's lifetime (e.g. the very first frame seen,
                // possibly starting mid-stream): there is nothing to conceal, just start here.
                None => {}
                Some(expected) if seq < expected => {
                    decoder.reset_state()?;
                }
                Some(expected) => {
                    let lost = seq - expected;
                    for _ in 0..lost {
                        // For PLC (empty input) opus requires frame_size to be an exact multiple
                        // of 2.5ms of samples, so we must request exactly one lost frame's worth
                        // of samples rather than however much space happens to be left in
                        // `output_buffer` (which triggers BAD_ARG when it isn't such a multiple).
                        let frame_size = decoder.get_last_packet_duration()? as usize;
                        if total + frame_size > output_buffer.len() {
                            return Err(SteamAudioError::InsufficientOutputBuffer);
                        }
                        let count = decoder.decode(
                            &[],
                            &mut output_buffer[total..total + frame_size],
                            false,
                        )?;
                        total += count;
                        if total >= output_buffer.len() {
                            return Err(SteamAudioError::InsufficientOutputBuffer);
                        }
                    }
                }
            }
            let len = len as usize;

            self.seq = Some(seq + 1);

            if data.len() < len {
                return Err(SteamAudioError::InsufficientData);
            }

            let count = decoder.decode(&data[0..len], &mut output_buffer[total..], false)?;
            data = &data[len..];
            total += count;
            if total >= output_buffer.len() {
                return Err(SteamAudioError::InsufficientOutputBuffer);
            }
        }

        Ok(total)
    }
}
