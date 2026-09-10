//! Acoustic echo cancellation for meeting mode.
//!
//! Replaces the non-linear WebRTC AEC (which heavily distorted local voice)
//! with a linear Frequency Domain Adaptive Filter (FDAF).

use fdaf_aec::FdafAec;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split, Observer};

pub const EXPECTED_ACOUSTIC_DELAY_MS: i32 = 50;

// fdaf-aec requires fft_size to be a power of two.
// A 4096 filter length at 48kHz covers ~85.3ms of acoustic delay.
const FFT_SIZE: usize = 4096;
const FRAME_SIZE: usize = FFT_SIZE / 2; // 2048

pub struct Aec {
    fdaf: FdafAec,
    render_prod: <HeapRb<f32> as Split>::Prod,
    render_cons: <HeapRb<f32> as Split>::Cons,
    capture_prod: <HeapRb<f32> as Split>::Prod,
    capture_cons: <HeapRb<f32> as Split>::Cons,
    output_prod: <HeapRb<f32> as Split>::Prod,
    output_cons: <HeapRb<f32> as Split>::Cons,
    disabled: bool,
    expected_delay_samples: usize,
    sample_rate: u32,
    initialized_delay: bool,
}

impl Aec {
    pub fn new(sample_rate: u32) -> Self {
        // Buffers need to be large enough to hold at least a few FDAF frames
        // plus the potential delay hint pre-padding.
        let render_rb = HeapRb::<f32>::new(32768);
        let (render_prod, render_cons) = render_rb.split();

        let capture_rb = HeapRb::<f32>::new(32768);
        let (capture_prod, capture_cons) = capture_rb.split();

        let output_rb = HeapRb::<f32>::new(32768);
        let (output_prod, output_cons) = output_rb.split();

        let fdaf = FdafAec::new(FFT_SIZE, 0.01);

        Self {
            fdaf,
            render_prod,
            render_cons,
            capture_prod,
            capture_cons,
            output_prod,
            output_cons,
            disabled: false,
            expected_delay_samples: 0,
            sample_rate,
            initialized_delay: false,
        }
    }

    pub fn new_with_default_delay_hint(sample_rate: u32) -> Self {
        let mut aec = Self::new(sample_rate);
        aec.set_expected_delay_ms(EXPECTED_ACOUSTIC_DELAY_MS);
        aec
    }

    pub fn set_expected_delay_ms(&mut self, delay_ms: i32) {
        if delay_ms > 0 {
            self.expected_delay_samples = (delay_ms as u32 * self.sample_rate / 1000) as usize;
        }
    }

    pub fn process_render(&mut self, frame: &[f32]) {
        if self.disabled { return; }
        if !self.initialized_delay && self.expected_delay_samples > 0 {
            // Push delay samples of silence into the render buffer so that render is delayed
            // to match capture. This perfectly aligns the signals if the hint is correct,
            // allowing the filter to easily cancel it.
            let delay_zeros = vec![0.0; self.expected_delay_samples];
            let _ = self.render_prod.push_slice(&delay_zeros);
            self.initialized_delay = true;
        }
        let _ = self.render_prod.push_slice(frame);
    }

    pub fn process_capture(&mut self, frame: &mut [f32]) {
        if self.disabled { return; }
        
        let _ = self.capture_prod.push_slice(frame);

        while self.render_cons.occupied_len() >= FRAME_SIZE && self.capture_cons.occupied_len() >= FRAME_SIZE {
            let mut render_chunk = vec![0.0; FRAME_SIZE];
            let mut capture_chunk = vec![0.0; FRAME_SIZE];
            
            self.render_cons.pop_slice(&mut render_chunk);
            self.capture_cons.pop_slice(&mut capture_chunk);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.fdaf.process(&render_chunk, &capture_chunk)
            }));

            match result {
                Ok(out_chunk) => {
                    let _ = self.output_prod.push_slice(&out_chunk);
                }
                Err(_) => {
                    self.disabled = true;
                    tracing::error!("FDAF AEC panicked. Disabling.");
                    break;
                }
            }
        }

        if self.output_cons.occupied_len() >= frame.len() {
            self.output_cons.pop_slice(frame);
        } else {
            // Before output buffer fills up, just pass the mic frame unmodified (with some
            // latency zero padding if we wanted exact delay match, but passing raw mic is fine
            // for the first few ms). Actually, wait. The output signal will be delayed by
            // FRAME_SIZE samples relative to the input if we do this. But since it's just audio
            // for ASR, a 42ms stutter at the very beginning of the meeting is invisible.
            // Let's just output silence until the filter produces data.
            for s in frame.iter_mut() {
                *s = 0.0;
            }
        }
    }
}
