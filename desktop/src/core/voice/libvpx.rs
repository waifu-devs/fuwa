//! VP8 on a libvpx linked as it is, through its own C calls: Windows' path,
//! where the prebuilt shiguredo_libvpx is a MinGW build MSVC can't link.
//! It sets libvpx up as shiguredo_libvpx does for `vp8.rs`, so both send
//! the same stream.

use std::ffi::c_int;
use std::mem::MaybeUninit;
use std::ptr;

use vpx_sys as sys;

use super::vp8::{Encoded, Yuv, to_bgra};

/// A VP8 encoder for one size, at a steady bitrate, made for calls.
pub struct Encoder {
    ctx: sys::vpx_codec_ctx_t,
    image: sys::vpx_image_t,
    frames: i64,
    pub width: u32,
    pub height: u32,
}

// SAFETY: the context is used by one thread at a time, through &mut self.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: u32, height: u32, bitrate: u32, fps: u32, screen: bool) -> Result<Self, String> {
        let fps = fps.max(1);
        // SAFETY: plain libvpx calls; every pointer outlives the call it's passed to.
        unsafe {
            let iface = sys::vpx_codec_vp8_cx();
            let mut cfg = MaybeUninit::<sys::vpx_codec_enc_cfg_t>::zeroed();
            check(sys::vpx_codec_enc_config_default(iface, cfg.as_mut_ptr(), 0), "config")?;
            let mut cfg = cfg.assume_init();
            cfg.g_w = width;
            cfg.g_h = height;
            cfg.g_timebase.num = 1;
            cfg.g_timebase.den = fps as c_int;
            cfg.rc_target_bitrate = bitrate / 1000;
            cfg.rc_min_quantizer = 2;
            cfg.rc_max_quantizer = 56;
            cfg.rc_end_usage = sys::vpx_rc_mode::VPX_CBR;
            cfg.g_error_resilient = 1;
            // Keyframes come when asked (a viewer starting or switching), and every 10 s anyway.
            cfg.kf_max_dist = fps * 10;
            cfg.g_threads = if width * height >= 1280 * 720 { 4 } else { 1 };

            let mut ctx = MaybeUninit::<sys::vpx_codec_ctx_t>::zeroed();
            check(
                sys::vpx_codec_enc_init_ver(ctx.as_mut_ptr(), iface, &cfg, 0, sys::VPX_ENCODER_ABI_VERSION as c_int),
                "init",
            )?;
            let mut this =
                Self { ctx: ctx.assume_init(), image: MaybeUninit::zeroed().assume_init(), frames: 0, width, height };
            this.control(sys::vp8e_enc_control_id::VP8E_SET_CQ_LEVEL, 10)?;
            // Positive in real time: libvpx picks its speed by how long frames take.
            this.control(sys::vp8e_enc_control_id::VP8E_SET_CPUUSED, 8)?;
            if screen {
                // Screens change little between frames: what stays still costs nothing.
                this.control(sys::vp8e_enc_control_id::VP8E_SET_STATIC_THRESHOLD, 100)?;
            }
            this.control(sys::vp8e_enc_control_id::VP8E_SET_MAX_INTRA_BITRATE_PCT, if screen { 600 } else { 300 })?;
            Ok(this)
        }
    }

    fn control(&mut self, id: sys::vp8e_enc_control_id, value: c_int) -> Result<(), String> {
        // SAFETY: these controls each take one int.
        check(unsafe { sys::vpx_codec_control_(&mut self.ctx, id as c_int, value) }, "control")
    }

    /// Encodes one picture this encoder's size.
    pub fn encode(&mut self, yuv: &Yuv, keyframe: bool) -> Vec<Encoded> {
        let mut out = Vec::new();
        if (yuv.width, yuv.height) != (self.width, self.height) {
            return out;
        }
        let flags = if keyframe { sys::VPX_EFLAG_FORCE_KF as sys::vpx_enc_frame_flags_t } else { 0 };
        // SAFETY: the image only borrows `yuv`'s packed planes, for this call;
        // libvpx reads them as I420 at this size and copies what it keeps.
        unsafe {
            let wrapped = sys::vpx_img_wrap(
                &mut self.image,
                sys::vpx_img_fmt::VPX_IMG_FMT_I420,
                self.width,
                self.height,
                1,
                yuv.data.as_ptr().cast_mut(),
            );
            if wrapped.is_null() {
                return out;
            }
            let code = sys::vpx_codec_encode(
                &mut self.ctx,
                &self.image,
                self.frames,
                1,
                flags,
                // Its type has a name only from libvpx 1.15 on.
                sys::VPX_DL_REALTIME as _,
            );
            self.frames += 1;
            if code != sys::VPX_CODEC_OK {
                return out;
            }
            let mut iter = ptr::null();
            loop {
                let packet = sys::vpx_codec_get_cx_data(&mut self.ctx, &mut iter);
                let Some(packet) = packet.as_ref() else { break };
                if packet.kind != sys::vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    continue;
                }
                let frame = packet.data.frame;
                let data = std::slice::from_raw_parts(frame.buf.cast::<u8>(), frame.sz).to_vec();
                out.push(Encoded { data, keyframe: frame.flags & sys::VPX_FRAME_IS_KEY != 0 });
            }
        }
        out
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: made by vpx_codec_enc_init_ver and destroyed once.
        unsafe { sys::vpx_codec_destroy(&mut self.ctx) };
    }
}

