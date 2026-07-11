//! Minimal FFI binding to the CELT 0.11.1 decoder exported by
//! vaudio_celt_client.so, covering the two codecs TF2 demos use:
//! mono "vaudio_celt" (22050 Hz, 512-sample frames, 64-byte packets, as
//! used by celt_convert/csgo.c) and mono "vaudio_celt_high" (44100 Hz,
//! 1024-sample frames, 128-byte packets).

use std::os::raw::c_int;
use std::ptr;

/// Output sample rate every `CeltDecoder`, regardless of variant, decodes to.
pub const OUTPUT_SAMPLE_RATE: u32 = 22050;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeltVariant {
    /// "vaudio_celt": 22050 Hz, 512-sample frames, 64-byte packets.
    Standard,
    /// "vaudio_celt_high": 44100 Hz, 1024-sample frames, 128-byte packets.
    /// Despite the 44100 Hz mode, the decoded samples are used as-is at
    /// `OUTPUT_SAMPLE_RATE` with no resampling (matches celt_convert's
    /// csgo_high.c, which decodes the same way and just labels the raw
    /// output as 22050 Hz).
    High,
}

impl CeltVariant {
    fn sample_rate(self) -> c_int {
        match self {
            CeltVariant::Standard => 22050,
            CeltVariant::High => 44100,
        }
    }

    fn frame_size(self) -> usize {
        match self {
            CeltVariant::Standard => 512,
            CeltVariant::High => 1024,
        }
    }

    fn bytes_per_frame(self) -> usize {
        match self {
            CeltVariant::Standard => 64,
            CeltVariant::High => 128,
        }
    }
}

#[repr(C)]
struct CeltMode {
    _private: [u8; 0],
}

#[repr(C)]
struct CeltDecoderState {
    _private: [u8; 0],
}

extern "C" {
    fn celt_mode_create(fs: c_int, frame_size: c_int, error: *mut c_int) -> *mut CeltMode;
    fn celt_mode_destroy(mode: *mut CeltMode);

    fn celt_decoder_create_custom(
        mode: *const CeltMode,
        channels: c_int,
        error: *mut c_int,
    ) -> *mut CeltDecoderState;
    fn celt_decoder_destroy(st: *mut CeltDecoderState);

    fn celt_decode(
        st: *mut CeltDecoderState,
        data: *const u8,
        len: c_int,
        pcm: *mut i16,
        frame_size: c_int,
    ) -> c_int;
}

pub struct CeltDecoder {
    mode: *mut CeltMode,
    state: *mut CeltDecoderState,
    variant: CeltVariant,
}

impl CeltDecoder {
    pub fn new(variant: CeltVariant) -> Self {
        unsafe {
            let mode = celt_mode_create(
                variant.sample_rate(),
                variant.frame_size() as c_int,
                ptr::null_mut(),
            );
            assert!(!mode.is_null(), "celt_mode_create failed");

            let state = celt_decoder_create_custom(mode, 1, ptr::null_mut());
            assert!(!state.is_null(), "celt_decoder_create_custom failed");

            Self {
                mode,
                state,
                variant,
            }
        }
    }

    pub fn variant(&self) -> CeltVariant {
        self.variant
    }

    /// Decodes a buffer of raw CELT voice data (fixed-size frames per
    /// `variant`) into 16-bit mono PCM samples, mirroring csgo.c /
    /// csgo_high.c's decode loop. The raw decoded samples are returned
    /// as-is (no resampling) and are meant to be played back at
    /// `OUTPUT_SAMPLE_RATE` regardless of variant. Any trailing bytes that
    /// don't fill a full frame are dropped.
    pub fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        let bytes_per_frame = self.variant.bytes_per_frame();
        let frame_size = self.variant.frame_size();
        let frames = data.len() / bytes_per_frame;
        let mut pcm = vec![0i16; frames * frame_size];

        for i in 0..frames {
            let chunk = &data[i * bytes_per_frame..(i + 1) * bytes_per_frame];
            let out = &mut pcm[i * frame_size..(i + 1) * frame_size];
            let ret = unsafe {
                celt_decode(
                    self.state,
                    chunk.as_ptr(),
                    bytes_per_frame as c_int,
                    out.as_mut_ptr(),
                    frame_size as c_int,
                )
            };
            if ret < 0 {
                eprintln!("celt_decode failed: {} (frame {}/{})", ret, i, frames);
            }
        }

        pcm
    }
}

impl Drop for CeltDecoder {
    fn drop(&mut self) {
        unsafe {
            celt_decoder_destroy(self.state);
            celt_mode_destroy(self.mode);
        }
    }
}
