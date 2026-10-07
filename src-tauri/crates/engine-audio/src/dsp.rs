//! DSP chain run inside the output callback (EQ lands here in stage 3).

pub struct DspChain {
    #[allow(dead_code)]
    sample_rate: f32,
}

impl DspChain {
    pub fn new(sample_rate: f32) -> Self {
        Self { sample_rate }
    }

    /// Process interleaved stereo in place. Must not allocate or block.
    pub fn process_interleaved(&mut self, _buf: &mut [f32]) {}
}