/// A VP8 decoder that hands back pictures as BGRA.
pub struct Decoder {
    ctx: sys::vpx_codec_ctx_t,
}

// SAFETY: as for the encoder.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self, String> {
        // SAFETY: plain libvpx calls; no configuration means its defaults.
        unsafe {
            let mut ctx = MaybeUninit::<sys::vpx_codec_ctx_t>::zeroed();
            check(
                sys::vpx_codec_dec_init_ver(
                    ctx.as_mut_ptr(),
                    sys::vpx_codec_vp8_dx(),
                    ptr::null(),
                    0,
                    sys::VPX_DECODER_ABI_VERSION as c_int,
                ),
                "init",
            )?;
            Ok(Self { ctx: ctx.assume_init() })
        }
    }

    /// Decodes one frame into `out` (BGRA, its buffer reused), giving its
    /// size; None when it didn't decode or held no picture.
    pub fn decode(&mut self, frame: &[u8], out: &mut Vec<u8>) -> Option<(u32, u32)> {
        let len = u32::try_from(frame.len()).ok()?;
        // SAFETY: libvpx reads `frame` during the call; the pictures it gives
        // back stay valid until the next decode, and are read before then.
        unsafe {
            if sys::vpx_codec_decode(&mut self.ctx, frame.as_ptr(), len, ptr::null_mut(), 0) != sys::VPX_CODEC_OK {
                return None;
            }
            let mut size = None;
            let mut iter = ptr::null();
            loop {
                let picture = sys::vpx_codec_get_frame(&mut self.ctx, &mut iter);
                let Some(p) = picture.as_ref() else { break };
                if p.fmt != sys::vpx_img_fmt::VPX_IMG_FMT_I420 {
                    continue;
                }
                let (w, h) = (p.d_w, p.d_h);
                let plane = |i: usize, rows: u32| {
                    let stride = p.stride[i] as usize;
                    (std::slice::from_raw_parts(p.planes[i], stride * rows as usize), stride)
                };
                let half = h.div_ceil(2);
                to_bgra(plane(0, h), plane(1, half), plane(2, half), w, h, false, out);
                size = Some((w, h));
            }
            size
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: made by vpx_codec_dec_init_ver and destroyed once.
        unsafe { sys::vpx_codec_destroy(&mut self.ctx) };
    }
}

fn check(code: sys::vpx_codec_err_t, what: &str) -> Result<(), String> {
    if code == sys::VPX_CODEC_OK { Ok(()) } else { Err(format!("libvpx {what}: {code:?}")) }
}
